//! `config.toml`: the accounts and their signatures, plus a few app settings.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::{html, paths};

/// Microsoft app registration used for sign-in (the one `ms365-mail` uses, so its
/// sign-ins can be imported without asking for consent again).
pub const DEFAULT_CLIENT_ID: &str = "67c15088-6286-452b-8816-5241d2a7d1b9";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Show remote images in mails (off: tracking pixels stay silent).
    #[serde(default)]
    pub load_remote_images: bool,
    #[serde(default, rename = "account")]
    pub accounts: Vec<AccountConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AccountConfig {
    /// Short handle used by the CLI (`-a marc`) and as directory name.
    pub id: String,
    pub email: String,
    #[serde(default)]
    pub name: String,
    /// Plain text appended to new mails, replies and forwards (when there is no HTML signature).
    #[serde(default)]
    pub signature: String,
    /// HTML signature (imported from Spark); wins over the text `signature` when set. Images may
    /// be `data:` URIs: drafts get them as inline attachments.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub signature_html: String,
    /// Look, don't touch: no marking as read, flags, moves, deletes or drafts. For mailboxes
    /// another system processes (invoice@ is read by webIC).
    #[serde(default)]
    pub read_only: bool,
}

impl AccountConfig {
    /// The signature as HTML: the HTML signature, else the text one converted, `None` when the
    /// account has neither.
    pub fn signature_block(&self) -> Option<String> {
        if !self.signature_html.trim().is_empty() {
            Some(self.signature_html.trim().to_string())
        } else if !self.signature.trim().is_empty() {
            Some(html::text_to_html(self.signature.trim_end()))
        } else {
            None
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = paths::config_file();
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|e| Error::validation(format!("{} is not valid: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self) -> Result<()> {
        let text = toml::to_string_pretty(self).map_err(|e| Error::general(e.to_string()))?;
        paths::write_private(&paths::config_file(), text.as_bytes())?;
        Ok(())
    }

    pub fn client_id(&self) -> &str {
        self.client_id.as_deref().unwrap_or(DEFAULT_CLIENT_ID)
    }

    /// Find an account by id or email (case-insensitive). `None` picks the first account.
    pub fn account(&self, key: Option<&str>) -> Result<&AccountConfig> {
        let Some(key) = key.filter(|k| !k.is_empty()) else {
            return self.accounts.first().ok_or_else(|| {
                Error::auth("no account configured")
                    .hint("sign in with `jm accounts add` or import ms365-mail sign-ins with `jm accounts import`")
            });
        };
        let key = key.to_lowercase();
        self.accounts
            .iter()
            .find(|a| a.id.to_lowercase() == key || a.email.to_lowercase() == key)
            .ok_or_else(|| {
                let known: Vec<_> = self.accounts.iter().map(|a| a.id.as_str()).collect();
                Error::not_found(format!("unknown account '{key}'")).hint(format!("known accounts: {}", known.join(", ")))
            })
    }

    /// Add an account, or refresh name and address of a known one (matched by email; its id,
    /// signatures and read-only setting stay).
    pub fn upsert(&mut self, account: AccountConfig) {
        match self.accounts.iter_mut().find(|a| a.email.eq_ignore_ascii_case(&account.email)) {
            Some(existing) => {
                existing.email = account.email;
                if !account.name.is_empty() {
                    existing.name = account.name;
                }
            }
            None => self.accounts.push(account),
        }
    }

    /// A free account id derived from the mail address (`marc@roothirsch.com` → `marc`).
    pub fn new_account_id(&self, email: &str) -> String {
        if let Some(existing) = self.accounts.iter().find(|a| a.email.eq_ignore_ascii_case(email)) {
            return existing.id.clone();
        }
        let base: String = email
            .split('@')
            .next()
            .unwrap_or("account")
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '-' })
            .collect();
        let base = if base.is_empty() { "account".to_string() } else { base };
        let mut id = base.clone();
        let mut n = 2;
        while self.accounts.iter().any(|a| a.id == id) {
            id = format!("{base}{n}");
            n += 1;
        }
        id
    }
}
