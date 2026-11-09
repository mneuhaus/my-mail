//! Where Just Mail keeps its files.
//!
//! `JUST_MAIL_HOME` moves everything somewhere else (tests, screenshots); the default is
//! `~/.config/just-mail`, shared by the app and the CLI.

use std::path::PathBuf;

pub fn home() -> PathBuf {
    if let Some(dir) = std::env::var_os("JUST_MAIL_HOME") {
        return PathBuf::from(dir);
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".config").join("just-mail")
}

pub fn config_file() -> PathBuf {
    home().join("config.toml")
}

pub fn account_dir(account_id: &str) -> PathBuf {
    home().join("accounts").join(account_id)
}

pub fn token_file(account_id: &str) -> PathBuf {
    account_dir(account_id).join("token.json")
}

pub fn events_file() -> PathBuf {
    home().join("events.jsonl")
}

pub fn ids_file() -> PathBuf {
    home().join("cache").join("ids.tsv")
}

/// Write a file atomically (temp file + rename), readable by its owner only from its first
/// byte on (it may hold a token).
pub fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};
    static WRITES: AtomicU64 = AtomicU64::new(0);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // a name of its own for every write: two at once must not share their temp file
    let tmp = path.with_extension(format!("tmp{}-{}", std::process::id(), WRITES.fetch_add(1, Ordering::Relaxed)));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options.open(&tmp).and_then(|mut file| file.write_all(data)).and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}
