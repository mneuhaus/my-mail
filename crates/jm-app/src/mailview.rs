//! The mail in a web view. Mail HTML is made for browsers, so WebKit shows it the way its sender
//! designed it (columns, colors, fonts), which a native rendering could only approximate.
//!
//! One web view lies over the body area of the reader; GPUI keeps toolbar and header. Each mail on
//! the page lives in a shadow root, so its styles stay its own. A content policy lets only the
//! page's own script run (none of a mail) and keeps remote content (images, fonts, styles) out
//! until the reader allows images. The script passes link clicks, copying and the app's keyboard
//! shortcuts on (`window.ipc`). Mail with its own background colors shows as designed, on light;
//! plain mail takes the app's colors, in dark mode all of them.

use std::cell::Cell;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;

use futures::channel::mpsc;
use gpui_kit::*;
use html_escape::{encode_double_quoted_attribute as attr, encode_text as text};
use serde::Deserialize;
use wry::dpi::{LogicalPosition, LogicalSize};

/// What the page asks of the app.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum WebMessage {
    /// A link was clicked.
    Open(String),
    /// ⌘C on a selection.
    Copy(String),
    /// A key the page has no use for: maybe a shortcut of the app.
    Key { key: String, cmd: bool, shift: bool },
    /// "Edit" on a draft below the mail.
    Edit(String),
}

/// One mail on the page.
pub struct Card {
    /// Sender, date and recipients above the body: for the later messages of a conversation.
    pub head: Option<CardHead>,
    /// The mail as HTML (see `jm_core::html::to_web_document`).
    pub html: String,
}

pub struct CardHead {
    pub sender: String,
    pub date: String,
    pub to: String,
    /// The id of a draft, to offer editing it.
    pub draft: Option<String>,
}

/// The page's colors, from the app's theme.
pub struct Palette {
    pub dark: bool,
    pub foreground: Hsla,
    pub muted: Hsla,
    pub border: Hsla,
    pub card: Hsla,
    pub hover: Hsla,
    pub link: Hsla,
    pub selection: Hsla,
}

pub struct MailView {
    web: Rc<wry::WebView>,
    /// Hash of what the page shows, so it only loads again when that changes.
    shown: Option<u64>,
    /// Where the web view was put last, in window coordinates.
    placed: Rc<Cell<Option<Bounds<Pixels>>>>,
    visible: Rc<Cell<bool>>,
    /// A menu of GPUI lies where the web view would be (see `cover`).
    covered: Rc<Cell<bool>>,
}

impl MailView {
    /// The web view, hidden until a mail is shown. What the page asks comes out of the receiver.
    pub fn new(window: &Window) -> wry::Result<(Self, mpsc::UnboundedReceiver<WebMessage>)> {
        let (tx, rx) = mpsc::unbounded();
        let web = wry::WebViewBuilder::new()
            .with_transparent(true)
            .with_visible(false)
            .with_focused(false)
            .with_accept_first_mouse(true)
            .with_back_forward_navigation_gestures(false)
            .with_devtools(cfg!(debug_assertions))
            // the page and its mails, nothing else: links go through `WebMessage::Open`
            .with_navigation_handler(|url| url == "about:blank")
            .with_ipc_handler(move |request| {
                if let Ok(message) = serde_json::from_str(request.body()) {
                    let _ = tx.unbounded_send(message);
                }
            })
            .build_as_child(window)?;
        let view = Self { web: Rc::new(web), shown: None, placed: Rc::default(), visible: Rc::default(), covered: Rc::default() };
        Ok((view, rx))
    }

    /// Hide the web view and give the keyboard back to the window (clicked into, the web view
    /// keeps it, and the editor that comes next would get no keys).
    pub fn hide(&self, cx: &App) {
        if self.visible.replace(false) {
            let _ = self.web.set_visible(false);
            // AppKit tells the window at once, and the window would update the app while it is
            // being updated: later, outside of it
            let web = self.web.clone();
            cx.foreground_executor()
                .spawn(async move {
                    let _ = web.focus_parent();
                })
                .detach();
        }
    }

    /// GPUI cannot draw over the web view: while the returned guard lives, it stays hidden, if an
    /// overlay reaching from the left up to `right` (in the window) would lie under it.
    pub fn cover(&self, right: Pixels) -> Option<Cover> {
        let placed = self.placed.get()?;
        if !self.visible.get() || right <= placed.origin.x {
            return None;
        }
        self.covered.set(true);
        self.visible.set(false);
        let _ = self.web.set_visible(false);
        Some(Cover(self.covered.clone()))
    }

    /// Show the mails `cards` builds, unless `key` (with `remote` and the palette) says the page
    /// shows them already.
    pub fn show(&mut self, key: u64, cards: impl FnOnce() -> Vec<Card>, remote: bool, palette: &Palette) {
        let mut hasher = DefaultHasher::new();
        (key, remote, palette.dark).hash(&mut hasher);
        let key = hasher.finish();
        if self.shown != Some(key) {
            self.shown = Some(key);
            let _ = self.web.load_html(&page(&cards(), remote, palette));
        }
    }

    /// Takes the space the web view should cover; it is put there when this is laid out.
    pub fn element(&self) -> impl IntoElement {
        let (web, placed, visible, covered) =
            (self.web.clone(), self.placed.clone(), self.visible.clone(), self.covered.clone());
        canvas(
            move |bounds, _, _| {
                if placed.get() != Some(bounds) {
                    placed.set(Some(bounds));
                    let rect = wry::Rect {
                        position: LogicalPosition::new(f64::from(bounds.origin.x), f64::from(bounds.origin.y)).into(),
                        size: LogicalSize::new(f64::from(bounds.size.width), f64::from(bounds.size.height)).into(),
                    };
                    let _ = web.set_bounds(rect);
                }
                if !covered.get() && !visible.replace(true) {
                    let _ = web.set_visible(true);
                }
            },
            |_, _, _, _| {},
        )
        .size_full()
    }
}

/// Lets the web view show again when dropped (on the next paint).
pub struct Cover(Rc<Cell<bool>>);

impl Drop for Cover {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// A color for CSS.
fn css(color: Hsla) -> String {
    let c = color.to_rgb();
    let byte = |v: f32| (v * 255.).round() as u8;
    format!("rgba({},{},{},{:.3})", byte(c.r), byte(c.g), byte(c.b), c.a)
}

/// The whole page: one card per mail, the mails as data for the script, which puts each into a
/// shadow root of its card (so its styles stay inside). Only that script may run: the content
/// policy allows it by its nonce, so nothing in a mail runs (no script, no `onerror`, no
/// `javascript:` link), and nothing is loaded from elsewhere unless `remote`.
fn page(cards: &[Card], remote: bool, p: &Palette) -> String {
    let scheme = if p.dark { "dark" } else { "light" };
    let vars = format!(
        ":root{{color-scheme:{scheme};--fg:{};--muted:{};--border:{};--card:{};--hover:{};--link:{};--selection:{}}}",
        css(p.foreground),
        css(p.muted),
        css(p.border),
        css(p.card),
        css(p.hover),
        css(p.link),
        css(p.selection),
    );
    let mut body = String::new();
    let mut mails = Vec::new();
    for (i, card) in cards.iter().enumerate() {
        let designed = jm_core::html::has_own_colors(&card.html);
        body.push_str(if designed { "<div class=\"card designed\">" } else { "<div class=\"card\">" });
        if let Some(head) = &card.head {
            let edit = head.draft.as_ref().map_or(String::new(), |id| {
                format!("<button class=\"edit\" data-edit=\"{}\">{}</button>", attr(id), crate::tr!("Edit", "Bearbeiten"))
            });
            body.push_str(&format!(
                "<div class=\"head\"><div class=\"row\"><div class=\"who\">{}</div>{edit}<div class=\"when\">{}</div></div>{}</div>",
                text(&head.sender),
                text(&head.date),
                if head.to.is_empty() { String::new() } else { format!("<div class=\"to\">{}</div>", text(&head.to)) },
            ));
        }
        body.push_str(&format!("<div class=\"body\" data-mail=\"{i}\"></div></div>"));
        let colors = match (designed, p.dark) {
            (true, _) => DESIGNED_CSS.to_string(),
            (false, false) => PLAIN_CSS.to_string(),
            // its colors are meant for white paper
            (false, true) => format!("{PLAIN_CSS}{PLAIN_DARK_CSS}"),
        };
        mails.push(serde_json::json!({ "html": card.html, "css": format!("{MAIL_CSS}{colors}") }));
    }
    // inside the script: no `<` that could end it
    let mails = serde_json::to_string(&mails).unwrap_or_default().replace('<', "\\u003c");
    let remote_src = if remote { " https: http:" } else { "" };
    let nonce = {
        use std::hash::BuildHasher as _;
        format!("{:016x}", std::collections::hash_map::RandomState::new().hash_one(std::time::SystemTime::now()))
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; script-src 'nonce-{nonce}'; \
         style-src 'unsafe-inline'{remote_src}; img-src data:{remote_src}; font-src data:{remote_src}; form-action 'none'; base-uri 'none'\">\
         <style>{vars}{PAGE_CSS}</style></head><body>{body}<script nonce=\"{nonce}\">const MAILS = {mails};{PAGE_JS}</script></body></html>"
    )
}

const PAGE_CSS: &str = "\
html,body{margin:0;background:transparent}\
body{padding:0 24px 24px;font:14px/1.45 -apple-system,BlinkMacSystemFont,sans-serif;color:var(--fg)}\
::selection{background:var(--selection)}\
.card{border:1px solid var(--border);border-radius:10px;background:var(--card);overflow:hidden}\
.card+.card{margin-top:12px}\
.head{padding:16px 24px 0}\
.row{display:flex;gap:12px;align-items:center}\
.who{flex:1;min-width:0;font-weight:500}\
.when,.to{font-size:12px;color:var(--muted)}\
.to{margin-top:2px}\
.edit{font:inherit;font-size:12px;color:var(--fg);background:transparent;border:0;border-radius:6px;padding:2px 8px;cursor:pointer}\
.edit:hover{background:var(--hover)}\
.body{padding:20px 24px}\
.head+.body{padding-top:12px}\
.designed .body{padding:0;background:#fff}\
.designed .head{padding-bottom:12px}";

/// In front of every mail's own styles. `.jm-body` stands for the mail's `<body>`.
const MAIL_CSS: &str = ".jm-body{overflow-wrap:break-word;overflow-x:auto}::selection{background:var(--selection)}";

/// Mail with its own colors, on the white it was made for.
const DESIGNED_CSS: &str = ".jm-body{background:#fff;color:#000;color-scheme:light}";

/// Plain mail sits on the card like the app's own text; big pictures shrink to fit.
const PLAIN_CSS: &str = "a{color:var(--link)}img{max-width:100%;height:auto}";

/// Plain mail in dark mode: all of it in the app's colors.
const PLAIN_DARK_CSS: &str = "\
.jm-body *{color:inherit!important;background-color:transparent!important;border-color:var(--border)!important}\
.jm-body a,.jm-body a *{color:var(--link)!important}";

/// Puts each mail into the shadow root of its card: parsed apart (`DOMParser`, where nothing runs
/// or loads), its styles and body taken over, `html` and `body` in its styles pointed at the box it
/// is shown in. Passes link clicks, copying and keys on to the app: arrows, space and the like
/// stay with the page for scrolling, ⌘C copies through the app, other keys are the app's shortcuts.
const PAGE_JS: &str = r##"
(() => {
  const send = (m) => window.ipc.postMessage(JSON.stringify(m));
  const outer = /(^|[^\w.#-])(html|body)(?![\w-])/gi;
  function retarget(rules) {
    for (const rule of rules) {
      if (rule.selectorText) rule.selectorText = rule.selectorText.replace(outer, "$1.jm-body");
      else if (rule.cssRules) retarget(rule.cssRules);
    }
  }
  function mount(host, mail) {
    const doc = new DOMParser().parseFromString(mail.html, "text/html");
    const root = host.attachShadow({ mode: "open" });
    const own = document.createElement("style");
    own.textContent = mail.css;
    root.append(own);
    for (const el of doc.head.querySelectorAll("style, link[rel~='stylesheet']")) root.append(document.importNode(el, true));
    const body = document.createElement("div");
    for (const name of ["style", "dir", "lang"]) {
      const value = doc.body.getAttribute(name) || (name !== "style" && doc.documentElement.getAttribute(name));
      if (value) body.setAttribute(name, value);
    }
    body.className = ("jm-body " + (doc.body.getAttribute("class") || "")).trim();
    const bg = doc.body.getAttribute("bgcolor"), color = doc.body.getAttribute("text");
    if (bg) body.style.backgroundColor = bg;
    if (color) body.style.color = color;
    for (const node of doc.body.childNodes) body.append(document.importNode(node, true));
    root.append(body);
    for (const style of root.querySelectorAll("style")) if (style.sheet) retarget(style.sheet.cssRules);
  }
  for (const host of document.querySelectorAll("[data-mail]")) mount(host, MAILS[+host.dataset.mail]);

  const own = new Set(["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "PageUp", "PageDown", "Home", "End", " ",
    "Tab", "Shift", "Meta", "Control", "Alt", "CapsLock", "Fn", "Dead", "Unidentified"]);
  document.addEventListener("keydown", (e) => {
    if (e.ctrlKey || e.altKey || e.isComposing || own.has(e.key)) return;
    e.preventDefault();
    if (e.metaKey && e.key === "c") {
      const text = document.getSelection().toString(); // as shown, shadow roots included
      if (text) send({ copy: text });
    } else if (e.metaKey && e.key === "a") {
      // all of one mail (where the selection is): a selection does not reach into more shadow roots
      const selection = document.getSelection();
      const roots = [...document.querySelectorAll("[data-mail]")].map((host) => host.shadowRoot);
      const [range] = selection.getComposedRanges({ shadowRoots: roots });
      const root = range && roots.find((r) => r.contains(range.startContainer)) || roots[0];
      const body = root.querySelector(".jm-body");
      selection.setBaseAndExtent(body, 0, body, body.childNodes.length);
    } else {
      send({ key: { key: e.key, cmd: e.metaKey, shift: e.shiftKey } });
    }
  }, true);
  document.addEventListener("click", (e) => {
    const target = e.composedPath()[0];
    const el = target.closest && target.closest("a[href], area[href], [data-edit]");
    if (!el) return;
    e.preventDefault();
    if (el.dataset.edit) return send({ edit: el.dataset.edit });
    const href = el.getAttribute("href");
    if (href.startsWith("#")) {
      const root = el.getRootNode(), name = href.slice(1);
      const to = root.getElementById(name) || root.querySelector(`[name="${CSS.escape(name)}"]`);
      if (to) to.scrollIntoView();
    } else {
      send({ open: el.href });
    }
  }, true);
})();
"##;
