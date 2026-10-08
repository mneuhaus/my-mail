//! A tiny append-only event log (`events.jsonl`) through which the CLI tells a running app what
//! changed ("drafts of marc changed") or what to show ("open this message").

use std::io::{Read as _, Seek as _, SeekFrom, Write as _};

use serde::{Deserialize, Serialize};

use crate::paths;

const MAX_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// Something in this account changed; `folder` is a well-known name or folder id.
    Changed { account: String, folder: Option<String> },
    /// Show this message in the app.
    Open { account: String, id: String },
}

/// Append an event. Never fails loudly: the app also refreshes on its own.
pub fn emit(event: &Event) {
    let path = paths::events_file();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::metadata(&path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let _ = std::fs::write(&path, b"");
    }
    if let (Ok(line), Ok(mut f)) =
        (serde_json::to_string(event), std::fs::OpenOptions::new().create(true).append(true).open(&path))
    {
        let _ = f.write_all(format!("{line}\n").as_bytes());
    }
}

/// Reads events appended after it was created.
pub struct EventReader {
    offset: u64,
}

impl Default for EventReader {
    fn default() -> Self {
        Self::new()
    }
}

impl EventReader {
    /// Start at the current end: older events are not replayed.
    pub fn new() -> Self {
        let offset = std::fs::metadata(paths::events_file()).map(|m| m.len()).unwrap_or(0);
        Self { offset }
    }

    pub fn poll(&mut self) -> Vec<Event> {
        let path = paths::events_file();
        let Ok(len) = std::fs::metadata(&path).map(|m| m.len()) else { return vec![] };
        if len < self.offset {
            self.offset = 0; // truncated by a writer
        }
        if len == self.offset {
            return vec![];
        }
        let Ok(mut f) = std::fs::File::open(&path) else { return vec![] };
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return vec![];
        }
        let mut text = String::new();
        if f.read_to_string(&mut text).is_err() {
            return vec![];
        }
        // only consume complete lines
        let complete = text.rfind('\n').map(|i| i + 1).unwrap_or(0);
        self.offset += complete as u64;
        text[..complete].lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }
}
