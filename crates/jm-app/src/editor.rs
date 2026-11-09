//! The draft editor's page: the author's part of a draft as rich text (`contenteditable` in
//! WebKit, on the web view the reader uses). What a draft carries, bold, lists, tables, pictures
//! and links, stays as it is; nothing becomes plain text and back. Below the text, as the mail
//! goes out and not editable: the signature, then a reply's quoted mail on request.
//!
//! Above the text the page has a format bar (GPUI cannot draw menus over a web view, the page
//! can): size, bold, italic, underlined, struck through, colors, lists, quote, link, picture,
//! table, and taking formats away. Its script and styles are `editor/page.js` and
//! `editor/page.css`; `tools/editor-test` runs them in WebKit.
//!
//! The page reports every change (its HTML, pictures as `cid:` again), when it gets the keyboard,
//! and what only the app can do: read the pasteboard (a screenshot becomes a picture of the
//! draft, copied files attachments, HTML from elsewhere comes in cleaned, see
//! `jm_core::html::clean_for_editing`) and pick a picture from disk. Keys: ⌘B, ⌘I, ⌘U, ⇧⌘X struck
//! through, ⌘K link, ⇧⌘7 numbered and ⇧⌘8 bulleted list, ⇥ and ⇧⇥ in lists and tables, ⌘Z and
//! ⇧⌘Z, ⌘X, ⌘C, ⌘V, ⇧⌘V as text, ⌘A.

use std::sync::atomic::{AtomicU64, Ordering};

use gpui_kit::AssetSource as _;
use html_escape::encode_double_quoted_attribute as attr;
use jm_core::markdown;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::mailview::{self, Palette, css};
use crate::tr;

/// A message of the editor's page.
#[derive(Deserialize, Debug)]
pub struct EditorMessage {
    /// The page that sent it (see [`Content::page`]): a late one from an earlier draft's page is
    /// dropped.
    pub page: u64,
    #[serde(flatten)]
    pub event: EditorEvent,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum EditorEvent {
    /// Its script runs: it takes `JM.…` calls from now on.
    Ready,
    /// The author's HTML after a change.
    Edited { html: String },
    /// The text got the keyboard.
    Focused,
    /// ⌘V or "Paste": the app reads the pasteboard and inserts what fits; `plain` as text only.
    Paste {
        #[serde(default)]
        plain: bool,
    },
    /// The picture button: the app asks for files and puts them into the text.
    Picture,
    /// ⇧⇥ in the text: back to the field above it.
    Leave,
    /// Every change made so far was reported (answer to `JM.flush()`).
    Flushed,
}

/// What the page shows.
pub struct Content<'a> {
    /// Tells this page's messages apart (see [`next_page`]).
    pub page: u64,
    /// The author's part of the draft.
    pub html: &'a str,
    /// The pictures it embeds as `data:` URLs, by content id (lowercase).
    pub pictures: &'a [(String, String)],
    /// The signature as it goes out.
    pub signature: Option<&'a str>,
    /// A reply's quoted mail (`jm_core::html::to_web_document`).
    pub quote: Option<&'a str>,
    /// Put the cursor into the text once it shows.
    pub focus: bool,
}

/// A number for the next page.
pub fn next_page() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// `JM.<function>(args…)` for [`mailview::MailView::evaluate`]; `args` is one value, or a tuple of them.
pub fn call(function: &str, args: impl Serialize) -> String {
    let args = match serde_json::to_value(args).unwrap_or_default() {
        serde_json::Value::Array(list) => list.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(","),
        serde_json::Value::Null => String::new(),
        one => one.to_string(),
    };
    format!("JM.{function}({args})")
}

/// The HTML the pasteboard holds (copied from a web page, Word, Pages or the editor itself).
#[cfg(target_os = "macos")]
pub fn pasteboard_html() -> Option<String> {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeHTML};
    // SAFETY: an AppKit constant
    let kind = unsafe { NSPasteboardTypeHTML };
    NSPasteboard::generalPasteboard().stringForType(kind).map(|html| html.to_string())
}

#[cfg(not(target_os = "macos"))]
pub fn pasteboard_html() -> Option<String> {
    None
}

/// An icon of the app (Lucide, as GPUI's buttons show them) as SVG for the page.
fn icon(name: &str) -> String {
    let svg = crate::AppAssets.load(&format!("icons/{name}.svg")).ok().flatten().unwrap_or_default();
    String::from_utf8_lossy(&svg).into_owned()
}

/// The format bar: a button runs a command of the page's script (`data-cmd`) or opens a menu
/// (`data-menu`).
fn bar() -> String {
    let button = |kind: &str, name: &str, picture: &str, tip: &str| {
        format!("<button type=\"button\" tabindex=\"-1\" data-{kind}=\"{name}\" title=\"{}\">{}</button>", attr(tip), icon(picture))
    };
    let separator = "<span class=\"sep\"></span>".to_string();
    [
        button("menu", "size", "a-large-small", tr!("Text size", "Schriftgröße")),
        button("cmd", "bold", "bold", tr!("Bold (⌘B)", "Fett (⌘B)")),
        button("cmd", "italic", "italic", tr!("Italic (⌘I)", "Kursiv (⌘I)")),
        button("cmd", "underline", "underline", tr!("Underline (⌘U)", "Unterstrichen (⌘U)")),
        button("cmd", "strike", "strikethrough", tr!("Strike through (⇧⌘X)", "Durchgestrichen (⇧⌘X)")),
        button("menu", "color", "palette", tr!("Text color and marker", "Textfarbe und Markierung")),
        separator.clone(),
        button("cmd", "bullets", "list", tr!("Bulleted list (⇧⌘8, or “- ” at the start of a line)", "Aufzählung (⇧⌘8, oder „- “ am Zeilenanfang)")),
        button("cmd", "numbers", "list-ordered", tr!("Numbered list (⇧⌘7, or “1. ” at the start of a line)", "Nummerierte Liste (⇧⌘7, oder „1. “ am Zeilenanfang)")),
        button("cmd", "quote", "text-quote", tr!("Quote", "Zitat")),
        separator.clone(),
        button("menu", "link", "link", tr!("Link (⌘K)", "Link (⌘K)")),
        button("cmd", "picture", "image", tr!("Insert pictures (or paste them, or drop them on the text)", "Bilder einfügen (oder per ⌘V, oder auf den Text ziehen)")),
        button("menu", "table", "table", tr!("Table", "Tabelle")),
        separator,
        button("cmd", "clear", "remove-formatting", tr!("Remove formatting", "Formatierung entfernen")),
    ]
    .concat()
}

pub fn page(content: &Content, p: &Palette) -> String {
    let scheme = if p.dark { "dark" } else { "light" };
    let vars = format!(
        ":root{{color-scheme:{scheme};--fg:{};--muted:{};--border:{};--hover:{};--active:{};--link:{};--selection:{};\
         --bg:{};--field:{};--popover:{};--accent:{};--on:{};--danger:{}}}",
        css(p.foreground),
        css(p.muted),
        css(p.border),
        css(p.hover),
        css(p.active),
        css(p.link),
        css(p.selection),
        css(p.background),
        css(p.background),
        css(p.popover),
        css(p.accent),
        css(p.accent.opacity(0.14)),
        css(p.danger),
    );
    let plain = if p.dark { format!("{}{}", mailview::PLAIN_CSS, mailview::PLAIN_DARK_CSS) } else { mailview::PLAIN_CSS.into() };
    let mail = |html: &str| {
        let colors = if jm_core::html::has_own_colors(html) { mailview::DESIGNED_CSS } else { plain.as_str() };
        json!({ "html": html, "css": format!("{}{colors}", mailview::MAIL_CSS) })
    };
    let data = json!({
        "page": content.page,
        "html": content.html,
        "pictures": content.pictures.iter().cloned().collect::<std::collections::HashMap<_, _>>(),
        "signature": content.signature.map(mail),
        "quote": content.quote.map(mail),
        "focus": content.focus,
        // what a mail needs inline, as Markdown from `jm` comes out
        "styles": { "td": markdown::CELL, "th": markdown::HEAD_CELL, "quote": markdown::QUOTE },
        "icons": { "check": icon("check"), "open": icon("external-link"), "unlink": icon("unlink"), "none": icon("ban") },
        "labels": {
            "small": tr!("Small", "Klein"),
            "normal": tr!("Normal", "Normal"),
            "large": tr!("Large", "Groß"),
            "huge": tr!("Very large", "Sehr groß"),
            "color": tr!("Text color", "Textfarbe"),
            "automatic": tr!("Automatic", "Automatisch"),
            "mark": tr!("Marker", "Markierung"),
            "unmarked": tr!("None", "Keine"),
            "address": tr!("Web or mail address", "Web- oder Mail-Adresse"),
            "apply": tr!("Link (↩)", "Verlinken (↩)"),
            "open": tr!("Open the link", "Link öffnen"),
            "unlink": tr!("Remove the link", "Link entfernen"),
            "table": tr!("Insert a table", "Tabelle einfügen"),
            "rowAbove": tr!("Row above", "Zeile darüber"),
            "rowBelow": tr!("Row below", "Zeile darunter"),
            "columnLeft": tr!("Column to the left", "Spalte links"),
            "columnRight": tr!("Column to the right", "Spalte rechts"),
            "removeRow": tr!("Delete row", "Zeile löschen"),
            "removeColumn": tr!("Delete column", "Spalte löschen"),
            "removeTable": tr!("Delete table", "Tabelle löschen"),
            "showQuote": tr!("Show quoted mail", "Zitierte E-Mail anzeigen"),
            "hideQuote": tr!("Hide quoted mail", "Zitierte E-Mail ausblenden"),
        },
    });
    // inside the script: no `<` that could end it
    let data = data.to_string().replace('<', "\\u003c");
    let placeholder = attr(tr!("Write your message…", "Deine Nachricht…"));
    let signature_tip = attr(tr!("Signature: change it under Accounts & signatures", "Signatur: änderbar unter Konten & Signaturen"));
    let nonce = mailview::nonce();
    format!(
        "<!doctype html><html class=\"{scheme}\"><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; script-src 'nonce-{nonce}'; \
         style-src 'unsafe-inline'; img-src data:; font-src data:; form-action 'none'; base-uri 'none'\">\
         <style>{vars}{PAGE_CSS}</style></head><body>\
         <div id=\"bar\">{}</div><div id=\"pop\" hidden></div>\
         <div id=\"main\"><div id=\"text\" contenteditable=\"true\" spellcheck=\"true\" data-placeholder=\"{placeholder}\"></div>\
         <div id=\"signature\" title=\"{signature_tip}\" hidden></div>\
         <button id=\"quote-toggle\" hidden></button><div id=\"quote\" hidden></div></div>\
         <div id=\"frame\" hidden><i></i><span></span></div>\
         <script nonce=\"{nonce}\">const DATA = {data};{}{PAGE_JS}</script></body></html>",
        bar(),
        mailview::MOUNT_JS
    )
}

/// The page with an empty text, a signature and a quote, for `tools/editor-test` (see `main`).
pub fn sample_page(dark: bool) -> String {
    let content = Content {
        page: 1,
        html: "",
        pictures: &[],
        signature: Some("<div>Alex Morgan<br>Northwind Studio</div>"),
        quote: Some("<html><body><p>Hi Alex, thanks for the quote.</p></body></html>"),
        focus: true,
    };
    page(&content, &Palette::sample(dark))
}

const PAGE_CSS: &str = include_str!("editor/page.css");
const PAGE_JS: &str = include_str!("editor/page.js");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_take_one_argument_or_several() {
        assert_eq!(call("flush", ()), "JM.flush()");
        assert_eq!(call("insert", "<b>\"a\"</b>"), "JM.insert(\"<b>\\\"a\\\"</b>\")");
        assert_eq!(call("caret", (12, 40)), "JM.caret(12,40)");
        assert_eq!(call("dropping", true), "JM.dropping(true)");
    }

    #[test]
    fn the_page_has_every_icon_and_no_way_out_of_its_script() {
        let content = Content { page: 7, html: "</script><b>x</b>", pictures: &[], signature: None, quote: None, focus: false };
        let page = page(&content, &Palette::sample(false));
        assert!(!page.contains("</script><b>"));
        assert!(page.contains("\\u003c/script>\\u003cb>x"));
        // 13 buttons in the bar, 4 icons for its menus (in the script's data)
        assert_eq!((page.matches("<svg").count(), page.matches("\\u003csvg").count()), (13, 4));
    }
}
