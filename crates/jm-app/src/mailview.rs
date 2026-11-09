//! The mail in a web view. Mail HTML is made for browsers, so WebKit shows it the way its sender
//! designed it (columns, colors, fonts), which a native rendering could only approximate.
//!
//! One web view lies over the body area of the reader; GPUI keeps toolbar and header. Each mail on
//! the page lives in a shadow root, so its styles stay its own. A content policy lets only the
//! page's own script run (none of a mail) and keeps remote content (images, fonts, styles) out
//! until the reader allows images. The script passes link clicks, copying and the app's keyboard
//! shortcuts on (`window.ipc`). Mail with its own background colors shows as designed, on light;
//! plain mail takes the app's colors, in dark mode all of them.
//!
//! The web view keeps nothing between mails (no cookies, no cache), previews no links, downloads
//! nothing, and navigates nowhere.
//!
//! The draft editor uses the same web view for its own page (see `editor`): reader and editor
//! never show at the same time.

use std::cell::Cell;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::channel::mpsc;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use html_escape::{encode_double_quoted_attribute as attr, encode_text as text};
use serde::Deserialize;
use wry::dpi::{LogicalPosition, LogicalSize};

/// What the page asks of the app, and what happens to the web view itself (never read from a
/// page's message: a page cannot claim that files were dropped on it).
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
    /// A click into the mail: the keyboard is the web view's now.
    Focused,
    /// From the draft editor's page.
    Editor(crate::editor::EditorMessage),
    /// Files are dragged over the draft editor's page, or left it again.
    #[serde(skip)]
    Dragging(bool),
    /// Files were dropped on the draft editor's page, at this point of it.
    #[serde(skip)]
    Dropped { paths: Vec<PathBuf>, at: (i32, i32) },
    /// WebKit's process for the page ended (it crashed, or macOS took its memory): the page is
    /// blank until it is loaded again.
    #[serde(skip)]
    Ended,
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
    /// What the page lies on (the page itself is transparent).
    pub background: Hsla,
    pub card: Hsla,
    pub popover: Hsla,
    /// A button under the pointer, and one held down.
    pub hover: Hsla,
    pub active: Hsla,
    /// The one blue for what matters.
    pub accent: Hsla,
    pub danger: Hsla,
    pub link: Hsla,
    pub selection: Hsla,
}

impl Palette {
    /// The colors of the app's theme.
    pub fn of(cx: &App) -> Self {
        let theme = cx.theme();
        Palette {
            dark: theme.mode.is_dark(),
            foreground: theme.foreground,
            muted: theme.muted_foreground,
            border: theme.border,
            background: theme.background,
            card: crate::theme::extra(cx).card,
            popover: theme.popover,
            hover: theme.secondary_hover,
            active: theme.secondary_active,
            accent: theme.primary,
            danger: theme.danger,
            link: theme.link,
            selection: theme.selection,
        }
    }

    /// The same colors without a running app (see `editor::sample_page`).
    pub fn sample(dark: bool) -> Self {
        let color = |name| crate::theme::color_of(name, dark);
        Palette {
            dark,
            foreground: color("foreground"),
            muted: color("muted_foreground"),
            border: color("border"),
            background: color("background"),
            card: crate::theme::extra_of(dark).card,
            popover: color("popover"),
            hover: color("secondary_hover"),
            active: color("secondary_active"),
            accent: color("primary"),
            danger: color("danger"),
            link: color("link"),
            selection: color("selection"),
        }
    }
}

/// A handle: clones are the same web view.
#[derive(Clone)]
pub struct MailView {
    web: Rc<wry::WebView>,
    /// Hash of what the page shows, so it only loads again when that changes.
    shown: Rc<Cell<Option<u64>>>,
    /// Where the web view was put last, in window coordinates.
    placed: Rc<Cell<Option<Bounds<Pixels>>>>,
    visible: Rc<Cell<bool>>,
    /// A menu of GPUI lies where the web view would be (see `cover`).
    covered: Rc<Cell<bool>>,
    /// Take the keyboard as soon as it shows (hidden, it cannot).
    focus_pending: Rc<Cell<bool>>,
}

impl MailView {
    /// The web view, hidden until a mail is shown. What the page asks comes out of the receiver.
    pub fn new(window: &Window) -> wry::Result<(Self, mpsc::UnboundedReceiver<WebMessage>)> {
        let (tx, rx) = mpsc::unbounded();
        let placed: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::default();
        let shown: Rc<Cell<Option<u64>>> = Rc::default();
        #[cfg(target_os = "macos")]
        use wry::WebViewBuilderExtDarwin as _;
        let web = wry::WebViewBuilder::new()
            .with_transparent(true)
            .with_visible(false)
            .with_focused(false)
            .with_accept_first_mouse(true)
            .with_back_forward_navigation_gestures(false)
            .with_devtools(cfg!(debug_assertions))
            // the page and its mails, nothing else: links go through `WebMessage::Open`
            .with_navigation_handler(|url| url == "about:blank")
            // nothing a mail links to is fetched: not by "Download Linked File" of WebKit's menu,
            // not by a preview of the link; and no mail finds what another one left behind
            .with_download_started_handler(|_, _| false)
            .with_incognito(true)
            .with_ipc_handler({
                let tx = tx.clone();
                move |request| {
                    if let Ok(message) = serde_json::from_str(request.body()) {
                        let _ = tx.unbounded_send(message);
                    }
                }
            })
            // files dragged onto the draft's text are the app's (WebKit would try to open them);
            // any other drag, text moved within the text, stays WebKit's
            .with_drag_drop_handler({
                let (tx, placed, files) = (tx.clone(), placed.clone(), Cell::new(false));
                move |event| {
                    let ours = files.get();
                    match event {
                        wry::DragDropEvent::Enter { paths, .. } => {
                            files.set(EDITOR.load(Ordering::Relaxed) && !paths.is_empty());
                            if files.get() {
                                let _ = tx.unbounded_send(WebMessage::Dragging(true));
                            }
                            return files.get();
                        }
                        wry::DragDropEvent::Drop { paths, position: (x, y) } if ours => {
                            files.set(false);
                            // wry counts from the window's left edge
                            let left = placed.get().map_or(0., |p| f32::from(p.origin.x));
                            let _ = tx.unbounded_send(WebMessage::Dropped { paths, at: (x - left as i32, y) });
                        }
                        wry::DragDropEvent::Leave if ours => {
                            files.set(false);
                            let _ = tx.unbounded_send(WebMessage::Dragging(false));
                        }
                        _ => {}
                    }
                    ours
                }
            });
        #[cfg(target_os = "macos")]
        let web = web.with_allow_link_preview(false).with_on_web_content_process_terminate_handler({
            let shown = shown.clone();
            move || {
                shown.set(None);
                let _ = tx.unbounded_send(WebMessage::Ended);
            }
        });
        let web = web.build_as_child(window)?;
        keys_stay_in_the_page();
        let view = Self {
            web: Rc::new(web),
            shown,
            placed,
            visible: Rc::default(),
            covered: Rc::default(),
            focus_pending: Rc::default(),
        };
        Ok((view, rx))
    }

    /// Hide the web view and give the keyboard back to the window (the editor that comes next
    /// would get no keys).
    pub fn hide(&self, cx: &App) {
        self.focus_pending.set(false);
        if self.visible.replace(false) {
            let _ = self.web.set_visible(false);
            self.release_keyboard(cx);
        }
    }

    /// Give the keyboard back to the window. Clicked into, the web view keeps it, also when the
    /// app is clicked next (GPUI's view does not take it back): what is typed into the search
    /// would go to the page, which passes it on as shortcuts.
    pub fn release_keyboard(&self, cx: &App) {
        // AppKit tells the window at once, and the window would update the app while it is
        // being updated: later, outside of it
        let web = self.web.clone();
        cx.foreground_executor()
            .spawn(async move {
                let _ = web.focus_parent();
            })
            .detach();
    }

    /// Give the web view the keyboard (now, or when it shows next).
    pub fn focus(&self, cx: &App) {
        if !self.visible.get() {
            self.focus_pending.set(true);
            return;
        }
        // later, outside of the update: see `release_keyboard`
        let web = self.web.clone();
        cx.foreground_executor()
            .spawn(async move {
                let _ = web.focus();
            })
            .detach();
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
    pub fn show(&self, key: u64, cards: impl FnOnce() -> Vec<Card>, remote: bool, palette: &Palette) {
        let mut hasher = DefaultHasher::new();
        (key, remote, palette.dark).hash(&mut hasher);
        let key = hasher.finish();
        if self.shown.get() != Some(key) {
            self.shown.set(Some(key));
            EDITOR.store(false, Ordering::Relaxed);
            let _ = self.web.load_html(&page(&cards(), remote, palette));
        }
    }

    /// Load the page again with the next [`MailView::show`], even if it shows the same mails.
    pub fn forget(&self) {
        self.shown.set(None);
    }

    /// Load another page than the mails (the draft editor's).
    pub fn load(&self, html: &str) {
        self.shown.set(None);
        EDITOR.store(true, Ordering::Relaxed);
        let _ = self.web.load_html(html);
    }

    /// Run `js` in the page.
    pub fn evaluate(&self, js: &str) {
        let _ = self.web.evaluate_script(js);
    }

    /// Takes the space the web view should cover; it is put there when this is laid out.
    pub fn element(&self) -> impl IntoElement {
        let (web, placed, visible, covered, focus) = (
            self.web.clone(),
            self.placed.clone(),
            self.visible.clone(),
            self.covered.clone(),
            self.focus_pending.clone(),
        );
        canvas(
            move |bounds, _, cx| {
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
                if visible.get() && focus.take() {
                    cx.foreground_executor()
                        .spawn(async move {
                            let _ = web.focus();
                        })
                        .detach();
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

/// Whether the page is the draft editor's (see `keys_stay_in_the_page`).
static EDITOR: AtomicBool = AtomicBool::new(false);

/// While the editor has the keyboard, keys without ⌘ are its text's. AppKit offers arrows (and other
/// function keys) to GPUI's view first, as key equivalents, and GPUI hands them to its own text
/// input; macOS then typed them into the draft as control characters (0x1C–0x1F for the arrows).
/// Shortcuts with ⌘ still go to GPUI first. The reader keeps GPUI's way: ↑/↓ go through the list
/// even after a click into the mail.
#[cfg(target_os = "macos")]
fn keys_stay_in_the_page() {
    use std::sync::OnceLock;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
    use objc2::{msg_send, sel};

    type KeyEquivalent = unsafe extern "C-unwind" fn(&AnyObject, Sel, &AnyObject) -> Bool;
    type KeyDown = unsafe extern "C-unwind" fn(&AnyObject, Sel, &AnyObject);
    static GPUI_KEY_EQUIVALENT: OnceLock<Imp> = OnceLock::new();
    static GPUI_KEY_DOWN: OnceLock<Imp> = OnceLock::new();

    /// `view` is GPUI's; is `event` for the editor's page instead?
    unsafe fn for_the_page(view: &AnyObject, event: &AnyObject) -> bool {
        const COMMAND: usize = 1 << 20; // NSEventModifierFlagCommand
        if !EDITOR.load(Ordering::Relaxed) {
            return false;
        }
        let Some(web_view) = AnyClass::get(c"WKWebView") else { return false };
        // SAFETY: AppKit objects on the main thread, where AppKit sends key events
        unsafe {
            let flags: usize = msg_send![event, modifierFlags];
            let window: Option<Retained<AnyObject>> = msg_send![view, window];
            let responder: Option<Retained<AnyObject>> = window.and_then(|w| msg_send![&w, firstResponder]);
            flags & COMMAND == 0
                && responder.is_some_and(|r| {
                    let is: Bool = msg_send![&r, isKindOfClass: web_view];
                    is.as_bool()
                })
        }
    }
    unsafe extern "C-unwind" fn key_equivalent(view: &AnyObject, sel: Sel, event: &AnyObject) -> Bool {
        // SAFETY: GPUI's implementation, with the signature it was made for
        unsafe {
            if for_the_page(view, event) {
                return Bool::NO; // AppKit goes on to the web view's `keyDown:`
            }
            std::mem::transmute::<Imp, KeyEquivalent>(GPUI_KEY_EQUIVALENT.get().copied().unwrap())(view, sel, event)
        }
    }
    // keys the page did not use come back to its superview, GPUI's view
    unsafe extern "C-unwind" fn key_down(view: &AnyObject, sel: Sel, event: &AnyObject) {
        // SAFETY: as above
        unsafe {
            if !for_the_page(view, event) {
                std::mem::transmute::<Imp, KeyDown>(GPUI_KEY_DOWN.get().copied().unwrap())(view, sel, event);
            }
        }
    }

    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Some(class) = AnyClass::get(c"GPUIView") else { return };
        let ours: [(Sel, Imp, &OnceLock<Imp>); 2] = [
            // SAFETY: function pointers of the same size; called back with their own signature
            (sel!(performKeyEquivalent:), unsafe { std::mem::transmute::<KeyEquivalent, Imp>(key_equivalent) }, &GPUI_KEY_EQUIVALENT),
            (sel!(keyDown:), unsafe { std::mem::transmute::<KeyDown, Imp>(key_down) }, &GPUI_KEY_DOWN),
        ];
        let methods = class.instance_methods();
        for (sel, imp, gpui) in ours {
            // GPUI's own methods only: replacing one inherited from NSView would change every view
            if let Some(method) = methods.iter().find(|m| m.name() == sel) {
                // SAFETY: same signature as GPUI's, which stays callable through `gpui`
                let _ = gpui.set(unsafe { method.set_implementation(imp) });
            }
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn keys_stay_in_the_page() {}

/// A color for CSS.
pub(crate) fn css(color: Hsla) -> String {
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
    let nonce = nonce();
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; script-src 'nonce-{nonce}'; \
         style-src 'unsafe-inline'{remote_src}; img-src data:{remote_src}; font-src data:{remote_src}; form-action 'none'; base-uri 'none'\">\
         <style>{vars}{PAGE_CSS}</style></head><body>{body}<script nonce=\"{nonce}\">const MAILS = {mails};{MOUNT_JS}{PAGE_JS}</script></body></html>"
    )
}

/// A fresh value that lets only the page's own script run (content policy `script-src`).
pub(crate) fn nonce() -> String {
    use std::hash::BuildHasher as _;
    format!("{:016x}", std::collections::hash_map::RandomState::new().hash_one(std::time::SystemTime::now()))
}

const PAGE_CSS: &str = "\
html,body{margin:0;background:transparent}\
body{padding:0 24px 24px;font:14px/1.45 -apple-system,BlinkMacSystemFont,sans-serif;color:var(--fg)}\
::selection{background:var(--selection)}\
.card{border:1px solid var(--border);border-radius:10px;background:var(--card);overflow:hidden;contain:paint}\
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
pub(crate) const MAIL_CSS: &str = ".jm-body{overflow-wrap:break-word;overflow-x:auto}::selection{background:var(--selection)}";

/// Mail with its own colors, on the white it was made for.
pub(crate) const DESIGNED_CSS: &str = ".jm-body{background:#fff;color:#000;color-scheme:light}";

/// Plain mail sits on the card like the app's own text; big pictures shrink to fit.
pub(crate) const PLAIN_CSS: &str = "a{color:var(--link)}img{max-width:100%;height:auto}";

/// Plain mail in dark mode: all of it in the app's colors.
pub(crate) const PLAIN_DARK_CSS: &str = "\
.jm-body *{color:inherit!important;background-color:transparent!important;border-color:var(--border)!important}\
.jm-body a,.jm-body a *{color:var(--link)!important}";

/// `mount(host, mail)` puts a mail into the shadow root of `host`: parsed apart (`DOMParser`,
/// where nothing runs or loads), its styles and body taken over, `html` and `body` in its styles
/// pointed at the box it is shown in. `mail` is `{ html, css }`.
pub(crate) const MOUNT_JS: &str = r##"
const mount = (() => {
  const outer = /(^|[^\w.#-])(html|body)(?![\w-])/gi;
  function retarget(rules) {
    for (const rule of rules) {
      if (rule.selectorText) rule.selectorText = rule.selectorText.replace(outer, "$1.jm-body");
      else if (rule.cssRules) retarget(rule.cssRules);
    }
  }
  // a mail may name an element "body" or "head", and `doc.body` would then be that element
  const part = (doc, name) => Object.getOwnPropertyDescriptor(Document.prototype, name).get.call(doc);
  const all = (doc, selector) => Document.prototype.querySelectorAll.call(doc, selector);
  function mount(host, mail) {
    const doc = new DOMParser().parseFromString(mail.html, "text/html");
    // nothing that could run, load or lead elsewhere (the content policy holds them back too)
    for (const el of all(doc, "script, iframe, frame, frameset, object, embed, applet, base, meta, link:not([rel~='stylesheet'])")) el.remove();
    const head = part(doc, "head"), from = part(doc, "body"), html = part(doc, "documentElement");
    const root = host.attachShadow({ mode: "open" });
    const own = document.createElement("style");
    own.textContent = mail.css;
    root.append(own);
    for (const el of head.querySelectorAll("style, link")) root.append(document.importNode(el, true));
    const body = document.createElement("div");
    for (const name of ["style", "dir", "lang"]) {
      const value = from.getAttribute(name) || (name !== "style" && html.getAttribute(name));
      if (value) body.setAttribute(name, value);
    }
    body.className = ("jm-body " + (from.getAttribute("class") || "")).trim();
    const bg = from.getAttribute("bgcolor"), color = from.getAttribute("text");
    if (bg) body.style.backgroundColor = bg;
    if (color) body.style.color = color;
    for (const node of [...from.childNodes]) body.append(document.importNode(node, true));
    root.append(body);
    for (const style of root.querySelectorAll("style")) if (style.sheet) retarget(style.sheet.cssRules);
  }
  return mount;
})();
"##;

/// Puts each mail into the shadow root of its card (`MOUNT_JS`). Passes link clicks, copying and
/// keys on to the app: arrows, space and the like stay with the page for scrolling, ⌘C copies
/// through the app, other keys are the app's shortcuts.
const PAGE_JS: &str = r##"
(() => {
  const send = (m) => window.ipc.postMessage(JSON.stringify(m));
  // the keyboard is here now: ⌘C copies from the mail, not from the field that had it before
  window.addEventListener("focus", () => send("focused"));
  // WebKit's own menu only for what is selected, a link or a picture: on the bare page it offers
  // "Reload", which would leave the page empty
  document.addEventListener("contextmenu", (e) => {
    const target = e.composedPath()[0];
    if (!document.getSelection().toString() && !(target.closest && target.closest("a[href], img"))) e.preventDefault();
  });

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
    // the page's own button only: a mail cannot have one
    if (el.dataset.edit) return el.getRootNode() === document && send({ edit: el.dataset.edit });
    const href = el.getAttribute("href");
    if (href.startsWith("#")) {
      const root = el.getRootNode(), name = href.slice(1);
      const to = root.getElementById(name) || root.querySelector(`[name="${CSS.escape(name)}"]`);
      if (to) to.scrollIntoView();
    } else {
      send({ open: el.href });
    }
  }, true);

  // last, so a mail that cannot be shown takes neither the keys nor the other mails along
  for (const host of document.querySelectorAll("[data-mail]")) {
    try {
      mount(host, MAILS[+host.dataset.mail]);
    } catch (error) {
      host.textContent = String(error);
    }
  }
})();
"##;
