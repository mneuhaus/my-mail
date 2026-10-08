//! Graph mail objects. They read Graph's camelCase JSON and write the same names, except
//! recipients, which come out flat as `{name, address}`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "GraphRecipient")]
pub struct Recipient {
    pub name: String,
    pub address: String,
}

#[derive(Deserialize)]
struct GraphRecipient {
    #[serde(rename = "emailAddress")]
    email_address: GraphAddress,
}

#[derive(Deserialize)]
struct GraphAddress {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    address: Option<String>,
}

impl From<GraphRecipient> for Recipient {
    fn from(r: GraphRecipient) -> Self {
        Recipient { name: r.email_address.name.unwrap_or_default(), address: r.email_address.address.unwrap_or_default() }
    }
}

impl Recipient {
    pub fn new(address: impl Into<String>) -> Self {
        Recipient { name: String::new(), address: address.into() }
    }

    /// Name if there is one, else the address.
    pub fn display(&self) -> &str {
        if self.name.trim().is_empty() { &self.address } else { &self.name }
    }

    /// `Name <address>` or just the address.
    pub fn full(&self) -> String {
        if self.name.trim().is_empty() || self.name == self.address {
            self.address.clone()
        } else {
            format!("{} <{}>", self.name, self.address)
        }
    }

    pub fn to_graph(&self) -> serde_json::Value {
        if self.name.is_empty() {
            serde_json::json!({ "emailAddress": { "address": self.address } })
        } else {
            serde_json::json!({ "emailAddress": { "name": self.name, "address": self.address } })
        }
    }

    /// Parse `a@b.c, Name <d@e.f>; g@h.i` into recipients. Rejects anything without an `@`.
    pub fn parse_list(input: &str) -> crate::Result<Vec<Recipient>> {
        let mut out = Vec::new();
        for part in input.split([',', ';', '\n']) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let (name, address) = match (part.rfind('<'), part.rfind('>')) {
                (Some(open), Some(close)) if open < close => {
                    (part[..open].trim().trim_matches('"').to_string(), part[open + 1..close].trim().to_string())
                }
                _ => (String::new(), part.to_string()),
            };
            if !address.contains('@') || address.contains(' ') {
                return Err(crate::Error::validation(format!("'{part}' is not a mail address")));
            }
            out.push(Recipient { name, address });
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FlagStatus {
    #[default]
    NotFlagged,
    Flagged,
    Complete,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Flag {
    #[serde(default)]
    pub flag_status: FlagStatus,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Body {
    #[serde(default)]
    pub content_type: String,
    #[serde(default)]
    pub content: String,
}

impl Body {
    pub fn is_html(&self) -> bool {
        self.content_type.eq_ignore_ascii_case("html")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub body_preview: String,
    #[serde(default)]
    pub is_read: bool,
    #[serde(default)]
    pub is_draft: bool,
    #[serde(default)]
    pub has_attachments: bool,
    #[serde(default)]
    pub importance: Option<String>,
    #[serde(default)]
    pub received_date_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub sent_date_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_modified_date_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub parent_folder_id: Option<String>,
    #[serde(default)]
    pub web_link: Option<String>,
    #[serde(default)]
    pub flag: Flag,
    #[serde(default)]
    pub from: Option<Recipient>,
    #[serde(default)]
    pub to_recipients: Vec<Recipient>,
    #[serde(default)]
    pub cc_recipients: Vec<Recipient>,
    #[serde(default)]
    pub bcc_recipients: Vec<Recipient>,
    #[serde(default)]
    pub body: Option<Body>,
    /// Only what is new in this message, without the quoted mail (asked for in conversations).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unique_body: Option<Body>,
}

impl Message {
    pub fn subject(&self) -> &str {
        self.subject.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or("(no subject)")
    }

    pub fn is_flagged(&self) -> bool {
        self.flag.flag_status == FlagStatus::Flagged
    }

    /// When the mail arrived (drafts: last change).
    pub fn date(&self) -> Option<DateTime<Utc>> {
        if self.is_draft {
            self.last_modified_date_time.or(self.received_date_time)
        } else {
            self.received_date_time.or(self.sent_date_time)
        }
    }

    pub fn sender(&self) -> String {
        self.from.as_ref().map(|r| r.display().to_string()).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub parent_folder_id: Option<String>,
    #[serde(default)]
    pub child_folder_count: u32,
    #[serde(default)]
    pub unread_item_count: u32,
    #[serde(default)]
    pub total_item_count: u32,
    /// `inbox`, `drafts`, `sentitems`, `archive`, `deleteditems`, `junkemail` (filled in by us).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub well_known: Option<String>,
    /// Nesting level below the mailbox root (filled in by us).
    #[serde(default)]
    pub depth: u32,
}

/// Well-known folders in the order the sidebar shows them.
pub const WELL_KNOWN: &[&str] = &["inbox", "drafts", "sentitems", "archive", "junkemail", "deleteditems"];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub is_inline: bool,
    #[serde(rename = "@odata.type", default)]
    pub odata_type: String,
}

impl Attachment {
    /// Only file attachments can be downloaded as bytes (not item or reference attachments).
    pub fn is_file(&self) -> bool {
        self.odata_type.is_empty() || self.odata_type.ends_with("fileAttachment")
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub mail: Option<String>,
    #[serde(default)]
    pub user_principal_name: Option<String>,
}

impl Me {
    pub fn email(&self) -> String {
        self.mail.clone().or_else(|| self.user_principal_name.clone()).unwrap_or_default().to_lowercase()
    }
}

/// What to put into a new or changed draft. `None` fields stay as they are on update.
#[derive(Debug, Clone, Default)]
pub struct DraftInput {
    pub to: Option<Vec<Recipient>>,
    pub cc: Option<Vec<Recipient>>,
    pub bcc: Option<Vec<Recipient>>,
    pub subject: Option<String>,
    /// HTML of the part the author wrote (signature and quote are kept separately).
    pub body_html: Option<String>,
    pub importance: Option<String>,
}

/// A local file to attach.
#[derive(Debug, Clone)]
pub struct NewAttachment {
    pub name: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

impl NewAttachment {
    pub fn from_path(path: &std::path::Path) -> crate::Result<Self> {
        let bytes = std::fs::read(path).map_err(|e| crate::Error::not_found(format!("{}: {e}", path.display())))?;
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "attachment".into());
        Ok(NewAttachment { content_type: content_type_for(&name).to_string(), name, bytes })
    }
}

pub fn content_type_for(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or_default().to_lowercase();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "xml" => "application/xml",
        "json" => "application/json",
        "zip" => "application/zip",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "ics" => "text/calendar",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_recipient_lists() {
        let list = Recipient::parse_list("a@b.de, \"Max M\" <max@x.de>; c@d.de").unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[1], Recipient { name: "Max M".into(), address: "max@x.de".into() });
        assert!(Recipient::parse_list("not an address").is_err());
    }

    #[test]
    fn reads_graph_messages() {
        let json = r#"{"id":"A","subject":null,"from":{"emailAddress":{"name":"X","address":"x@y.z"}},
            "flag":{"flagStatus":"flagged"},"receivedDateTime":"2026-10-07T15:20:00Z"}"#;
        let m: Message = serde_json::from_str(json).unwrap();
        assert_eq!(m.subject(), "(no subject)");
        assert!(m.is_flagged());
        assert_eq!(m.sender(), "X");
        assert_eq!(serde_json::to_value(&m.from).unwrap(), serde_json::json!({"name":"X","address":"x@y.z"}));
    }
}
