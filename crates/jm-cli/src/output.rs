//! Compact output for people. Works on the same JSON values `--json` prints.

use chrono::{DateTime, Local, Utc};
use serde_json::Value;

/// How to show a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Accounts,
    AccountChange,
    Folders,
    Messages,
    Message,
    Html,
    Raw,
    Attachments,
    Saved,
    Changed,
    Draft,
    Deleted,
    Opened,
}

const WHO_WIDTH: usize = 24;
const SUBJECT_WIDTH: usize = 90;

pub fn render(view: View, v: &Value) -> String {
    match view {
        View::Accounts => accounts(v),
        View::AccountChange => account_change(v),
        View::Folders => folders(v),
        View::Messages => messages(v),
        View::Message => message(&v["message"], false),
        View::Html => format!("{}\n", s(&v["message"]["html"])),
        View::Raw => format!("{}\n", serde_json::to_string_pretty(&v["raw"]).unwrap_or_default()),
        View::Attachments => attachments(v),
        View::Saved => saved(v),
        View::Changed => changed(v),
        View::Draft => draft(v),
        View::Deleted => format!("deleted draft {} ({})\n", s(&v["id"]), s(&v["account"])),
        View::Opened => opened(v),
    }
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

fn yes(v: &Value) -> bool {
    v.as_bool().unwrap_or(false)
}

/// At most `max` characters, `…` when cut (never splits a character).
pub fn truncate(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn local_time(v: &Value, format: &str) -> String {
    v.as_str()
        .and_then(|t| t.parse::<DateTime<Utc>>().ok())
        .map(|t| t.with_timezone(&Local).format(format).to_string())
        .unwrap_or_else(|| " ".repeat(16))
}

fn person(r: &Value) -> String {
    let (name, address) = (s(&r["name"]).trim(), s(&r["address"]));
    if name.is_empty() || name == address { address.to_string() } else { format!("{name} <{address}>") }
}

fn short_person(r: &Value) -> &str {
    let name = s(&r["name"]).trim();
    if name.is_empty() { s(&r["address"]) } else { name }
}

fn people(list: &Value) -> String {
    list.as_array().map(|l| l.iter().map(person).collect::<Vec<_>>().join(", ")).unwrap_or_default()
}

fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1024 * 1024 => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
        b if b >= 1024 => format!("{} KB", b / 1024),
        b => format!("{b} B"),
    }
}

fn accounts(v: &Value) -> String {
    let list = v["accounts"].as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        return "no accounts yet: `jm accounts import` or `jm accounts add`\n".into();
    }
    let id_width = list.iter().map(|a| s(&a["id"]).chars().count()).max().unwrap_or(0);
    let mail_width = list.iter().map(|a| s(&a["email"]).chars().count()).max().unwrap_or(0);
    let mut out = String::new();
    for a in &list {
        let mut notes = vec![if yes(&a["signed_in"]) { "signed in" } else { "NOT signed in" }];
        if yes(&a["read_only"]) {
            notes.push("read-only");
        }
        if yes(&a["signature"]) {
            notes.push("signature");
        }
        if yes(&a["default"]) {
            notes.push("default");
        }
        out.push_str(&format!(
            "{:<id_width$}  {:<mail_width$}  {}  [{}]\n",
            s(&a["id"]),
            s(&a["email"]),
            s(&a["name"]),
            notes.join(", ")
        ));
    }
    out
}

fn account_change(v: &Value) -> String {
    match s(&v["action"]) {
        "imported" => {
            let mut out = String::new();
            for a in v["imported"].as_array().into_iter().flatten() {
                out.push_str(&format!(
                    "imported {} ({}) from ms365-mail profile {}\n",
                    s(&a["id"]),
                    s(&a["email"]),
                    s(&a["profile"])
                ));
            }
            for f in v["failed"].as_array().into_iter().flatten() {
                out.push_str(&format!(
                    "failed   {} (profile {}): {}\n",
                    s(&f["username"]),
                    s(&f["profile"]),
                    s(&f["error"]["message"])
                ));
            }
            out
        }
        "removed" => format!("removed {} ({})\n", s(&v["account"]["id"]), s(&v["account"]["email"])),
        _ => format!("added {} ({})\n", s(&v["account"]["id"]), s(&v["account"]["email"])),
    }
}

fn folders(v: &Value) -> String {
    let list = v["folders"].as_array().cloned().unwrap_or_default();
    let label = |f: &Value| {
        let indent = "  ".repeat(f["depth"].as_u64().unwrap_or(0) as usize);
        format!("{indent}{}", truncate(s(&f["name"]), 50))
    };
    let width = list.iter().map(|f| label(f).chars().count()).max().unwrap_or(0);
    let mut out = String::new();
    for f in &list {
        let counts = format!("{}/{}", f["unread"].as_u64().unwrap_or(0), f["total"].as_u64().unwrap_or(0));
        let well_known = f["well_known"].as_str().map(|w| format!("  ({w})")).unwrap_or_default();
        out.push_str(&format!("{:<width$}  {counts:>11}{well_known}\n", label(f)));
    }
    out
}

/// `a1b2c3d4  2026-10-07 17:20  ● ⚑ 📎 Anastasiia von Spark      Subject …`
pub fn message_line(m: &Value) -> String {
    let unread = if yes(&m["unread"]) { "●" } else { " " };
    let flag = match s(&m["flag"]) {
        "flagged" => "⚑",
        "complete" => "✓",
        _ => " ",
    };
    let clip = if yes(&m["has_attachments"]) { "📎" } else { "  " };
    let who = if yes(&m["draft"]) {
        let to = m["to"].as_array().cloned().unwrap_or_default();
        match to.as_slice() {
            [] => "→ (no recipient)".to_string(),
            [one] => format!("→ {}", short_person(one)),
            [one, rest @ ..] => format!("→ {} +{}", short_person(one), rest.len()),
        }
    } else {
        short_person(&m["from"]).to_string()
    };
    format!(
        "{}  {}  {unread} {flag} {clip} {:<WHO_WIDTH$}  {}",
        s(&m["id"]),
        local_time(&m["date"], "%Y-%m-%d %H:%M"),
        truncate(&who, WHO_WIDTH),
        truncate(s(&m["subject"]), SUBJECT_WIDTH)
    )
}

fn messages(v: &Value) -> String {
    let list = v["messages"].as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        return "no messages\n".into();
    }
    let mut out: String = list.iter().map(|m| message_line(m) + "\n").collect();
    if !v["next"].is_null() {
        out.push_str(&format!("… more: raise -n (now {}) or narrow with --since/--from\n", list.len()));
    }
    out
}

fn headers(m: &Value) -> String {
    let mut out = String::new();
    let mut line = |label: &str, value: String| {
        if !value.trim().is_empty() {
            out.push_str(&format!("{label:<9}{value}\n"));
        }
    };
    line("Subject:", s(&m["subject"]).to_string());
    if !m["from"].is_null() {
        line("From:", person(&m["from"]));
    }
    line("To:", people(&m["to"]));
    line("Cc:", people(&m["cc"]));
    line("Bcc:", people(&m["bcc"]));
    line("Date:", local_time(&m["date"], "%Y-%m-%d %H:%M (%a)"));
    let mut marks = Vec::new();
    if yes(&m["draft"]) {
        marks.push("draft".to_string());
    }
    if yes(&m["unread"]) {
        marks.push("unread".to_string());
    }
    match s(&m["flag"]) {
        "flagged" => marks.push("flagged".into()),
        "complete" => marks.push("flag complete".into()),
        _ => {}
    }
    if let Some(imp) = m["importance"].as_str().filter(|i| *i != "normal") {
        marks.push(format!("importance {imp}"));
    }
    let marks = if marks.is_empty() { String::new() } else { format!("  [{}]", marks.join(", ")) };
    line("Id:", format!("{}{marks}", s(&m["id"])));
    let files = m["attachments"].as_array().cloned().unwrap_or_default();
    if !files.is_empty() {
        out.push_str("Attachments:\n");
        for a in &files {
            let inline = if yes(&a["inline"]) { ", inline" } else { "" };
            out.push_str(&format!(
                "  {} ({}, {}{inline})\n",
                s(&a["name"]),
                size(a["size"].as_u64().unwrap_or(0)),
                a["content_type"].as_str().unwrap_or("?")
            ));
        }
    }
    out
}

fn message(m: &Value, author_only: bool) -> String {
    let mut out = headers(m);
    let author = m["author_text"].as_str();
    if let Some(author) = author {
        out.push_str("\n--- author text ---\n");
        out.push_str(author.trim_end());
        out.push('\n');
        if !author_only {
            out.push_str("\n--- whole draft ---\n");
        }
    } else {
        out.push('\n');
    }
    if !author_only || author.is_none() {
        out.push_str(s(&m["text"]).trim_end());
        out.push('\n');
    }
    out
}

fn draft(v: &Value) -> String {
    let d = &v["draft"];
    let mut out = format!("{} draft {} ({})\n", s(&v["action"]), s(&d["id"]), s(&v["account"]));
    out.push_str(&message(d, true));
    let signature = if yes(&d["has_signature"]) { "with signature" } else { "no signature" };
    out.push_str(&format!("\n({signature}; whole draft: jm drafts show {})\n", s(&d["id"])));
    out
}

fn attachments(v: &Value) -> String {
    let list = v["attachments"].as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        return "no attachments\n".into();
    }
    list.iter()
        .map(|a| {
            let mut notes =
                vec![size(a["size"].as_u64().unwrap_or(0)), a["content_type"].as_str().unwrap_or("?").to_string()];
            if yes(&a["inline"]) {
                notes.push("inline".into());
            }
            if !yes(&a["file"]) {
                notes.push("item, not downloadable".into());
            }
            format!("{}  ({})\n", s(&a["name"]), notes.join(", "))
        })
        .collect()
}

fn saved(v: &Value) -> String {
    v["saved"].as_array().into_iter().flatten().map(|f| format!("{}\n", s(&f["path"]))).collect()
}

fn changed(v: &Value) -> String {
    let m = &v["message"];
    let what = match s(&v["action"]) {
        "flag" => match s(&m["flag"]) {
            "flagged" => "flagged".to_string(),
            "complete" => "flag complete".to_string(),
            _ => "flag cleared".to_string(),
        },
        "move" => format!("moved to {}", s(&v["folder"])),
        other => format!("marked {other}"),
    };
    format!("{}  {what}  {}\n", s(&m["id"]), truncate(s(&m["subject"]), SUBJECT_WIDTH))
}

fn opened(v: &Value) -> String {
    let mut out = format!("asked Just Mail to show {} ({})\n", s(&v["id"]), s(&v["account"]));
    if let Some(note) = v["note"].as_str() {
        out.push_str(&format!("note: {note}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn truncates_on_character_boundaries() {
        assert_eq!(truncate("Grüße aus Gütersloh", 7), "Grüße …");
        assert_eq!(truncate("kurz", 10), "kurz");
        assert_eq!(truncate("📎📎📎", 2), "📎…");
    }

    #[test]
    fn list_lines_show_marks_and_people() {
        let m = json!({"id":"a1b2c3d4","date":"2026-10-07T15:20:00Z","unread":true,"flag":"flagged",
            "has_attachments":false,"draft":false,"from":{"name":"Anastasiia von Spark","address":"a@spark.de"},
            "subject":"Microsoft Graph wird jetzt teurer"});
        let line = message_line(&m);
        assert!(line.starts_with("a1b2c3d4  2026-10-07 "), "{line}");
        assert!(line.contains("● ⚑    Anastasiia von Spark"), "{line}");
        assert!(line.ends_with("Microsoft Graph wird jetzt teurer"));
        let draft = json!({"id":"x","draft":true,"to":[{"name":"","address":"max@x.de"}],"subject":"Hi"});
        assert!(message_line(&draft).contains("→ max@x.de"));
    }
}
