//! Small helpers: dates, files, clipboard.

use chrono::{DateTime, Datelike, Local, Utc};

use crate::i18n;

const MONTHS_EN: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const MONTHS_DE: [&str; 12] = ["Jan.", "Feb.", "März", "Apr.", "Mai", "Juni", "Juli", "Aug.", "Sept.", "Okt.", "Nov.", "Dez."];
const DAYS_EN: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const DAYS_DE: [&str; 7] = ["Mo.", "Di.", "Mi.", "Do.", "Fr.", "Sa.", "So."];

/// Compact date for the message list: time today, weekday this week, day and month this year.
pub fn list_date(date: Option<DateTime<Utc>>) -> String {
    let Some(date) = date else { return String::new() };
    let local = date.with_timezone(&Local);
    let now = Local::now();
    let days = (now.date_naive() - local.date_naive()).num_days();
    let german = i18n::german();
    if days == 0 {
        local.format("%H:%M").to_string()
    } else if days == 1 {
        if german { "Gestern".into() } else { "Yesterday".into() }
    } else if (0..7).contains(&days) {
        let wd = local.weekday().num_days_from_monday() as usize;
        if german { DAYS_DE[wd].into() } else { DAYS_EN[wd].into() }
    } else if local.year() == now.year() {
        let m = local.month0() as usize;
        if german { format!("{}. {}", local.day(), MONTHS_DE[m]) } else { format!("{} {}", MONTHS_EN[m], local.day()) }
    } else if german {
        local.format("%d.%m.%y").to_string()
    } else {
        local.format("%Y-%m-%d").to_string()
    }
}

/// Full date for the reader header.
pub fn long_date(date: Option<DateTime<Utc>>) -> String {
    let Some(date) = date else { return String::new() };
    let local = date.with_timezone(&Local);
    let m = local.month0() as usize;
    let wd = local.weekday().num_days_from_monday() as usize;
    if i18n::german() {
        format!("{} {}. {} {}, {}", DAYS_DE[wd], local.day(), MONTHS_DE[m], local.year(), local.format("%H:%M"))
    } else {
        format!("{} {} {}, {} {}", DAYS_EN[wd], MONTHS_EN[m], local.day(), local.year(), local.format("%H:%M"))
    }
}

/// `12 KB`, `1.4 MB`.
pub fn size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Fetches an attachment into ~/Downloads, to open it from there.
pub fn save_attachment(mailbox: &jm_core::Mailbox, message_id: &str, attachment: &jm_core::Attachment) -> jm_core::Result<std::path::PathBuf> {
    let bytes = mailbox.attachment_bytes(message_id, &attachment.id)?;
    let path = download_path(&attachment.name);
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// A free path in ~/Downloads for `name` ("file (2).pdf" if taken).
fn download_path(name: &str) -> std::path::PathBuf {
    let dir = dirs_downloads();
    let clean: String = name.chars().map(|c| if c == '/' || c == ':' { '_' } else { c }).collect();
    let clean = if clean.trim().is_empty() { "attachment".to_string() } else { clean };
    let mut path = dir.join(&clean);
    let (stem, ext) = match clean.rfind('.') {
        Some(i) if i > 0 => (clean[..i].to_string(), clean[i..].to_string()),
        _ => (clean.clone(), String::new()),
    };
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    path
}

fn dirs_downloads() -> std::path::PathBuf {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_else(|| ".".into());
    home.join("Downloads")
}

/// Shorten to `max` characters on a character boundary.
pub fn ellipsize(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
