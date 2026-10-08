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

/// Write a file atomically (temp file + rename) with owner-only permissions.
pub fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)
}
