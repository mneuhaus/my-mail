//! The operations behind both the CLI commands and the MCP tools. Each one takes typed
//! parameters and returns one JSON object; the CLI prints it (as JSON or formatted for people),
//! the MCP server hands it to the model as text.
//!
//! Nothing here can send mail: the CLI is built without jm-core's `send` feature.

use std::collections::HashSet;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use jm_core::events::{self, Event};
use jm_core::graph::{self, ListQuery};
use jm_core::{
    AccountConfig, Attachment, Config, DraftInput, Error, FlagStatus, Mailbox, Message, NewAttachment, Recipient,
    Result, auth, html, ids, paths,
};
use serde_json::{Value, json};

pub const SEND_REFUSAL: &str = "Sending is not available in the CLI: open the draft in Just Mail and send it there.";
const APP_BUNDLE_ID: &str = "nrw.neuhaus.justmail";
const TEXT_WIDTH: usize = 100;
pub const MAX_LIMIT: u32 = 1000;

/// The answer to every attempt to send (CLI command or MCP tool).
pub fn refuse_send() -> Error {
    Error::validation(SEND_REFUSAL).hint("drafts land in the Drafts folder; `jm open <id>` shows one in Just Mail")
}

/// Loaded config plus the account picked with `-a` / the `account` argument.
pub struct Ctx {
    pub config: Config,
    pub account: Option<String>,
}

impl Ctx {
    pub fn load(account: Option<String>) -> Result<Self> {
        Ok(Self { config: Config::load()?, account: account.filter(|a| !a.trim().is_empty()) })
    }

    /// The picked account, else the first.
    fn mailbox(&self) -> Result<Mailbox> {
        Mailbox::open(&self.config, self.account.as_deref())
    }

    /// Mailbox and full Graph id for a short id (or prefix) or full id. A short id knows its
    /// account, which is used unless one was picked explicitly.
    fn target(&self, id: &str) -> Result<(Mailbox, String)> {
        let resolved = ids::resolve(id)?;
        if let (Some(picked), Some(owner)) = (&self.account, &resolved.account) {
            let picked = &self.config.account(Some(picked))?.id;
            if picked != owner {
                return Err(Error::validation(format!("message '{id}' belongs to account '{owner}', not '{picked}'"))
                    .hint(format!("drop -a or use -a {owner}")));
            }
        }
        let key = self.account.as_deref().or(resolved.account.as_deref());
        Ok((Mailbox::open(&self.config, key)?, resolved.id))
    }
}

/// Tell a running Just Mail what changed so it refreshes.
fn changed(mb: &Mailbox, folder: Option<&str>) {
    events::emit(&Event::Changed { account: mb.id().to_string(), folder: folder.map(str::to_string) });
}

// ---- parameters ------------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct ListParams {
    /// Folder name or id; `None` = inbox (or the whole mailbox with `flagged` / `all`).
    pub folder: Option<String>,
    pub limit: u32,
    pub unread: bool,
    pub flagged: bool,
    pub attachments: bool,
    pub from: Option<String>,
    pub since: Option<String>,
    pub all: bool,
    /// Continue an earlier listing (its `next` value).
    pub next: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DraftParams {
    /// Recipient lists; each entry may itself be a comma-separated list.
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: Option<String>,
    pub body: Option<String>,
    /// `-` reads stdin.
    pub body_file: Option<PathBuf>,
    /// The body is HTML already (default: plain text).
    pub html: bool,
    pub attachments: Vec<PathBuf>,
    /// Append the account signature (new drafts, replies, forwards).
    pub signature: bool,
    pub importance: Option<String>,
}

// ---- input helpers ---------------------------------------------------------------------------

/// `--since`: `36h`, `7d`, `2w`, `today`, `yesterday`, a date (local midnight) or an RFC 3339 time.
pub fn parse_since(input: &str, now: DateTime<Local>) -> Result<DateTime<Utc>> {
    let s = input.trim().to_lowercase();
    let invalid = || {
        Error::validation(format!("cannot read the time '{input}'"))
            .hint("use 36h, 7d, 2w, today, yesterday, 2026-10-01 or 2026-10-01T14:00:00Z")
    };
    let today = now.date_naive();
    match s.as_str() {
        "today" => return local_midnight(today).ok_or_else(invalid),
        "yesterday" => return local_midnight(today.pred_opt().ok_or_else(invalid)?).ok_or_else(invalid),
        _ => {}
    }
    if let Some(unit) = s.chars().last()
        && matches!(unit, 'h' | 'd' | 'w')
    {
        let n: u32 = s[..s.len() - 1].trim().parse().map_err(|_| invalid())?;
        let span = match unit {
            'h' => chrono::Duration::hours(n.into()),
            'd' => chrono::Duration::days(n.into()),
            _ => chrono::Duration::weeks(n.into()),
        };
        return Ok((now - span).with_timezone(&Utc));
    }
    if let Ok(date) = NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
        return local_midnight(date).ok_or_else(invalid);
    }
    DateTime::parse_from_rfc3339(input.trim()).map(|t| t.with_timezone(&Utc)).map_err(|_| invalid())
}

fn local_midnight(date: NaiveDate) -> Option<DateTime<Utc>> {
    Local.from_local_datetime(&date.and_hms_opt(0, 0, 0)?).earliest().map(|t| t.with_timezone(&Utc))
}

/// Recipients from repeated and/or comma-separated values; `None` when none were given.
pub fn parse_recipients(values: &[String]) -> Result<Option<Vec<Recipient>>> {
    if values.is_empty() {
        return Ok(None);
    }
    Recipient::parse_list(&values.join(", ")).map(Some)
}

/// The author's HTML from `body` or `body_file` (`-` = stdin). Plain text unless `as_html`.
pub fn body_html(body: Option<&str>, body_file: Option<&Path>, as_html: bool) -> Result<Option<String>> {
    let text = match (body, body_file) {
        (Some(_), Some(_)) => return Err(Error::validation("use either a body or a body file, not both")),
        (Some(body), None) => body.to_string(),
        (None, Some(path)) if path == Path::new("-") => {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)?;
            text
        }
        (None, Some(path)) => std::fs::read_to_string(path)
            .map_err(|e| Error::not_found(format!("cannot read body file {}: {e}", path.display())))?,
        (None, None) => return Ok(None),
    };
    Ok(Some(if as_html { text } else { html::text_to_html(&text) }))
}

/// Read every file before anything is created, so a bad path changes nothing.
fn load_files(paths: &[PathBuf]) -> Result<Vec<NewAttachment>> {
    paths
        .iter()
        .map(|p| {
            let file = NewAttachment::from_path(&expand_home(p))?;
            if file.bytes.len() > graph::MAX_ATTACHMENT_BYTES {
                return Err(Error::validation(format!("{} is larger than 3 MB", p.display()))
                    .hint("share large files as a link instead"));
            }
            Ok(file)
        })
        .collect()
}

fn expand_home(path: &Path) -> PathBuf {
    match (path.strip_prefix("~"), std::env::var_os("HOME")) {
        (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => path.to_path_buf(),
    }
}

fn check_importance(value: Option<&str>) -> Result<Option<String>> {
    match value.map(str::to_lowercase) {
        None => Ok(None),
        Some(v) if matches!(v.as_str(), "low" | "normal" | "high") => Ok(Some(v)),
        Some(v) => Err(Error::validation(format!("importance '{v}' is not low, normal or high"))),
    }
}

fn check_limit(limit: u32) -> Result<u32> {
    if (1..=MAX_LIMIT).contains(&limit) {
        Ok(limit)
    } else {
        Err(Error::validation(format!("limit must be between 1 and {MAX_LIMIT}, not {limit}")))
    }
}

// ---- output helpers --------------------------------------------------------------------------

/// One message as listed: short and full id, people, flags, preview.
pub fn message_summary(m: &Message) -> Value {
    json!({
        "id": ids::short(&m.id),
        "graph_id": m.id,
        "date": m.date(),
        "from": m.from,
        "to": m.to_recipients,
        "cc": m.cc_recipients,
        "subject": m.subject(),
        "preview": m.body_preview.trim(),
        "unread": !m.is_read,
        "flag": m.flag.flag_status,
        "has_attachments": m.has_attachments,
        "draft": m.is_draft,
        "importance": m.importance,
    })
}

fn attachment_json(a: &Attachment) -> Value {
    json!({
        "name": a.name,
        "size": a.size,
        "content_type": a.content_type,
        "inline": a.is_inline,
        "file": a.is_file(),
    })
}

/// A whole message: summary plus readable text, attachments and (drafts) the author's part.
fn message_detail(m: &Message, attachments: &[Attachment], with_html: bool) -> Value {
    let mut out = message_summary(m);
    out["bcc"] = json!(m.bcc_recipients);
    out["web_link"] = json!(m.web_link);
    let body = m.body.clone().unwrap_or_default();
    out["text"] =
        json!(if body.is_html() { html::to_readable_text(&body.content, TEXT_WIDTH) } else { body.content.clone() });
    if m.is_draft && body.is_html() {
        out["author_text"] = json!(html::author_html(&body.content).map(html::html_to_text));
        out["has_signature"] = json!(html::has_signature(&body.content));
    }
    out["attachments"] = Value::Array(attachments.iter().map(attachment_json).collect());
    if with_html {
        out["html"] = json!(body.content);
    }
    out
}

fn attachments_of(mb: &Mailbox, m: &Message) -> Result<Vec<Attachment>> {
    if m.has_attachments { mb.attachments(&m.id) } else { Ok(vec![]) }
}

fn messages_value(mb: &Mailbox, messages: &[Message]) -> Value {
    ids::remember(mb.id(), messages.iter().map(|m| m.id.as_str()));
    Value::Array(messages.iter().map(message_summary).collect())
}

fn account_json(config: &Config, a: &AccountConfig) -> Value {
    json!({
        "id": a.id,
        "email": a.email,
        "name": a.name,
        "read_only": a.read_only,
        "signed_in": paths::token_file(&a.id).is_file(),
        "signature": !a.signature.trim().is_empty(),
        "default": config.accounts.first().map(|f| f.id == a.id).unwrap_or(false),
    })
}

// ---- accounts --------------------------------------------------------------------------------

pub fn accounts(config: &Config) -> Value {
    json!({ "accounts": config.accounts.iter().map(|a| account_json(config, a)).collect::<Vec<_>>() })
}

/// Device code sign-in. Instructions go to stderr; stdout stays free for the result.
pub fn accounts_add() -> Result<Value> {
    let mut config = Config::load()?;
    let code = auth::start_device_code(config.client_id())?;
    let copied = copy_to_clipboard(&code.user_code);
    eprintln!(
        "Open {} and enter the code {}{}",
        code.verification_uri,
        code.user_code,
        if copied { " (copied)" } else { "" }
    );
    eprintln!("Waiting for the sign-in …");
    let token = auth::wait_device_code(config.client_id(), &code)?;
    let account = auth::register_account(&mut config, token)?;
    Ok(json!({ "action": "added", "account": account_json(&config, &account) }))
}

fn copy_to_clipboard(text: &str) -> bool {
    use std::io::Write as _;
    let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() else { return false };
    let wrote = child.stdin.take().map(|mut s| s.write_all(text.as_bytes()).is_ok()).unwrap_or(false);
    child.wait().map(|s| s.success()).unwrap_or(false) && wrote
}

/// Import every ms365-mail sign-in (default profile and `profiles/*`).
pub fn accounts_import() -> Result<Value> {
    let mut config = Config::load()?;
    let logins = auth::find_ms365_logins(config.client_id());
    if logins.is_empty() {
        return Err(Error::not_found("no ms365-mail sign-ins found")
            .hint("expected token caches in ~/.config/ms365-mail; sign in with `jm accounts add` instead"));
    }
    let mut done: HashSet<String> = HashSet::new();
    let mut imported = Vec::new();
    let mut failed = Vec::new();
    for login in &logins {
        let user = login.username.to_lowercase();
        if done.contains(&user) {
            continue;
        }
        match auth::import_ms365_login(&mut config, login) {
            Ok(account) => {
                done.insert(user);
                done.insert(account.email.to_lowercase());
                imported.push(
                    json!({ "id": account.id, "email": account.email, "name": account.name, "profile": login.profile }),
                );
            }
            Err(e) => failed.push((user, json!({ "username": login.username, "profile": login.profile, "error": e }))),
        }
    }
    // a sign-in that failed in one profile but worked in another is not a failure
    let failed: Vec<Value> = failed.into_iter().filter(|(user, _)| !done.contains(user)).map(|(_, v)| v).collect();
    if imported.is_empty() {
        let first = failed.first().and_then(|f| f.pointer("/error/message")).and_then(Value::as_str).unwrap_or("");
        return Err(Error::auth(format!(
            "none of the {} ms365-mail sign-ins could be imported: {first}",
            failed.len()
        ))
        .hint("sign in with `jm accounts add`"));
    }
    Ok(
        json!({ "action": "imported", "imported": imported, "failed": failed, "accounts": accounts(&config)["accounts"] }),
    )
}

pub fn accounts_remove(key: &str) -> Result<Value> {
    let mut config = Config::load()?;
    let account = config.account(Some(key))?.clone();
    config.accounts.retain(|a| a.id != account.id);
    config.save()?;
    match std::fs::remove_dir_all(paths::account_dir(&account.id)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    Ok(json!({ "action": "removed", "account": { "id": account.id, "email": account.email } }))
}

// ---- reading ---------------------------------------------------------------------------------

pub fn folders(ctx: &Ctx) -> Result<Value> {
    let mb = ctx.mailbox()?;
    let folders: Vec<Value> = mb
        .folders()?
        .iter()
        .map(|f| {
            json!({
                "id": f.id,
                "name": f.display_name,
                "well_known": f.well_known,
                "depth": f.depth,
                "parent_id": f.parent_folder_id,
                "unread": f.unread_item_count,
                "total": f.total_item_count,
            })
        })
        .collect();
    Ok(json!({ "account": mb.id(), "folders": folders }))
}

pub fn list(ctx: &Ctx, p: &ListParams) -> Result<Value> {
    let mb = ctx.mailbox()?;
    let (label, page) = match &p.next {
        Some(next) => {
            if !next.starts_with("https://graph.microsoft.com/") {
                return Err(Error::validation("`next` must be the value of `next` from an earlier listing"));
            }
            (Value::Null, mb.page(next)?)
        }
        None => {
            if p.all && p.folder.is_some() {
                return Err(Error::validation("pick either a folder or the whole mailbox (all), not both"));
            }
            let since = p.since.as_deref().map(|s| parse_since(s, Local::now())).transpose()?;
            let whole = p.all || (p.flagged && p.folder.is_none());
            let name = p.folder.clone().unwrap_or_else(|| "inbox".into());
            let folder = if whole { None } else { Some(mb.resolve_folder(&name)?) };
            let query = ListQuery {
                folder,
                top: check_limit(p.limit)?,
                unread: p.unread,
                flagged: p.flagged,
                has_attachments: p.attachments,
                from: p.from.clone().filter(|f| !f.trim().is_empty()),
                since,
            };
            (json!(if whole { "all".to_string() } else { name }), mb.list(&query)?)
        }
    };
    Ok(json!({
        "account": mb.id(),
        "folder": label,
        "count": page.messages.len(),
        "messages": messages_value(&mb, &page.messages),
        "next": page.next_link,
    }))
}

pub fn search(ctx: &Ctx, query: &str, folder: Option<&str>, limit: u32) -> Result<Value> {
    if query.trim().is_empty() {
        return Err(Error::validation("the search query is empty"));
    }
    let mb = ctx.mailbox()?;
    let folder = folder.map(|f| mb.resolve_folder(f)).transpose()?;
    let messages = mb.search(query, folder.as_deref(), check_limit(limit)?)?;
    Ok(json!({
        "account": mb.id(),
        "query": query,
        "count": messages.len(),
        "messages": messages_value(&mb, &messages),
    }))
}

/// One message. `raw` returns the Graph object as is; `html` adds the HTML body.
pub fn show(ctx: &Ctx, id: &str, with_html: bool, raw: bool) -> Result<Value> {
    let (mb, id) = ctx.target(id)?;
    let m = mb.message(&id)?;
    ids::remember(mb.id(), [m.id.as_str()]);
    if raw {
        return Ok(json!({ "account": mb.id(), "id": ids::short(&m.id), "raw": m }));
    }
    let attachments = attachments_of(&mb, &m)?;
    Ok(json!({ "account": mb.id(), "message": message_detail(&m, &attachments, with_html) }))
}

pub fn attachments(ctx: &Ctx, id: &str) -> Result<Value> {
    let (mb, id) = ctx.target(id)?;
    let list = mb.attachments(&id)?;
    Ok(json!({
        "account": mb.id(),
        "id": ids::short(&id),
        "attachments": list.iter().map(attachment_json).collect::<Vec<_>>(),
    }))
}

/// Save attachments (all real files, or the named ones) into `dir` (default ~/Downloads).
pub fn download(ctx: &Ctx, id: &str, names: &[String], dir: Option<&Path>) -> Result<Value> {
    let (mb, id) = ctx.target(id)?;
    let all = mb.attachments(&id)?;
    let picked: Vec<&Attachment> = if names.is_empty() {
        // inline images are logos and signature pictures
        let is_inline_image =
            |a: &Attachment| a.is_inline && a.content_type.as_deref().unwrap_or("").starts_with("image/");
        all.iter().filter(|a| a.is_file() && !is_inline_image(a)).collect()
    } else {
        names
            .iter()
            .map(|name| {
                let found = all.iter().find(|a| a.name.eq_ignore_ascii_case(name.trim())).ok_or_else(|| {
                    let known: Vec<&str> = all.iter().map(|a| a.name.as_str()).collect();
                    Error::not_found(format!("no attachment named '{name}'")).hint(format!(
                        "attachments: {}",
                        if known.is_empty() { "none".into() } else { known.join(", ") }
                    ))
                })?;
                if !found.is_file() {
                    return Err(Error::validation(format!("'{name}' is an attached item (mail or event), not a file")));
                }
                Ok(found)
            })
            .collect::<Result<_>>()?
    };
    if picked.is_empty() {
        return Err(Error::not_found("this message has no file attachments")
            .hint("`jm attachments <id>` lists what is attached"));
    }
    let dir = match dir {
        Some(d) => expand_home(d),
        None => PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into())).join("Downloads"),
    };
    std::fs::create_dir_all(&dir)
        .map_err(|e| Error::validation(format!("cannot use {} as download folder: {e}", dir.display())))?;
    let mut saved = Vec::new();
    for a in picked {
        let bytes = mb.attachment_bytes(&id, &a.id)?;
        let path = free_path(&dir, &safe_file_name(&a.name));
        std::fs::write(&path, &bytes)?;
        saved.push(json!({ "name": a.name, "path": path, "size": bytes.len() }));
    }
    Ok(json!({ "account": mb.id(), "id": ids::short(&id), "saved": saved }))
}

/// A file name without path separators or control characters.
pub fn safe_file_name(name: &str) -> String {
    let cleaned: String =
        name.chars().map(|c| if c.is_control() || matches!(c, '/' | '\\' | ':') { '_' } else { c }).collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim().to_string();
    if cleaned.is_empty() { "attachment".into() } else { cleaned }
}

/// `dir/name`, or `dir/name (2).ext`, … when taken.
pub fn free_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (2..).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).expect("some free name")
}

// ---- changing received mail ------------------------------------------------------------------

pub fn set_flag(ctx: &Ctx, id: &str, status: FlagStatus) -> Result<Value> {
    let (mb, id) = ctx.target(id)?;
    let m = mb.set_flag(&id, status)?;
    changed(&mb, None);
    Ok(json!({ "account": mb.id(), "action": "flag", "message": message_summary(&m) }))
}

pub fn mark_read(ctx: &Ctx, id: &str, read: bool) -> Result<Value> {
    let (mb, id) = ctx.target(id)?;
    let m = mb.set_read(&id, read)?;
    changed(&mb, None);
    Ok(json!({ "account": mb.id(), "action": if read { "read" } else { "unread" }, "message": message_summary(&m) }))
}

/// Move to `archive`, `trash` or any folder (name or id).
pub fn move_message(ctx: &Ctx, id: &str, to: &str) -> Result<Value> {
    let (mb, id) = ctx.target(id)?;
    let folder = mb.resolve_folder(to)?;
    let m = mb.move_to(&id, &folder)?;
    ids::remember(mb.id(), [m.id.as_str()]);
    changed(&mb, None);
    Ok(json!({ "account": mb.id(), "action": "move", "folder": to, "message": message_summary(&m) }))
}

// ---- drafts ----------------------------------------------------------------------------------

pub fn list_drafts(ctx: &Ctx, limit: u32) -> Result<Value> {
    list(ctx, &ListParams { folder: Some("drafts".into()), limit, ..Default::default() })
}

fn draft_result(mb: &Mailbox, action: &str, m: &Message) -> Result<Value> {
    ids::remember(mb.id(), [m.id.as_str()]);
    changed(mb, Some("drafts"));
    let attachments = attachments_of(mb, m)?;
    Ok(json!({ "account": mb.id(), "action": action, "draft": message_detail(m, &attachments, false) }))
}

/// Attach files to a draft that already exists; a failure says which draft is left.
fn attach_all(mb: &Mailbox, draft_id: &str, files: &[NewAttachment]) -> Result<()> {
    for file in files {
        mb.add_attachment(draft_id, file).map_err(|e| {
            let hint = format!("draft {} exists without {}", ids::short(draft_id), file.name);
            Error { hint: Some(hint), ..e }
        })?;
    }
    Ok(())
}

pub fn create_draft(ctx: &Ctx, p: &DraftParams) -> Result<Value> {
    let to = parse_recipients(&p.to)?
        .filter(|to| !to.is_empty())
        .ok_or_else(|| Error::validation("a new draft needs at least one recipient (to)"))?;
    let subject = p.subject.clone().filter(|s| !s.trim().is_empty());
    let subject = subject.ok_or_else(|| Error::validation("a new draft needs a subject"))?;
    let body = body_html(p.body.as_deref(), p.body_file.as_deref(), p.html)?
        .ok_or_else(|| Error::validation("a new draft needs a body").hint("pass --body TEXT or --body-file PATH"))?;
    let input = DraftInput {
        to: Some(to),
        cc: parse_recipients(&p.cc)?,
        bcc: parse_recipients(&p.bcc)?,
        subject: Some(subject),
        body_html: Some(body),
        importance: check_importance(p.importance.as_deref())?,
    };
    let files = load_files(&p.attachments)?;
    let mb = ctx.mailbox()?;
    let draft = mb.create_draft(&input, p.signature, &files)?;
    draft_result(&mb, "created", &draft)
}

pub fn reply_draft(ctx: &Ctx, id: &str, all: bool, p: &DraftParams) -> Result<Value> {
    let body = body_html(p.body.as_deref(), p.body_file.as_deref(), p.html)?
        .ok_or_else(|| Error::validation("a reply needs a body").hint("pass --body TEXT or --body-file PATH"))?;
    let files = load_files(&p.attachments)?;
    let (mb, id) = ctx.target(id)?;
    let draft = mb.create_reply(&id, all, &body, p.signature)?;
    attach_all(&mb, &draft.id, &files)?;
    let draft = if files.is_empty() { draft } else { mb.message(&draft.id)? };
    draft_result(&mb, "created", &draft)
}

pub fn forward_draft(ctx: &Ctx, id: &str, p: &DraftParams) -> Result<Value> {
    let to = parse_recipients(&p.to)?
        .filter(|to| !to.is_empty())
        .ok_or_else(|| Error::validation("a forward needs at least one recipient (to)"))?;
    let body = body_html(p.body.as_deref(), p.body_file.as_deref(), p.html)?.unwrap_or_default();
    let files = load_files(&p.attachments)?;
    let (mb, id) = ctx.target(id)?;
    let draft = mb.create_forward(&id, &to, &body, p.signature)?;
    attach_all(&mb, &draft.id, &files)?;
    let draft = if files.is_empty() { draft } else { mb.message(&draft.id)? };
    draft_result(&mb, "created", &draft)
}

/// Change a draft. A new body replaces only the author's part (signature and quote stay).
pub fn update_draft(ctx: &Ctx, id: &str, p: &DraftParams) -> Result<Value> {
    let input = DraftInput {
        to: parse_recipients(&p.to)?,
        cc: parse_recipients(&p.cc)?,
        bcc: parse_recipients(&p.bcc)?,
        subject: p.subject.clone(),
        body_html: body_html(p.body.as_deref(), p.body_file.as_deref(), p.html)?,
        importance: check_importance(p.importance.as_deref())?,
    };
    let files = load_files(&p.attachments)?;
    let has_changes = input.to.is_some()
        || input.cc.is_some()
        || input.bcc.is_some()
        || input.subject.is_some()
        || input.body_html.is_some()
        || input.importance.is_some();
    if !has_changes && files.is_empty() {
        return Err(Error::validation("nothing to change")
            .hint("pass --to, --cc, --bcc, --subject, --body/--body-file, --importance or --attach"));
    }
    let (mb, id) = ctx.target(id)?;
    // received mail must never be edited, whatever is being changed
    let current = mb.message(&id)?;
    if !current.is_draft {
        return Err(Error::validation("this message is not a draft").hint("only drafts can be changed"));
    }
    let mut draft = if has_changes { mb.update_draft(&id, &input)? } else { current };
    if !files.is_empty() {
        attach_all(&mb, &id, &files)?;
        draft = mb.message(&id)?;
    }
    draft_result(&mb, "updated", &draft)
}

pub fn delete_draft(ctx: &Ctx, id: &str) -> Result<Value> {
    let (mb, id) = ctx.target(id)?;
    mb.delete_draft(&id).map_err(|e| {
        // a deleted draft still resolves (it sits in Recoverable Items) but refuses a second delete
        if e.message.contains("ErrorCannotDeleteObject") {
            Error::not_found("this draft is already deleted").hint("`jm drafts list` shows the drafts")
        } else {
            e
        }
    })?;
    changed(&mb, Some("drafts"));
    Ok(json!({ "account": mb.id(), "action": "deleted", "id": ids::short(&id), "graph_id": id }))
}

// ---- the app ---------------------------------------------------------------------------------

fn app_running() -> bool {
    ["just-mail", "Just Mail"].iter().any(|name| {
        Command::new("pgrep")
            .args(["-x", name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// Ask Just Mail to show a message, starting it first when it isn't running.
pub fn open(ctx: &Ctx, id: &str) -> Result<Value> {
    let (mb, id) = ctx.target(id)?;
    let was_running = app_running();
    let mut note = None;
    if !was_running {
        let started = Command::new("open")
            .args(["-b", APP_BUNDLE_ID])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if started {
            // the app only reads events written after it started
            for _ in 0..20 {
                if app_running() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            std::thread::sleep(Duration::from_secs(2));
            note = Some("Just Mail was not running and has been started".to_string());
        } else {
            note = Some(format!("Just Mail is not running and could not be started (bundle {APP_BUNDLE_ID})"));
        }
    }
    events::emit(&Event::Open { account: mb.id().to_string(), id: id.clone() });
    Ok(
        json!({ "account": mb.id(), "action": "open", "id": ids::short(&id), "app_was_running": was_running, "note": note }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 10, 8, 15, 30, 0).unwrap()
    }

    #[test]
    fn since_accepts_spans() {
        let n = now();
        assert_eq!(parse_since("36h", n).unwrap(), (n - chrono::Duration::hours(36)).with_timezone(&Utc));
        assert_eq!(parse_since("7d", n).unwrap(), (n - chrono::Duration::days(7)).with_timezone(&Utc));
        assert_eq!(parse_since(" 2W ", n).unwrap(), (n - chrono::Duration::weeks(2)).with_timezone(&Utc));
    }

    #[test]
    fn since_accepts_dates_and_times() {
        let n = now();
        let midnight = Local.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap().with_timezone(&Utc);
        assert_eq!(parse_since("2026-10-01", n).unwrap(), midnight);
        let today = Local.with_ymd_and_hms(2026, 10, 8, 0, 0, 0).unwrap().with_timezone(&Utc);
        assert_eq!(parse_since("today", n).unwrap(), today);
        assert_eq!(parse_since("yesterday", n).unwrap(), today - chrono::Duration::days(1));
        assert_eq!(
            parse_since("2026-10-01T14:00:00Z", n).unwrap(),
            Utc.with_ymd_and_hms(2026, 10, 1, 14, 0, 0).unwrap()
        );
    }

    #[test]
    fn since_rejects_nonsense() {
        for bad in ["", "d", "-3d", "7x", "last week", "2026-13-01"] {
            let err = parse_since(bad, now()).unwrap_err();
            assert_eq!(err.code, jm_core::ErrorCode::Validation, "{bad}");
        }
    }

    #[test]
    fn recipients_come_from_repeated_and_listed_values() {
        assert_eq!(parse_recipients(&[]).unwrap(), None);
        let list = parse_recipients(&["a@b.de, Max <max@x.de>".into(), "c@d.de".into()]).unwrap().unwrap();
        let addresses: Vec<&str> = list.iter().map(|r| r.address.as_str()).collect();
        assert_eq!(addresses, ["a@b.de", "max@x.de", "c@d.de"]);
        assert_eq!(list[1].name, "Max");
        assert!(parse_recipients(&["nobody".into()]).is_err());
    }

    #[test]
    fn plain_text_bodies_become_html_and_html_stays() {
        let text = body_html(Some("Hallo <Martin>,\n\nGrüße"), None, false).unwrap().unwrap();
        assert_eq!(text, "Hallo &lt;Martin&gt;,<br>\n<br>\nGrüße");
        let raw = body_html(Some("<b>fett</b>"), None, true).unwrap().unwrap();
        assert_eq!(raw, "<b>fett</b>");
        assert_eq!(body_html(None, None, false).unwrap(), None);
        assert!(body_html(Some("a"), Some(Path::new("b")), false).is_err());
        let missing = body_html(None, Some(Path::new("/nonexistent/jm-body.txt")), false).unwrap_err();
        assert_eq!(missing.code, jm_core::ErrorCode::NotFound);
    }

    #[test]
    fn body_and_signature_assemble_into_marked_blocks() {
        let body = body_html(Some("Hi\nthere"), None, false).unwrap().unwrap();
        let doc = html::new_document(&body, "Marc Neuhaus\nRoothirsch GmbH");
        assert_eq!(html::author_html(&doc).map(html::html_to_text).as_deref(), Some("Hi\nthere"));
        assert!(html::has_signature(&doc));
        assert!(doc.find("jm-body").unwrap() < doc.find("Roothirsch GmbH").unwrap());
    }

    #[test]
    fn importance_and_limits_are_checked() {
        assert_eq!(check_importance(Some("HIGH")).unwrap().as_deref(), Some("high"));
        assert!(check_importance(Some("urgent")).is_err());
        assert!(check_limit(0).is_err() && check_limit(1001).is_err() && check_limit(25).is_ok());
    }

    #[test]
    fn download_names_are_safe_and_free() {
        assert_eq!(safe_file_name("../a/b:c.pdf"), "_a_b_c.pdf");
        assert_eq!(safe_file_name("  "), "attachment");
        let dir = std::env::temp_dir().join(format!("jm-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("file.pdf"), b"x").unwrap();
        assert_eq!(free_path(&dir, "file.pdf"), dir.join("file (2).pdf"));
        assert_eq!(free_path(&dir, "new.pdf"), dir.join("new.pdf"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
