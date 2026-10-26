//! The draft editor's page: the author's part of a draft as rich text (`contenteditable` in
//! WebKit, on the web view the reader uses). What a draft carries, bold, lists, tables, pictures
//! and links, stays as it is; nothing becomes plain text and back. Below the text, as the mail
//! goes out and not editable: the signature, then a reply's quoted mail on request.
//!
//! The page reports every change (its HTML, pictures as `cid:` again), when it gets the keyboard,
//! and when something is to be pasted: the app reads the pasteboard itself, so a screenshot
//! becomes a picture of the draft, copied files attachments, and HTML from elsewhere comes in
//! cleaned (`jm_core::html::clean_for_editing`). Keys: ⌘B, ⌘I, ⌘U, ⌘⇧7 numbered and ⌘⇧8 bulleted
//! list, ⌘Z and ⌘⇧Z, ⌘X, ⌘C, ⌘V, ⌘A.

use std::sync::atomic::{AtomicU64, Ordering};

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
    /// ⌘V or "Paste": the app reads the pasteboard and inserts what fits.
    Paste,
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

/// `JM.<function>(arg)` for [`mailview::MailView::evaluate`].
pub fn call(function: &str, arg: impl Serialize) -> String {
    format!("JM.{function}({})", serde_json::to_string(&arg).unwrap_or_default())
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

pub fn page(content: &Content, p: &Palette) -> String {
    let scheme = if p.dark { "dark" } else { "light" };
    let vars = format!(
        ":root{{color-scheme:{scheme};--fg:{};--muted:{};--border:{};--hover:{};--link:{};--selection:{}}}",
        css(p.foreground),
        css(p.muted),
        css(p.border),
        css(p.hover),
        css(p.link),
        css(p.selection),
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
        "labels": { "show": tr!("Show quoted mail", "Zitierte E-Mail anzeigen"), "hide": tr!("Hide quoted mail", "Zitierte E-Mail ausblenden") },
    });
    // inside the script: no `<` that could end it
    let data = data.to_string().replace('<', "\\u003c");
    let dark = if p.dark { DARK_CSS } else { "" };
    let placeholder = html_escape::encode_double_quoted_attribute(tr!("Write your message…", "Deine Nachricht…"));
    let signature_tip = html_escape::encode_double_quoted_attribute(tr!(
        "Signature: change it under Accounts & signatures",
        "Signatur: änderbar unter Konten & Signaturen"
    ));
    let nonce = mailview::nonce();
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; script-src 'nonce-{nonce}'; \
         style-src 'unsafe-inline'; img-src data:; font-src data:; form-action 'none'; base-uri 'none'\">\
         <style>{vars}{PAGE_CSS}{dark}</style></head><body>\
         <div id=\"text\" contenteditable=\"true\" spellcheck=\"true\" data-placeholder=\"{placeholder}\"></div>\
         <div id=\"signature\" title=\"{signature_tip}\" hidden></div>\
         <button id=\"quote-toggle\" hidden></button><div id=\"quote\" hidden></div>\
         <script nonce=\"{nonce}\">const DATA = {data};{}{PAGE_JS}</script></body></html>",
        mailview::MOUNT_JS
    )
}

const PAGE_CSS: &str = "\
html,body{margin:0;background:transparent;min-height:100%}\
body{padding:14px 20px 40px;font:14px/1.5 -apple-system,BlinkMacSystemFont,sans-serif;color:var(--fg);cursor:text}\
::selection{background:var(--selection)}\
#text{position:relative;outline:none;min-height:80px;overflow-wrap:break-word}\
#text.empty::before{content:attr(data-placeholder);position:absolute;color:var(--muted);pointer-events:none}\
#text a{color:var(--link)}\
#text img{max-width:100%;height:auto}\
#text img.loading{min-width:48px;min-height:32px;background:var(--hover)}\
#text table{border-collapse:collapse}\
#text ul,#text ol{margin-top:0;margin-bottom:0}\
#signature{margin-top:20px;padding-top:14px;border-top:1px solid var(--border);cursor:default}\
#quote-toggle{margin-top:16px;font:inherit;font-size:12px;color:var(--muted);background:transparent;border:0;\
border-radius:6px;padding:2px 8px;cursor:pointer}\
#quote-toggle:hover{background:var(--hover);color:var(--fg)}\
#quote{margin-top:8px;padding:12px 16px;border-radius:8px;background:var(--hover);cursor:default}";

/// Dark mode: the text in the app's colors, whatever it brought along (as plain mail in the reader).
const DARK_CSS: &str = "\
#text *{color:inherit!important;background-color:transparent!important;border-color:var(--border)!important}\
#text a,#text a *{color:var(--link)!important}";

const PAGE_JS: &str = r##"
(() => {
  const send = (m) => window.ipc.postMessage(JSON.stringify({ editor: Object.assign({ page: DATA.page }, m) }));
  const text = document.getElementById("text");
  const BLANK = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";

  // pictures of the draft: `cid:` in the mail, shown from `data:` URLs (`JM.pictures`)
  text.innerHTML = DATA.html;
  for (const img of text.querySelectorAll("img")) {
    const src = img.getAttribute("src") || "";
    if (/^cid:/i.test(src)) {
      img.dataset.cid = src.slice(4);
      img.src = BLANK;
      img.classList.add("loading");
    }
  }
  const pictures = (found) => {
    for (const img of text.querySelectorAll("img[data-cid]")) {
      const src = found[img.dataset.cid.toLowerCase()];
      if (src) {
        img.src = src;
        img.classList.remove("loading");
      }
    }
  };
  pictures(DATA.pictures);
  const html = () => {
    const copy = text.cloneNode(true);
    for (const img of copy.querySelectorAll("img[data-cid]")) {
      img.setAttribute("src", "cid:" + img.dataset.cid);
      img.removeAttribute("data-cid");
      img.classList.remove("loading");
      if (!img.classList.length) img.removeAttribute("class");
    }
    return copy.innerHTML;
  };

  const empty = () => text.classList.toggle("empty", !text.textContent.trim() && !text.querySelector("img, table, li, hr"));
  empty();
  document.execCommand("defaultParagraphSeparator", false, "div");
  text.addEventListener("input", () => {
    empty();
    send({ type: "edited", html: html() });
  });

  // where the cursor was, for what the app inserts and its toolbar
  let saved = null;
  document.addEventListener("selectionchange", () => {
    const s = getSelection();
    if (s.rangeCount && text.contains(s.getRangeAt(0).commonAncestorContainer)) saved = s.getRangeAt(0).cloneRange();
  });
  const restore = () => {
    if (document.activeElement !== text) text.focus();
    if (saved) {
      const s = getSelection();
      s.removeAllRanges();
      s.addRange(saved);
    }
  };
  const focused = () => send({ type: "focused" });
  text.addEventListener("focus", focused);
  window.addEventListener("focus", () => document.activeElement === text && focused());

  // a click below the text puts the cursor at its end
  document.addEventListener("mousedown", (e) => {
    if (e.target !== document.body && e.target !== document.documentElement) return;
    e.preventDefault();
    text.focus();
    const s = getSelection();
    s.selectAllChildren(text);
    s.collapseToEnd();
  });

  const commands = { b: "bold", i: "italic", u: "underline", x: "cut", c: "copy", z: "undo" };
  document.addEventListener("keydown", (e) => {
    if (!e.metaKey || e.ctrlKey || e.altKey || e.isComposing) return;
    const key = e.key.toLowerCase();
    let command = null;
    if (e.shiftKey) {
      if (e.code === "Digit7") command = "insertOrderedList";
      else if (e.code === "Digit8") command = "insertUnorderedList";
      else if (key === "z") command = "redo";
    } else if (key === "a") {
      e.preventDefault();
      getSelection().selectAllChildren(text.contains(getSelection().anchorNode) ? text : document.body);
    } else if (key === "v") {
      e.preventDefault();
      send({ type: "paste" });
    } else {
      command = commands[key];
    }
    if (command) {
      e.preventDefault();
      document.execCommand(command);
    }
  }, true);
  text.addEventListener("paste", (e) => {
    e.preventDefault();
    send({ type: "paste" });
  });
  // files go in through the paperclip; dropped, WebKit would try to open them
  for (const type of ["dragover", "drop"]) {
    text.addEventListener(type, (e) => [...(e.dataTransfer?.types || [])].includes("Files") && e.preventDefault());
  }

  if (DATA.signature) {
    const signature = document.getElementById("signature");
    mount(signature, DATA.signature);
    signature.hidden = false;
  }
  if (DATA.quote) {
    const quote = document.getElementById("quote"), toggle = document.getElementById("quote-toggle");
    toggle.textContent = DATA.labels.show;
    toggle.hidden = false;
    toggle.addEventListener("click", () => {
      if (!quote.shadowRoot) mount(quote, DATA.quote);
      quote.hidden = !quote.hidden;
      toggle.textContent = quote.hidden ? DATA.labels.show : DATA.labels.hide;
    });
  }

  window.JM = {
    insert(markup) {
      restore();
      document.execCommand("insertHTML", false, markup);
    },
    command(name) {
      restore();
      document.execCommand(name);
    },
    pictures,
    // a picture that could not go into the draft
    drop(cid) {
      for (const img of text.querySelectorAll("img[data-cid]")) if (img.dataset.cid === cid) img.remove();
      empty();
      send({ type: "edited", html: html() });
    },
    focus() {
      text.focus();
      if (!saved) {
        const s = getSelection();
        s.selectAllChildren(text);
        s.collapseToStart();
      }
    },
    // every change was sent right away; this one comes after them
    flush: () => send({ type: "flushed" }),
  };
  if (DATA.focus) JM.focus();
  send({ type: "ready" });
})();
"##;
