//! One signed-in mailbox, talking to Microsoft Graph.
//!
//! Message ids are immutable ids (`Prefer: IdType="ImmutableId"`), so they survive moves
//! between folders. Read-only accounts refuse every change.

use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::auth::{self, Token};
use crate::config::{AccountConfig, Config};
use crate::error::{Error, ErrorCode, Result};
use crate::html;
use crate::model::*;

const GRAPH: &str = "https://graph.microsoft.com/v1.0";
const PREFER_IDS: &str = "IdType=\"ImmutableId\"";
const PREFER_HTML: &str = "IdType=\"ImmutableId\", outlook.body-content-type=\"html\"";
/// Graph rejects requests above ~4 MB; base64 adds a third.
pub const MAX_ATTACHMENT_BYTES: usize = 3 * 1024 * 1024;

pub const SUMMARY_FIELDS: &str = "id,subject,bodyPreview,isRead,isDraft,hasAttachments,importance,receivedDateTime,\
sentDateTime,lastModifiedDateTime,conversationId,parentFolderId,webLink,flag,from,toRecipients,ccRecipients";
const FULL_FIELDS: &str = "id,subject,bodyPreview,isRead,isDraft,hasAttachments,importance,receivedDateTime,\
sentDateTime,lastModifiedDateTime,conversationId,parentFolderId,webLink,flag,from,toRecipients,ccRecipients,\
bccRecipients,body";
const FOLDER_FIELDS: &str = "id,displayName,parentFolderId,childFolderCount,unreadItemCount,totalItemCount";

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(15)).timeout_read(Duration::from_secs(90)).build()
}

/// Who is signed in, asked with a bare access token (used right after sign-in).
pub fn fetch_me(access_token: &str) -> Result<Me> {
    agent()
        .get(&format!("{GRAPH}/me?$select=displayName,mail,userPrincipalName"))
        .set("Authorization", &format!("Bearer {access_token}"))
        .call()
        .map_err(transport_error)?
        .into_json()
        .map_err(|e| Error::general(format!("unexpected answer from Graph: {e}")))
}

fn transport_error(e: ureq::Error) -> Error {
    match e {
        ureq::Error::Status(status, resp) => graph_error(status, resp),
        ureq::Error::Transport(t) => {
            Error::new(ErrorCode::Network, format!("Microsoft Graph is not reachable: {t}")).hint("check the network")
        }
    }
}

fn graph_error(status: u16, resp: ureq::Response) -> Error {
    let body: Value = resp.into_json().unwrap_or(Value::Null);
    let code = body.pointer("/error/code").and_then(Value::as_str).unwrap_or("").to_string();
    let message = body.pointer("/error/message").and_then(Value::as_str).unwrap_or("").to_string();
    let text = if message.is_empty() { format!("Graph answered {status}") } else { format!("{message} ({code})") };
    match status {
        401 => Error::auth(text).hint("sign in again with `jm accounts add` or in Just Mail"),
        403 => Error::auth(text).hint("the account or app lacks permission for this"),
        404 => Error::not_found(text),
        400 | 422 => Error::validation(text),
        _ => Error::new(ErrorCode::Graph, text),
    }
}

/// What to list.
#[derive(Debug, Clone, Default)]
pub struct ListQuery {
    /// Folder id or well-known name; `None` lists the whole mailbox.
    pub folder: Option<String>,
    pub top: u32,
    pub unread: bool,
    pub flagged: bool,
    pub has_attachments: bool,
    pub from: Option<String>,
    pub since: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default)]
pub struct Page {
    pub messages: Vec<Message>,
    pub next_link: Option<String>,
}

pub struct Mailbox {
    pub account: AccountConfig,
    client_id: String,
    token: Mutex<Option<Token>>,
    agent: ureq::Agent,
}

impl Mailbox {
    pub fn new(account: AccountConfig, client_id: impl Into<String>) -> Self {
        Self { account, client_id: client_id.into(), token: Mutex::new(None), agent: agent() }
    }

    /// The account named `key` (id or address), or the first one.
    pub fn open(config: &Config, key: Option<&str>) -> Result<Self> {
        Ok(Self::new(config.account(key)?.clone(), config.client_id()))
    }

    pub fn id(&self) -> &str {
        &self.account.id
    }

    fn access_token(&self, force_refresh: bool) -> Result<String> {
        let mut slot = self.token.lock().unwrap_or_else(|e| e.into_inner());
        let mut token = match slot.take() {
            Some(t) => t,
            None => Token::load(&self.account.id)?,
        };
        if force_refresh || !token.is_fresh() {
            // another process (app or CLI) may have refreshed already
            let on_disk = Token::load(&self.account.id)?;
            token = if !force_refresh && on_disk.is_fresh() {
                on_disk
            } else {
                auth::refresh(&self.account.id, &self.client_id, &on_disk)?
            };
        }
        let access = token.access_token.clone();
        *slot = Some(token);
        Ok(access)
    }

    fn guard_writable(&self) -> Result<()> {
        if self.account.read_only {
            return Err(Error::validation(format!("account '{}' is read-only", self.account.id))
                .hint("another system processes this mailbox; change it in Outlook if you really must"));
        }
        Ok(())
    }

    /// Send one request; refreshes the token once on 401 and waits out throttling.
    fn call(&self, method: &str, url: &str, prefer: &str, body: Option<&Value>) -> Result<ureq::Response> {
        let url = if url.starts_with("https://") { url.to_string() } else { format!("{GRAPH}{url}") };
        let mut refreshed = false;
        let mut attempts = 0;
        loop {
            attempts += 1;
            let token = self.access_token(false)?;
            let req = self
                .agent
                .request(method, &url)
                .set("Authorization", &format!("Bearer {token}"))
                .set("Prefer", prefer);
            let result = match body {
                Some(b) => req.send_json(b.clone()),
                None if method == "POST" => req.set("Content-Length", "0").call(),
                None => req.call(),
            };
            match result {
                Ok(resp) => return Ok(resp),
                Err(ureq::Error::Status(401, _)) if !refreshed => {
                    refreshed = true;
                    self.access_token(true)?;
                }
                Err(ureq::Error::Status(status @ (429 | 503 | 504), resp)) if attempts < 4 => {
                    let wait = resp.header("Retry-After").and_then(|v| v.parse::<u64>().ok()).unwrap_or(2 * attempts);
                    let _ = status;
                    std::thread::sleep(Duration::from_secs(wait.min(30)));
                }
                Err(e) => return Err(transport_error(e)),
            }
        }
    }

    fn json<T: DeserializeOwned>(&self, method: &str, url: &str, prefer: &str, body: Option<&Value>) -> Result<T> {
        self.call(method, url, prefer, body)?
            .into_json()
            .map_err(|e| Error::general(format!("unexpected answer from Graph: {e}")))
    }

    fn get<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        self.json("GET", url, PREFER_IDS, None)
    }

    pub fn me(&self) -> Result<Me> {
        self.get("/me?$select=displayName,mail,userPrincipalName")
    }

    // ---- folders -------------------------------------------------------------------------

    /// All folders as a tree in display order: well-known folders first, then the rest by
    /// name, children right after their parent.
    pub fn folders(&self) -> Result<Vec<Folder>> {
        let mut all: Vec<Folder> = self.collect(&format!("/me/mailFolders?$top=250&$select={FOLDER_FIELDS}"))?;
        let mut parents: Vec<String> =
            all.iter().filter(|f| f.child_folder_count > 0).map(|f| f.id.clone()).collect();
        let mut rounds = 0;
        while !parents.is_empty() && rounds < 5 {
            rounds += 1;
            let mut next = Vec::new();
            for parent in parents {
                let children: Vec<Folder> = self.collect(&format!(
                    "/me/mailFolders/{}/childFolders?$top=250&$select={FOLDER_FIELDS}",
                    enc(&parent)
                ))?;
                next.extend(children.iter().filter(|f| f.child_folder_count > 0).map(|f| f.id.clone()));
                all.extend(children);
            }
            parents = next;
        }
        for (name, id) in self.well_known_ids()? {
            if let Some(f) = all.iter_mut().find(|f| f.id == id) {
                f.well_known = Some(name);
            }
        }
        Ok(order_tree(all))
    }

    /// Ids of the well-known folders, in one batch request.
    fn well_known_ids(&self) -> Result<Vec<(String, String)>> {
        let requests: Vec<Value> = WELL_KNOWN
            .iter()
            .map(|name| {
                json!({ "id": name, "method": "GET", "url": format!("/me/mailFolders/{name}?$select=id"),
                        "headers": { "Prefer": PREFER_IDS } })
            })
            .collect();
        let resp: Value = self.json("POST", "/$batch", PREFER_IDS, Some(&json!({ "requests": requests })))?;
        let mut out = Vec::new();
        for r in resp.get("responses").and_then(Value::as_array).into_iter().flatten() {
            let name = r.get("id").and_then(Value::as_str).unwrap_or_default();
            if let Some(id) = r.pointer("/body/id").and_then(Value::as_str) {
                out.push((name.to_string(), id.to_string()));
            }
        }
        Ok(out)
    }

    fn collect<T: DeserializeOwned>(&self, url: &str) -> Result<Vec<T>> {
        let mut out = Vec::new();
        let mut next = Some(url.to_string());
        while let Some(url) = next {
            let page: Value = self.get(&url)?;
            if let Some(items) = page.get("value") {
                out.extend(serde_json::from_value::<Vec<T>>(items.clone())?);
            }
            next = page.get("@odata.nextLink").and_then(Value::as_str).map(str::to_string);
        }
        Ok(out)
    }

    /// Folder id for a well-known name (`inbox`, `sent`, …), a display name or an id.
    pub fn resolve_folder(&self, name: &str) -> Result<String> {
        let lower = name.trim().to_lowercase();
        let well_known = match lower.as_str() {
            "inbox" | "posteingang" => Some("inbox"),
            "drafts" | "entwürfe" | "entwuerfe" => Some("drafts"),
            "sent" | "sentitems" | "gesendet" => Some("sentitems"),
            "archive" | "archiv" => Some("archive"),
            "trash" | "deleted" | "deleteditems" | "papierkorb" => Some("deleteditems"),
            "junk" | "spam" | "junkemail" => Some("junkemail"),
            _ => None,
        };
        if let Some(w) = well_known {
            return Ok(w.to_string());
        }
        if name.len() > 40 && !name.contains(' ') {
            return Ok(name.to_string());
        }
        let folders = self.folders()?;
        folders
            .iter()
            .find(|f| f.display_name.to_lowercase() == lower)
            .map(|f| f.id.clone())
            .ok_or_else(|| Error::not_found(format!("no folder named '{name}'")).hint("list folders with `jm folders`"))
    }

    // ---- reading -------------------------------------------------------------------------

    pub fn list(&self, q: &ListQuery) -> Result<Page> {
        let base = match &q.folder {
            Some(folder) => format!("/me/mailFolders/{}/messages", enc(folder)),
            None => "/me/messages".to_string(),
        };
        let mut filters = Vec::new();
        if q.unread {
            filters.push("isRead eq false".to_string());
        }
        if q.flagged {
            filters.push("flag/flagStatus eq 'flagged'".to_string());
        }
        if q.has_attachments {
            filters.push("hasAttachments eq true".to_string());
        }
        if let Some(from) = &q.from {
            filters.push(format!("from/emailAddress/address eq '{}'", from.replace('\'', "''")));
        }
        let mut url = format!("{base}?$top={}&$select={SUMMARY_FIELDS}&$orderby=receivedDateTime desc", q.top.max(1));
        if q.since.is_some() || !filters.is_empty() {
            // Graph insists that the $orderby property leads the $filter
            let since = q.since.map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string());
            let mut all = vec![format!("receivedDateTime ge {}", since.as_deref().unwrap_or("1900-01-01T00:00:00Z"))];
            all.extend(filters);
            url.push_str(&format!("&$filter={}", enc(&all.join(" and "))));
        }
        self.page(&url)
    }

    pub fn page(&self, url: &str) -> Result<Page> {
        let value: Value = self.get(url)?;
        let messages = value.get("value").cloned().map(serde_json::from_value).transpose()?.unwrap_or_default();
        let next_link = value.get("@odata.nextLink").and_then(Value::as_str).map(str::to_string);
        Ok(Page { messages, next_link })
    }

    /// Full-text search (subject, body, people). Graph orders results by date.
    pub fn search(&self, query: &str, folder: Option<&str>, top: u32) -> Result<Vec<Message>> {
        let base = match folder {
            Some(f) => format!("/me/mailFolders/{}/messages", enc(f)),
            None => "/me/messages".to_string(),
        };
        let quoted = format!("\"{}\"", query.replace('"', ""));
        let url = format!("{base}?$search={}&$top={}&$select={SUMMARY_FIELDS}", enc(&quoted), top.max(1));
        Ok(self.page(&url)?.messages)
    }

    /// A message with its HTML body.
    pub fn message(&self, id: &str) -> Result<Message> {
        self.json("GET", &format!("/me/messages/{}?$select={FULL_FIELDS}", enc(id)), PREFER_HTML, None)
    }

    pub fn attachments(&self, id: &str) -> Result<Vec<Attachment>> {
        self.collect(&format!("/me/messages/{}/attachments?$select=id,name,contentType,size,isInline", enc(id)))
    }

    pub fn attachment_bytes(&self, message_id: &str, attachment_id: &str) -> Result<Vec<u8>> {
        let resp = self.call(
            "GET",
            &format!("/me/messages/{}/attachments/{}/$value", enc(message_id), enc(attachment_id)),
            PREFER_IDS,
            None,
        )?;
        let mut bytes = Vec::new();
        resp.into_reader().read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    // ---- changing ------------------------------------------------------------------------

    fn patch(&self, id: &str, body: Value) -> Result<Message> {
        self.guard_writable()?;
        self.json("PATCH", &format!("/me/messages/{}", enc(id)), PREFER_IDS, Some(&body))
    }

    pub fn set_read(&self, id: &str, read: bool) -> Result<Message> {
        self.patch(id, json!({ "isRead": read }))
    }

    pub fn set_flag(&self, id: &str, status: FlagStatus) -> Result<Message> {
        self.patch(id, json!({ "flag": { "flagStatus": status } }))
    }

    /// Move to a folder (id or well-known name); returns the moved message.
    pub fn move_to(&self, id: &str, folder: &str) -> Result<Message> {
        self.guard_writable()?;
        self.json(
            "POST",
            &format!("/me/messages/{}/move", enc(id)),
            PREFER_IDS,
            Some(&json!({ "destinationId": folder })),
        )
    }

    pub fn archive(&self, id: &str) -> Result<Message> {
        self.move_to(id, "archive")
    }

    /// Into Deleted Items (recoverable).
    pub fn trash(&self, id: &str) -> Result<Message> {
        self.move_to(id, "deleteditems")
    }

    // ---- drafts --------------------------------------------------------------------------

    fn signature(&self, with_signature: bool) -> &str {
        if with_signature { &self.account.signature } else { "" }
    }

    /// A new draft in Drafts. `input.body_html` is the author's part; the signature follows.
    pub fn create_draft(&self, input: &DraftInput, with_signature: bool, files: &[NewAttachment]) -> Result<Message> {
        self.guard_writable()?;
        let body = html::new_document(input.body_html.as_deref().unwrap_or(""), self.signature(with_signature));
        let mut msg = json!({
            "subject": input.subject.clone().unwrap_or_default(),
            "body": { "contentType": "HTML", "content": body },
            "toRecipients": recipients(input.to.as_deref()),
            "ccRecipients": recipients(input.cc.as_deref()),
            "bccRecipients": recipients(input.bcc.as_deref()),
        });
        if let Some(importance) = &input.importance {
            msg["importance"] = json!(importance);
        }
        if !files.is_empty() {
            msg["attachments"] = Value::Array(files.iter().map(file_attachment).collect::<Result<_>>()?);
        }
        let created: Message = self.json("POST", "/me/messages", PREFER_IDS, Some(&msg))?;
        self.message(&created.id)
    }

    /// Change a draft. A new body replaces only the author's part when the draft has Just
    /// Mail's markers (signature and quote stay), otherwise the whole body.
    pub fn update_draft(&self, id: &str, input: &DraftInput) -> Result<Message> {
        self.guard_writable()?;
        let mut patch = json!({});
        if let Some(to) = &input.to {
            patch["toRecipients"] = recipients(Some(to));
        }
        if let Some(cc) = &input.cc {
            patch["ccRecipients"] = recipients(Some(cc));
        }
        if let Some(bcc) = &input.bcc {
            patch["bccRecipients"] = recipients(Some(bcc));
        }
        if let Some(subject) = &input.subject {
            patch["subject"] = json!(subject);
        }
        if let Some(importance) = &input.importance {
            patch["importance"] = json!(importance);
        }
        if let Some(body_html) = &input.body_html {
            let current = self.message(id)?;
            if !current.is_draft {
                return Err(Error::validation("this message is not a draft"));
            }
            let old = current.body.map(|b| b.content).unwrap_or_default();
            let new = html::replace_author_html(&old, body_html, &self.account.signature);
            patch["body"] = json!({ "contentType": "HTML", "content": new });
        }
        self.patch(id, patch)?;
        self.message(id)
    }

    /// A reply draft in the conversation, the author's part and signature above the quote.
    pub fn create_reply(&self, id: &str, all: bool, body_html: &str, with_signature: bool) -> Result<Message> {
        self.guard_writable()?;
        let action = if all { "createReplyAll" } else { "createReply" };
        let draft: Message = self.json("POST", &format!("/me/messages/{}/{action}", enc(id)), PREFER_IDS, Some(&json!({})))?;
        self.fill_answer(&draft.id, body_html, with_signature, None)
    }

    /// A forward draft (with the original attachments).
    pub fn create_forward(
        &self,
        id: &str,
        to: &[Recipient],
        body_html: &str,
        with_signature: bool,
    ) -> Result<Message> {
        self.guard_writable()?;
        let draft: Message = self.json(
            "POST",
            &format!("/me/messages/{}/createForward", enc(id)),
            PREFER_IDS,
            Some(&json!({ "toRecipients": recipients(Some(to)) })),
        )?;
        self.fill_answer(&draft.id, body_html, with_signature, Some(to))
    }

    fn fill_answer(&self, draft_id: &str, body_html: &str, with_signature: bool, to: Option<&[Recipient]>) -> Result<Message> {
        let draft = self.message(draft_id)?;
        let quote = draft.body.map(|b| b.content).unwrap_or_default();
        let blocks = html::compose_blocks(body_html, self.signature(with_signature));
        let mut patch = json!({ "body": { "contentType": "HTML", "content": html::insert_at_top(&quote, &blocks) } });
        if let Some(to) = to {
            patch["toRecipients"] = recipients(Some(to));
        }
        self.patch(draft_id, patch)?;
        self.message(draft_id)
    }

    pub fn add_attachment(&self, draft_id: &str, file: &NewAttachment) -> Result<Attachment> {
        self.guard_writable()?;
        self.json("POST", &format!("/me/messages/{}/attachments", enc(draft_id)), PREFER_IDS, Some(&file_attachment(file)?))
    }

    pub fn remove_attachment(&self, draft_id: &str, attachment_id: &str) -> Result<()> {
        self.guard_writable()?;
        self.call("DELETE", &format!("/me/messages/{}/attachments/{}", enc(draft_id), enc(attachment_id)), PREFER_IDS, None)?;
        Ok(())
    }

    /// Delete a draft for good (drafts only; other mail goes to the trash).
    pub fn delete_draft(&self, id: &str) -> Result<()> {
        self.guard_writable()?;
        let current: Message = self.get(&format!("/me/messages/{}?$select=id,isDraft", enc(id)))?;
        if !current.is_draft {
            return Err(Error::validation("this message is not a draft").hint("use trash for received mail"));
        }
        self.call("DELETE", &format!("/me/messages/{}", enc(id)), PREFER_IDS, None)?;
        Ok(())
    }

    /// Send a draft. Only compiled into the desktop app.
    #[cfg(feature = "send")]
    pub fn send_draft(&self, id: &str) -> Result<()> {
        self.guard_writable()?;
        self.call("POST", &format!("/me/messages/{}/send", enc(id)), PREFER_IDS, None)?;
        Ok(())
    }
}

use std::io::Read as _;

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>().replace('+', "%20")
}

fn recipients(list: Option<&[Recipient]>) -> Value {
    Value::Array(list.unwrap_or_default().iter().map(Recipient::to_graph).collect())
}

fn file_attachment(file: &NewAttachment) -> Result<Value> {
    use base64::Engine as _;
    if file.bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(Error::validation(format!("{} is larger than 3 MB", file.name))
            .hint("share large files as a link instead"));
    }
    Ok(json!({
        "@odata.type": "#microsoft.graph.fileAttachment",
        "name": file.name,
        "contentType": file.content_type,
        "contentBytes": base64::engine::general_purpose::STANDARD.encode(&file.bytes),
    }))
}

fn order_tree(all: Vec<Folder>) -> Vec<Folder> {
    let ids: std::collections::HashSet<String> = all.iter().map(|f| f.id.clone()).collect();
    let is_root = |f: &Folder| f.parent_folder_id.as_ref().is_none_or(|p| !ids.contains(p));
    let mut roots: Vec<&Folder> = all.iter().filter(|f| is_root(f)).collect();
    roots.sort_by_key(|f| {
        let rank = f.well_known.as_deref().and_then(|w| WELL_KNOWN.iter().position(|k| *k == w)).unwrap_or(usize::MAX);
        (rank, f.display_name.to_lowercase())
    });
    let mut out = Vec::with_capacity(all.len());
    fn push(f: &Folder, depth: u32, all: &[Folder], out: &mut Vec<Folder>) {
        let mut f = f.clone();
        f.depth = depth;
        let id = f.id.clone();
        out.push(f);
        let mut children: Vec<&Folder> = all.iter().filter(|c| c.parent_folder_id.as_deref() == Some(&id)).collect();
        children.sort_by_key(|c| c.display_name.to_lowercase());
        for c in children {
            push(c, depth + 1, all, out);
        }
    }
    for r in roots {
        push(r, 0, &all, &mut out);
    }
    out
}
