//! Short message ids: Graph ids are ~150 characters and share a long prefix, which is no fun
//! to type or to pass around in a prompt. Every listed message gets an 8-character id derived
//! from its full id; `cache/ids.tsv` maps it back (with the account it belongs to).

use std::collections::HashMap;
use std::io::Write as _;

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::paths;

const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
const MAX_BYTES: u64 = 2 * 1024 * 1024;

/// Stable 8-character id for a Graph message id.
pub fn short(id: &str) -> String {
    let hash = Sha256::digest(id.as_bytes());
    let mut bits: u64 = 0;
    for b in &hash[..5] {
        bits = (bits << 8) | u64::from(*b);
    }
    (0..8).rev().map(|i| ALPHABET[((bits >> (i * 5)) & 31) as usize] as char).collect()
}

/// Remember `(account, full id)` pairs so their short ids resolve later.
pub fn remember<'a>(account: &str, ids: impl IntoIterator<Item = &'a str>) {
    let path = paths::ids_file();
    let mut lines = String::new();
    for id in ids {
        lines.push_str(&format!("{}\t{account}\t{id}\n", short(id)));
    }
    if lines.is_empty() {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::metadata(&path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        compact(&path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(lines.as_bytes());
    }
}

/// Keep the newer half of the index.
fn compact(path: &std::path::Path) {
    if let Ok(text) = std::fs::read_to_string(path) {
        let lines: Vec<&str> = text.lines().collect();
        let keep = lines[lines.len() / 2..].join("\n") + "\n";
        let _ = paths::write_private(path, keep.as_bytes());
    }
}

fn index() -> HashMap<String, (String, String)> {
    let mut map = HashMap::new();
    if let Ok(text) = std::fs::read_to_string(paths::ids_file()) {
        for line in text.lines() {
            let mut parts = line.splitn(3, '\t');
            if let (Some(s), Some(a), Some(id)) = (parts.next(), parts.next(), parts.next()) {
                map.insert(s.to_string(), (a.to_string(), id.to_string()));
            }
        }
    }
    map
}

/// A full id plus the account it was seen in (`None` when a full id was given).
pub struct Resolved {
    pub account: Option<String>,
    pub id: String,
}

/// Accept a full Graph id or a (prefix of a) short id.
pub fn resolve(input: &str) -> Result<Resolved> {
    let input = input.trim();
    if input.len() > 20 {
        return Ok(Resolved { account: None, id: input.to_string() });
    }
    let key = input.to_lowercase();
    let index = index();
    if let Some((account, id)) = index.get(&key) {
        return Ok(Resolved { account: Some(account.clone()), id: id.clone() });
    }
    let matches: Vec<_> = index.iter().filter(|(s, _)| s.starts_with(&key)).collect();
    match matches.as_slice() {
        [(_, (account, id))] => Ok(Resolved { account: Some(account.clone()), id: id.clone() }),
        [] => Err(Error::not_found(format!("unknown message id '{input}'"))
            .hint("list or search first (`jm list`, `jm search`) so short ids are known, or pass the full id")),
        _ => Err(Error::validation(format!("'{input}' matches {} messages", matches.len()))
            .hint("use more characters of the id")),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn short_ids_are_stable_and_short() {
        let a = super::short("AAMkAGI2THVSAAA=");
        assert_eq!(a.len(), 8);
        assert_eq!(a, super::short("AAMkAGI2THVSAAA="));
        assert_ne!(a, super::short("AAMkAGI2THVSAAB="));
    }
}
