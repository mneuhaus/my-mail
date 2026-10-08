//! Signatures from Spark (Readdle), Marc's previous mail client.
//!
//! Spark keeps them in a SQLite settings database, table `settings(itemGroup, itemKey,
//! itemValue)`: every signature under `SignaturesSettingsItemsGroup` (JSON with `htmlContent`),
//! and which address uses which one under `MetaSettingsItemsGroup` / `MetaBindingSettingsKey`
//! (`{"address": "<signature id>"}`). Spark keeps the database open in WAL mode, so it is copied
//! (with its `-wal` and `-shm` files) and the copy is read with the system `sqlite3`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::config::Config;
use crate::html;
use crate::error::{Error, Result};

const SQLITE: &str = "/usr/bin/sqlite3";
const SIGNATURES_GROUP: &str = "SignaturesSettingsItemsGroup";
const META_GROUP: &str = "MetaSettingsItemsGroup";
const BINDING_KEY: &str = "MetaBindingSettingsKey";

/// One Spark signature. A signature bound to several addresses is listed once per address;
/// one bound to none has no `email`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SparkSignature {
    pub email: Option<String>,
    pub id: String,
    pub html: String,
}

/// Settings databases of Spark Classic and the new Spark Mail, those that exist.
fn databases() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else { return vec![] };
    let library = home.join("Library");
    [
        library.join("Group Containers/3L68KQB4HG.group.com.readdle.smartemail/databases/settings.sqlite"),
        library.join("Application Support/Spark Mail/core-data/settings.sqlite"),
    ]
    .into_iter()
    .filter(|p| p.is_file())
    .collect()
}

/// All signatures (not deleted) of every Spark installation found, bound addresses filled in.
pub fn find_signatures() -> Result<Vec<SparkSignature>> {
    let dbs = databases();
    if dbs.is_empty() {
        return Err(Error::not_found("no Spark settings database found")
            .hint("expected Spark in ~/Library/Group Containers/…readdle.smartemail or ~/Library/Application Support/Spark Mail"));
    }
    let mut found: Vec<SparkSignature> = Vec::new();
    for db in dbs {
        for signature in signatures_in(&read_rows(&db)?) {
            if !found.contains(&signature) {
                found.push(signature);
            }
        }
    }
    Ok(found)
}

/// Set the bound Spark signature as HTML signature of every configured account whose address
/// has one (case-insensitive) and save the config. Returns the ids of the accounts that changed;
/// accounts without a binding stay as they are.
pub fn import_signatures(config: &mut Config) -> Result<Vec<String>> {
    let changed = apply_bindings(config, &find_signatures()?);
    if !changed.is_empty() {
        config.save()?;
    }
    Ok(changed)
}

/// Give each account the signature bound to its address; the ids of the accounts that changed.
fn apply_bindings(config: &mut Config, signatures: &[SparkSignature]) -> Vec<String> {
    let mut changed = Vec::new();
    for account in &mut config.accounts {
        let bound = signatures
            .iter()
            .find(|s| s.email.as_deref().is_some_and(|e| e.eq_ignore_ascii_case(account.email.trim())));
        let Some(signature) = bound else { continue };
        // Spark only records the signature for new mails; the short one Marc uses for replies is
        // the shortest unbound signature, when it really is shorter.
        let length = |s: &SparkSignature| html::html_to_text(&s.html).chars().count();
        let reply = signatures
            .iter()
            .filter(|s| s.email.is_none() && s.id != signature.id && length(s) < length(signature))
            .min_by_key(|s| length(s))
            .map(|s| s.html.clone())
            .unwrap_or_default();
        if account.signature_html != signature.html || account.signature_reply_html != reply {
            account.signature_html = signature.html.clone();
            account.signature_reply_html = reply;
            changed.push(account.id.clone());
        }
    }
    changed
}

/// `(itemGroup, itemKey, itemValue)` of the signature and binding rows, read from a copy.
fn read_rows(db: &Path) -> Result<Vec<(String, String, String)>> {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("jm-spark-{}-{stamp:x}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let result = (|| {
        let copy = dir.join("settings.sqlite");
        std::fs::copy(db, &copy)?;
        for suffix in ["-wal", "-shm"] {
            let side = PathBuf::from(format!("{}{suffix}", db.display()));
            if side.is_file() {
                std::fs::copy(&side, dir.join(format!("settings.sqlite{suffix}")))?;
            }
        }
        query(&copy)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result.map_err(|e| Error { message: format!("reading Spark's {}: {}", db.display(), e.message), ..e })
}

fn query(db: &Path) -> Result<Vec<(String, String, String)>> {
    let sql = format!(
        "select itemGroup, itemKey, cast(itemValue as text) as value from settings \
         where itemGroup = '{SIGNATURES_GROUP}' or (itemGroup = '{META_GROUP}' and itemKey = '{BINDING_KEY}')"
    );
    let out = Command::new(SQLITE)
        .arg("-json")
        .arg(db)
        .arg(sql)
        .output()
        .map_err(|e| Error::general(format!("cannot run {SQLITE}: {e}")))?;
    if !out.status.success() {
        return Err(Error::general(String::from_utf8_lossy(&out.stderr).trim().to_string()));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // sqlite3 prints nothing at all for an empty result
    if text.trim().is_empty() {
        return Ok(vec![]);
    }
    let rows: Vec<Value> = serde_json::from_str(&text)?;
    let field = |row: &Value, name: &str| row.get(name).and_then(Value::as_str).unwrap_or_default().to_string();
    Ok(rows.iter().map(|r| (field(r, "itemGroup"), field(r, "itemKey"), field(r, "value"))).collect())
}

/// Signatures from settings rows, with the addresses bound to them.
fn signatures_in(rows: &[(String, String, String)]) -> Vec<SparkSignature> {
    let bindings: BTreeMap<String, String> = rows
        .iter()
        .find(|(group, key, _)| group == META_GROUP && key == BINDING_KEY)
        .and_then(|(_, _, value)| serde_json::from_str(value).ok())
        .unwrap_or_default();
    let mut out = Vec::new();
    for (_, key, value) in rows.iter().filter(|(group, _, _)| group == SIGNATURES_GROUP) {
        let Ok(json) = serde_json::from_str::<Value>(value) else { continue };
        if json.get("deleted").and_then(Value::as_bool).unwrap_or(false) {
            continue;
        }
        let html = json.get("htmlContent").and_then(Value::as_str).unwrap_or_default().trim();
        if html.is_empty() {
            continue;
        }
        let id = json.get("identifier").and_then(Value::as_str).unwrap_or(key).to_string();
        let emails: Vec<&String> = bindings.iter().filter(|(_, bound)| **bound == id).map(|(email, _)| email).collect();
        let signature = |email: Option<&String>| SparkSignature {
            email: email.map(|e| e.trim().to_string()),
            id: id.clone(),
            html: html.to_string(),
        };
        if emails.is_empty() {
            out.push(signature(None));
        } else {
            out.extend(emails.into_iter().map(|e| signature(Some(e))));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(group: &str, key: &str, value: &str) -> (String, String, String) {
        (group.into(), key.into(), value.into())
    }

    #[test]
    fn signatures_get_their_bound_addresses_and_deleted_ones_go() {
        let rows = vec![
            row(META_GROUP, BINDING_KEY, r#"{"marc@roothirsch.com":"A","info@roothirsch.com":"A","x@y.de":"GONE"}"#),
            row(SIGNATURES_GROUP, "A", r#"{"identifier":"A","deleted":false,"htmlContent":"<div>Marc</div>"}"#),
            row(SIGNATURES_GROUP, "B", r#"{"identifier":"B","htmlContent":"<div>other</div>"}"#),
            row(SIGNATURES_GROUP, "GONE", r#"{"identifier":"GONE","deleted":true,"htmlContent":"<div>old</div>"}"#),
            row(SIGNATURES_GROUP, "BROKEN", "not json"),
        ];
        let found = signatures_in(&rows);
        let summary: Vec<(Option<&str>, &str)> = found.iter().map(|s| (s.email.as_deref(), s.id.as_str())).collect();
        assert_eq!(summary, [(Some("info@roothirsch.com"), "A"), (Some("marc@roothirsch.com"), "A"), (None, "B")]);
        assert_eq!(found[1].html, "<div>Marc</div>");
    }

    #[test]
    fn no_rows_no_signatures() {
        assert!(signatures_in(&[]).is_empty());
    }

    #[test]
    fn only_bound_accounts_change() {
        let account = |id: &str, email: &str| crate::AccountConfig {
            id: id.into(),
            email: email.into(),
            name: String::new(),
            signature: "text fallback".into(),
            signature_html: String::new(),
            signature_reply_html: String::new(),
            read_only: false,
        };
        let mut config = Config {
            accounts: vec![account("marc", "Marc@Roothirsch.com"), account("invoice", "invoice@roothirsch.com")],
            ..Default::default()
        };
        let signatures = [
            SparkSignature { email: Some("marc@roothirsch.com".into()), id: "A".into(), html: "<div>Marc</div>".into() },
            SparkSignature { email: None, id: "B".into(), html: "<div>unbound</div>".into() },
        ];
        assert_eq!(apply_bindings(&mut config, &signatures), ["marc"]);
        assert_eq!(config.accounts[0].signature_html, "<div>Marc</div>");
        assert_eq!(config.accounts[0].signature, "text fallback");
        assert_eq!(config.accounts[1].signature_html, "");
        // a second import changes nothing
        assert!(apply_bindings(&mut config, &signatures).is_empty());
        // the longer unbound signature is no reply signature
        assert_eq!(config.accounts[0].signature_reply_html, "");
    }

    #[test]
    fn the_short_unbound_signature_answers() {
        let mut config = Config {
            accounts: vec![crate::AccountConfig {
                id: "marc".into(),
                email: "marc@roothirsch.com".into(),
                name: String::new(),
                signature: String::new(),
                signature_html: String::new(),
                signature_reply_html: String::new(),
                read_only: false,
            }],
            ..Default::default()
        };
        let signatures = [
            SparkSignature { email: Some("marc@roothirsch.com".into()), id: "LONG".into(), html: "<div>Grüße, Adresse, DSGVO</div>".into() },
            SparkSignature { email: None, id: "SHORT".into(), html: "<div>Grüße, AGB</div>".into() },
            SparkSignature { email: None, id: "SHORTEST".into(), html: "<div>Gr</div>".into() },
        ];
        assert_eq!(apply_bindings(&mut config, &signatures), ["marc"]);
        assert_eq!(config.accounts[0].signature_html, "<div>Grüße, Adresse, DSGVO</div>");
        assert_eq!(config.accounts[0].signature_reply_html, "<div>Gr</div>");
    }
}
