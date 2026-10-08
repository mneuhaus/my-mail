//! `jm mcp`: a Model Context Protocol server on stdio (newline-delimited JSON-RPC 2.0).
//!
//! The tools are thin wrappers around [`crate::ops`]; every result is the same JSON object the
//! CLI prints with `--json`. There is no way to send mail: drafts are only prepared here and
//! sent by the user in the Just Mail app.

use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;

use jm_core::{Config, Error, FlagStatus, Result};
use serde_json::{Map, Value, json};

use crate::ops::{self, Ctx, DraftParams, ListParams};

const PROTOCOL_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];
const LATEST_PROTOCOL: &str = "2025-06-18";
const NOT_SENT: &str = "The draft is NOT sent: no tool here can send mail. The user (Marc) reviews and sends \
drafts in the Just Mail app.";
const INSTRUCTIONS: &str = "Just Mail: read the user's Microsoft 365 mail and prepare drafts. Nothing here can send \
mail; the user (Marc) reviews and sends drafts in the Just Mail app. Message ids are 8-character short ids from \
list/search results (full Graph ids work too). Read-only accounts (e.g. invoice) refuse every change.";

/// Serve until stdin closes.
pub fn serve() -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) => {
                let replies: Vec<Value> = batch.iter().filter_map(handle).collect();
                (!replies.is_empty()).then_some(Value::Array(replies))
            }
            Ok(message) => handle(&message),
            Err(e) => Some(rpc_error(Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(reply) = reply {
            writeln!(stdout, "{reply}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// Answer one JSON-RPC message; notifications get no answer.
pub fn handle(message: &Value) -> Option<Value> {
    let method = message.get("method").and_then(Value::as_str);
    let Some(id) = message.get("id").cloned() else {
        return None; // notification (e.g. notifications/initialized) or a stray response
    };
    let Some(method) = method else {
        return Some(rpc_error(id, -32600, "invalid request: no method"));
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => rpc_result(id, initialize(&params)),
        "ping" => rpc_result(id, json!({})),
        "tools/list" => rpc_result(id, json!({ "tools": tools() })),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(rpc_error(id, -32602, "tools/call needs a tool name"));
            };
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            rpc_result(id, call_tool(name, &args))
        }
        other => rpc_error(id, -32601, &format!("method not found: {other}")),
    })
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
    let version = if PROTOCOL_VERSIONS.contains(&asked) { asked } else { LATEST_PROTOCOL };
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "just-mail", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

/// Run a tool; failures come back as `isError` results the model can read and fix.
pub fn call_tool(name: &str, args: &Value) -> Value {
    let (text, is_error) = match run_tool(name, args) {
        Ok(value) => (value.to_string(), false),
        Err(e) => (json!({ "error": e }).to_string(), true),
    };
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

fn run_tool(name: &str, args: &Value) -> Result<Value> {
    if name.contains("send") {
        return Err(ops::refuse_send());
    }
    let a = Args::new(args)?;
    let ctx = || Ctx::load(a.opt_str("account")?);
    match name {
        "list_accounts" => Ok(ops::accounts(&Config::load()?)),
        "list_folders" => ops::folders(&ctx()?),
        "list_messages" => ops::list(
            &ctx()?,
            &ListParams {
                folder: a.opt_str("folder")?,
                limit: a.limit()?,
                unread: a.flag("unread")?,
                flagged: a.flag("flagged")?,
                attachments: a.flag("attachments")?,
                from: a.opt_str("from")?,
                since: a.opt_str("since")?,
                all: a.flag("all")?,
                next: a.opt_str("next")?,
            },
        ),
        "search_messages" => ops::search(&ctx()?, &a.str("query")?, a.opt_str("folder")?.as_deref(), a.limit()?),
        "get_message" => ops::show(&ctx()?, &a.str("id")?, a.flag("html")?, false),
        "download_attachments" => {
            let dir = a.opt_str("dir")?.map(PathBuf::from);
            ops::download(&ctx()?, &a.str("id")?, &a.list("names")?, dir.as_deref())
        }
        "set_flag" => {
            let status = match a.str("status")?.as_str() {
                "flagged" => FlagStatus::Flagged,
                "complete" => FlagStatus::Complete,
                "none" | "notFlagged" => FlagStatus::NotFlagged,
                other => return Err(Error::validation(format!("status '{other}' is not flagged, complete or none"))),
            };
            ops::set_flag(&ctx()?, &a.str("id")?, status)
        }
        "mark_read" => {
            let read =
                a.opt_bool("read")?.ok_or_else(|| Error::validation("missing argument 'read' (true or false)"))?;
            ops::mark_read(&ctx()?, &a.str("id")?, read)
        }
        "move_message" => ops::move_message(&ctx()?, &a.str("id")?, &a.str("to")?),
        "list_drafts" => ops::list_drafts(&ctx()?, a.limit()?),
        "create_draft" => ops::create_draft(&ctx()?, &a.draft()?),
        "reply_draft" => ops::reply_draft(&ctx()?, &a.str("id")?, a.flag("all")?, &a.draft()?),
        "forward_draft" => ops::forward_draft(&ctx()?, &a.str("id")?, &a.draft()?),
        "update_draft" => ops::update_draft(&ctx()?, &a.str("id")?, &a.draft()?),
        "delete_draft" => ops::delete_draft(&ctx()?, &a.str("id")?),
        other => Err(Error::not_found(format!("unknown tool '{other}'")).hint("see tools/list")),
    }
}

/// Typed access to tool arguments with readable errors.
struct Args<'a>(&'a Map<String, Value>);

static EMPTY: std::sync::LazyLock<Map<String, Value>> = std::sync::LazyLock::new(Map::new);

impl<'a> Args<'a> {
    fn new(args: &'a Value) -> Result<Self> {
        match args {
            Value::Object(map) => Ok(Args(map)),
            Value::Null => Ok(Args(&EMPTY)),
            _ => Err(Error::validation("tool arguments must be an object")),
        }
    }

    fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key).filter(|v| !v.is_null())
    }

    fn opt_str(&self, key: &str) -> Result<Option<String>> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(Error::validation(format!("argument '{key}' must be a string"))),
        }
    }

    fn str(&self, key: &str) -> Result<String> {
        self.opt_str(key)?.ok_or_else(|| Error::validation(format!("missing argument '{key}'")))
    }

    fn opt_bool(&self, key: &str) -> Result<Option<bool>> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(Error::validation(format!("argument '{key}' must be true or false"))),
        }
    }

    fn flag(&self, key: &str) -> Result<bool> {
        Ok(self.opt_bool(key)?.unwrap_or(false))
    }

    fn limit(&self) -> Result<u32> {
        match self.get("limit") {
            None => Ok(25),
            Some(v) => v
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| Error::validation("argument 'limit' must be a positive number")),
        }
    }

    /// A string list given as an array of strings or one comma-separated string.
    fn list(&self, key: &str) -> Result<Vec<String>> {
        match self.get(key) {
            None => Ok(vec![]),
            Some(Value::String(s)) if s.trim().is_empty() => Ok(vec![]),
            Some(Value::String(s)) => Ok(vec![s.clone()]),
            Some(Value::Array(items)) => items
                .iter()
                .map(|i| {
                    i.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| Error::validation(format!("argument '{key}' must list strings")))
                })
                .collect(),
            Some(_) => Err(Error::validation(format!("argument '{key}' must be a string or a list of strings"))),
        }
    }

    fn draft(&self) -> Result<DraftParams> {
        Ok(DraftParams {
            to: self.list("to")?,
            cc: self.list("cc")?,
            bcc: self.list("bcc")?,
            subject: self.opt_str("subject")?,
            body: self.get("body").and_then(Value::as_str).map(str::to_string),
            body_file: None,
            html: self.flag("html")?,
            attachments: self.list("attachments")?.into_iter().map(PathBuf::from).collect(),
            signature: self.opt_bool("signature")?.unwrap_or(true),
            importance: self.opt_str("importance")?,
        })
    }
}

// ---- tool definitions ------------------------------------------------------------------------

fn tool(name: &str, description: &str, properties: Value, required: &[&str], read_only: bool) -> Value {
    let mut props = properties.as_object().cloned().unwrap_or_default();
    props.insert(
        "account".into(),
        json!({ "type": "string", "description": "Account id or address. Default: the account a short id belongs to, else the first account." }),
    );
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": props, "required": required },
        "annotations": { "readOnlyHint": read_only, "openWorldHint": true },
    })
}

pub fn tools() -> Vec<Value> {
    let id = json!({ "type": "string", "description": "Message id: 8-character short id (or prefix) from a listing, or the full Graph id" });
    let limit =
        json!({ "type": "integer", "minimum": 1, "maximum": ops::MAX_LIMIT, "description": "How many (default 25)" });
    let folder = json!({ "type": "string", "description": "inbox, drafts, sent, archive, trash, junk, a folder name or a folder id" });
    let addresses = json!({
        "description": "Addresses: a list of strings or one comma-separated string; 'Name <a@b.c>' works",
        "anyOf": [{ "type": "string" }, { "type": "array", "items": { "type": "string" } }]
    });
    let body = json!({ "type": "string", "description": "The text you write (plain text unless html is true). Signature and quoted mail are added below it." });
    let html = json!({ "type": "boolean", "description": "body is HTML (default: plain text)" });
    let signature = json!({ "type": "boolean", "description": "Append the account signature (default true)" });
    let files = json!({ "type": "array", "items": { "type": "string" }, "description": "Local file paths to attach (max 3 MB each)" });
    let importance = json!({ "type": "string", "enum": ["low", "normal", "high"] });

    vec![
        tool(
            "list_accounts",
            "List the configured Microsoft 365 accounts: id, address, name, read_only, signed_in, signature set, default (the first).",
            json!({}),
            &[],
            true,
        ),
        tool("list_folders", "The folder tree of a mailbox with unread/total counts.", json!({}), &[], true),
        tool(
            "list_messages",
            "List messages, newest first. Default folder: inbox. 'flagged' without a folder (or all=true) covers the whole mailbox. Returns short ids for the other tools and 'next' when there is more.",
            json!({
                "folder": folder,
                "limit": limit,
                "unread": { "type": "boolean", "description": "Only unread" },
                "flagged": { "type": "boolean", "description": "Only flagged" },
                "attachments": { "type": "boolean", "description": "Only with attachments" },
                "from": { "type": "string", "description": "Only from this address" },
                "since": { "type": "string", "description": "Only newer than: 36h, 7d, 2w, today, yesterday, 2026-10-01 or an ISO time" },
                "all": { "type": "boolean", "description": "Whole mailbox (all folders)" },
                "next": { "type": "string", "description": "Continue a listing: its 'next' value" }
            }),
            &[],
            true,
        ),
        tool(
            "search_messages",
            "Full-text search over subject, body and people (whole mailbox unless a folder is given), newest first.",
            json!({ "query": { "type": "string" }, "folder": folder, "limit": limit }),
            &["query"],
            true,
        ),
        tool(
            "get_message",
            "One message: headers, the body as readable plain text, attachment list. Drafts also return author_text (the part above signature and quote). Reading does not mark it read.",
            json!({ "id": id, "html": { "type": "boolean", "description": "Also return the HTML body" } }),
            &["id"],
            true,
        ),
        tool(
            "download_attachments",
            "Save a message's attachments to disk (default ~/Downloads; existing files are not overwritten) and return the paths. Without names: all file attachments except inline images.",
            json!({
                "id": id,
                "names": { "type": "array", "items": { "type": "string" }, "description": "Attachment names to save" },
                "dir": { "type": "string", "description": "Target folder" }
            }),
            &["id"],
            false,
        ),
        tool(
            "set_flag",
            "Flag a message, mark the flag complete, or clear it.",
            json!({ "id": id, "status": { "type": "string", "enum": ["flagged", "complete", "none"] } }),
            &["id", "status"],
            false,
        ),
        tool(
            "mark_read",
            "Mark a message read (read=true) or unread (read=false).",
            json!({ "id": id, "read": { "type": "boolean" } }),
            &["id", "read"],
            false,
        ),
        tool(
            "move_message",
            "Move a message to archive, trash (Deleted Items, recoverable) or any folder.",
            json!({ "id": id, "to": { "type": "string", "description": "archive, trash, or a folder name or id" } }),
            &["id", "to"],
            false,
        ),
        tool("list_drafts", "List drafts, newest first.", json!({ "limit": limit }), &[], true),
        tool(
            "create_draft",
            &format!(
                "Create a new draft in the Drafts folder, with the account signature unless signature=false. {NOT_SENT}"
            ),
            json!({
                "to": addresses, "cc": addresses, "bcc": addresses,
                "subject": { "type": "string" }, "body": body, "html": html,
                "signature": signature, "attachments": files, "importance": importance
            }),
            &["to", "subject", "body"],
            false,
        ),
        tool(
            "reply_draft",
            &format!(
                "Create a reply draft to a message (all=true: reply all). Your body and the signature go above the quoted mail. {NOT_SENT}"
            ),
            json!({
                "id": id, "all": { "type": "boolean", "description": "Reply to all" },
                "body": body, "html": html, "signature": signature, "attachments": files
            }),
            &["id", "body"],
            false,
        ),
        tool(
            "forward_draft",
            &format!("Create a forward draft of a message (keeps its attachments). {NOT_SENT}"),
            json!({
                "id": id, "to": addresses, "body": body, "html": html,
                "signature": signature, "attachments": files
            }),
            &["id", "to"],
            false,
        ),
        tool(
            "update_draft",
            &format!(
                "Change a draft. A new body replaces only the author's part; signature and quote stay. Recipients given replace the old ones; files are added. {NOT_SENT}"
            ),
            json!({
                "id": id, "to": addresses, "cc": addresses, "bcc": addresses,
                "subject": { "type": "string" }, "body": body, "html": html,
                "importance": importance, "attachments": files
            }),
            &["id"],
            false,
        ),
        tool(
            "delete_draft",
            "Delete a draft for good. Only drafts can be deleted; received mail is refused.",
            json!({ "id": id }),
            &["id"],
            false,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_result_error(reply: &Value) -> (bool, Value) {
        let result = &reply["result"];
        let text = result["content"][0]["text"].as_str().unwrap();
        (result["isError"].as_bool().unwrap(), serde_json::from_str(text).unwrap())
    }

    #[test]
    fn initialize_negotiates_the_protocol() {
        let req = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"1"}}});
        let reply = handle(&req).unwrap();
        assert_eq!(reply["id"], 1);
        assert_eq!(reply["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(reply["result"]["serverInfo"]["name"], "just-mail");
        assert!(reply["result"]["capabilities"]["tools"].is_object());
        let future = json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2099-01-01"}});
        assert_eq!(handle(&future).unwrap()["result"]["protocolVersion"], LATEST_PROTOCOL);
    }

    #[test]
    fn notifications_get_no_answer_and_unknown_methods_an_error() {
        assert!(handle(&json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());
        let reply = handle(&json!({"jsonrpc":"2.0","id":"x","method":"resources/list"})).unwrap();
        assert_eq!(reply["error"]["code"], -32601);
        assert_eq!(handle(&json!({"jsonrpc":"2.0","id":3,"method":"ping"})).unwrap()["result"], json!({}));
    }

    #[test]
    fn tools_list_has_the_mail_tools_and_no_sending() {
        let reply = handle(&json!({"jsonrpc":"2.0","id":4,"method":"tools/list"})).unwrap();
        let tools = reply["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        for expected in [
            "list_accounts",
            "list_folders",
            "list_messages",
            "search_messages",
            "get_message",
            "download_attachments",
            "set_flag",
            "mark_read",
            "move_message",
            "list_drafts",
            "create_draft",
            "reply_draft",
            "forward_draft",
            "update_draft",
            "delete_draft",
        ] {
            assert!(names.contains(&expected), "{expected} missing");
        }
        assert!(!names.iter().any(|n| n.contains("send")));
        for t in tools {
            assert_eq!(t["inputSchema"]["type"], "object");
            assert!(t["inputSchema"]["properties"]["account"].is_object());
            let name = t["name"].as_str().unwrap();
            if ["create_draft", "reply_draft", "forward_draft", "update_draft"].contains(&name) {
                assert!(t["description"].as_str().unwrap().contains("NOT sent"), "{name}");
            }
        }
    }

    #[test]
    fn unknown_tools_are_errors_the_model_can_read() {
        let req = json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"frobnicate","arguments":{}}});
        let (is_error, body) = tool_result_error(&handle(&req).unwrap());
        assert!(is_error);
        assert_eq!(body["error"]["code"], "NOT_FOUND");
    }

    #[test]
    fn sending_is_refused() {
        for name in ["send_draft", "send", "send_message"] {
            let req =
                json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":name,"arguments":{"id":"abc"}}});
            let (is_error, body) = tool_result_error(&handle(&req).unwrap());
            assert!(is_error);
            assert_eq!(body["error"]["code"], "VALIDATION");
            assert_eq!(body["error"]["message"], ops::SEND_REFUSAL);
        }
    }

    #[test]
    fn bad_arguments_are_validation_errors() {
        let req =
            json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"get_message","arguments":{"id":5}}});
        let (is_error, body) = tool_result_error(&handle(&req).unwrap());
        assert!(is_error);
        assert_eq!(body["error"]["code"], "VALIDATION");
    }
}
