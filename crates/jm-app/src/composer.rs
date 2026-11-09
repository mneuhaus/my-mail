//! The draft editor. Every draft lives on the server (Drafts folder), so the CLI, Outlook and
//! this window all see the same thing. Edits are saved two seconds after the last keystroke.
//!
//! The text is rich text in WebKit (see `editor`): it holds only the author's part, as HTML, so
//! bold, lists, tables and pictures stay as they are, whoever wrote them (an agent through `jm`
//! too). The signature and a reply's quote stay in the draft's HTML untouched (see
//! `jm_core::html`). A picture (pasted, dropped on the text or picked from disk) goes in where
//! the cursor is and out as an inline image of the draft; other files are attached.
//!
//! Whatever changes the draft on the server goes one after the other, a save, then the uploads
//! (`saving`, `attaching`, and `DRAFT_WRITES` across composers): Graph refuses changes to one
//! message that overlap. A composer that goes away, because another mail was opened or the app
//! quits, still saves what was typed since the last save (`save_last_edits`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{IndentInline, Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, Disableable, Icon, Selectable as _, Sizable, WindowExt};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use jm_core::html::InlineImage;
use jm_core::{Attachment, DraftInput, Mailbox, Message, NewAttachment, Recipient, html};

use crate::actions::{self, SaveDraft, SendDraft};
use crate::editor::{self, EditorEvent, EditorMessage};
use crate::mailview::{Cover, MailView, Palette};
use crate::recipients::RecipientField;
use crate::sidebar::TOP_H;
use crate::{picture, theme, tr, util};

const AUTOSAVE: Duration = Duration::from_secs(2);
/// How long Send waits for the page to report the last keystrokes before it goes on anyway.
const FLUSH_WAIT: Duration = Duration::from_millis(800);
/// How long quitting waits for the last words to be saved.
const QUIT_WAIT: Duration = Duration::from_secs(5);

/// One change to a draft at a time, also across composers: a draft opened again must not be read
/// before the composer that just left it has saved its last words.
static DRAFT_WRITES: Mutex<()> = Mutex::new(());

/// Wait for the turn to read or change a draft (off the UI thread).
fn turn() -> MutexGuard<'static, ()> {
    DRAFT_WRITES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Save `input` to the draft `known` (a new one if there is none yet), when it is its turn.
fn write_draft(mailbox: &Mailbox, known: &Mutex<Option<String>>, input: &DraftInput) -> jm_core::Result<Message> {
    let _turn = turn();
    // not held while the request is on its way: the UI thread sets it too
    let id = known.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let message = match id {
        Some(id) => mailbox.update_draft(&id, input)?,
        None => mailbox.create_draft(input, true, &[])?,
    };
    *known.lock().unwrap_or_else(PoisonError::into_inner) = Some(message.id.clone());
    Ok(message)
}

/// On its way into the draft: it waits for the draft to exist.
enum Upload {
    /// A picture of the text.
    Picture(InlineImage),
    File(NewAttachment),
}

impl Upload {
    /// Into the draft `id`. Tells its name, the content id of a picture, and the attachment a
    /// file became.
    fn send(self, mailbox: &Mailbox, id: &str) -> (String, Option<String>, jm_core::Result<Option<Attachment>>) {
        match self {
            Upload::Picture(image) => {
                let added = mailbox.add_inline_image(id, &image).map(|_| None);
                (image.name, Some(image.content_id), added)
            }
            Upload::File(file) => {
                let added = mailbox.add_attachment(id, &file).map(Some);
                (file.name, None, added)
            }
        }
    }
}

pub enum ComposerEvent {
    Saved { account: usize, message: Box<Message> },
    Sent { account: usize, id: String, recipients: String },
    Discarded { account: usize, id: Option<String> },
}

impl EventEmitter<ComposerEvent> for Composer {}

pub struct Composer {
    account: usize,
    mailbox: Arc<Mailbox>,
    /// The draft as last saved; `None` until a new mail is first saved.
    draft: Option<Message>,
    /// Its id as the server has it, for work off the UI thread: a save on its way may have just
    /// made the draft.
    known: Arc<Mutex<Option<String>>>,
    /// Sent or deleted: nothing of it is to be saved anymore.
    gone: bool,
    loading: bool,
    to: Entity<RecipientField>,
    cc: Entity<RecipientField>,
    bcc: Entity<RecipientField>,
    subject: Entity<InputState>,
    show_cc: bool,
    attachments: Vec<Attachment>,
    /// The web view the text is edited in (`None`: WebKit could not start).
    view: Option<MailView>,
    /// This composer's editor page (see `editor::next_page`), whether it is dark, and whether its
    /// script runs.
    page: u64,
    page_dark: Option<bool>,
    page_ready: bool,
    /// Put the cursor into the text once the page shows.
    focus_text: bool,
    /// The author's part as the page last reported it.
    body_html: String,
    /// Body as last saved, to tell whether it needs saving.
    saved_html: String,
    /// The pictures of the text as `data:` URLs, by content id (lowercase).
    pictures: Vec<(String, String)>,
    /// Names given to pasted pictures (`Bild 1`, …), without the extension.
    picture_names: Vec<String>,
    /// Pictures and files waiting for the draft to exist, or for their turn.
    waiting: Vec<Upload>,
    /// Pictures and files on their way into the draft: being read, waiting or going up.
    uploading: usize,
    /// Uploads are going up right now.
    attaching: bool,
    /// Draft written by Just Mail (the text is the author's part only).
    has_markers: bool,
    /// The signature as it goes out, shown below the text (long for new mails, short for answers).
    signature: Option<String>,
    /// The quoted mail of a reply or forward, shown below the signature on request.
    quote: Option<String>,
    dirty: bool,
    saving: bool,
    sending: bool,
    /// Send once the page reported its last keystrokes.
    send_after_flush: bool,
    /// Send as soon as the running save and uploads finish.
    send_after_save: bool,
    /// Bumped on every edit; autosave only fires for the latest one.
    edits: u64,
    error: Option<String>,
    /// Keeps the web view hidden while recipient suggestions hang over it.
    cover: Option<Cover>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl Composer {
    fn base(
        account: usize,
        mailbox: Arc<Mailbox>,
        view: Option<MailView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let recipients = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            let account = mailbox.account.id.clone();
            cx.new(|cx| RecipientField::new(account, placeholder, window, cx))
        };
        let to = recipients(tr!("Recipients", "Empfänger"), window, cx);
        let cc = recipients(tr!("Copy", "Kopie"), window, cx);
        let bcc = recipients(tr!("Blind copy", "Blindkopie"), window, cx);
        let subject = cx.new(|cx| InputState::new(window, cx).placeholder(tr!("Subject", "Betreff")));
        let mut subscriptions = Vec::new();
        let inputs = [&to, &cc, &bcc].map(|field| field.read(cx).input().clone());
        for input in inputs.iter().chain([&subject]) {
            subscriptions.push(cx.subscribe_in(input, window, |this, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.changed(window, cx);
                }
            }));
        }
        // ↩ in the subject goes on to the text
        subscriptions.push(cx.subscribe_in(&subject, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.focus_text(window, cx);
            }
        }));
        // going away must not cost the last words: quitting waits for them
        subscriptions.push(cx.on_release(|this, cx| drop(this.save_last_edits(cx))));
        subscriptions.push(cx.on_app_quit(|this, cx| {
            if let Some(saved) = this.save_last_edits(cx) {
                let _ = saved.recv_timeout(QUIT_WAIT);
            }
            async {}
        }));
        // shown by the reader until now: the text field gets it once its page is there, and the
        // keyboard goes to the fields meanwhile
        if let Some(view) = &view {
            view.hide(cx);
        }
        Self {
            account,
            mailbox,
            draft: None,
            known: Arc::default(),
            gone: false,
            loading: false,
            to,
            cc,
            bcc,
            subject,
            show_cc: false,
            attachments: Vec::new(),
            view,
            page: editor::next_page(),
            page_dark: None,
            page_ready: false,
            focus_text: false,
            body_html: String::new(),
            saved_html: String::new(),
            pictures: Vec::new(),
            picture_names: Vec::new(),
            waiting: Vec::new(),
            uploading: 0,
            attaching: false,
            has_markers: true,
            signature: None,
            quote: None,
            dirty: false,
            saving: false,
            sending: false,
            send_after_flush: false,
            send_after_save: false,
            edits: 0,
            error: None,
            cover: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    /// A new, empty mail.
    pub fn blank(
        account: usize,
        mailbox: Arc<Mailbox>,
        view: Option<MailView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::base(account, mailbox, view, window, cx);
        this.signature = this.signature_html(false);
        this.load_page(cx);
        this.to.update(cx, |s, cx| s.focus(window, cx));
        this
    }

    /// The account signature a new mail (`answer == false`) or a reply gets.
    fn signature_html(&self, answer: bool) -> Option<String> {
        let account = &self.mailbox.account;
        if answer { account.reply_signature_block() } else { account.signature_block() }
    }

    /// An existing draft, fetched first.
    pub fn existing(
        account: usize,
        mailbox: Arc<Mailbox>,
        view: Option<MailView>,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::base(account, mailbox.clone(), view, window, cx);
        this.loading = true;
        let fetch = move || {
            let _turn = turn();
            mailbox.message(&id)
        };
        this.run(window, cx, fetch, |this, result, window, cx| {
            this.loading = false;
            match result {
                Ok(message) => this.fill(message, window, cx),
                Err(e) => this.error = Some(e.to_string()),
            }
        });
        this
    }

    /// A draft that was just created (reply, forward).
    pub fn loaded(
        account: usize,
        mailbox: Arc<Mailbox>,
        view: Option<MailView>,
        draft: Message,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::base(account, mailbox, view, window, cx);
        let forward = draft.to_recipients.is_empty();
        this.focus_text = !forward;
        this.fill(draft, window, cx);
        if forward {
            this.to.update(cx, |s, cx| s.focus(window, cx));
        }
        this
    }

    /// Account and id of the draft, if it can be fetched again without losing anything: saved,
    /// and nothing on its way to the server.
    pub fn reloadable(&self) -> Option<(String, String)> {
        let settled = !self.dirty && !self.loading && !self.saving && !self.sending && self.uploading == 0;
        let draft = self.draft.as_ref().filter(|_| settled)?;
        Some((self.mailbox.account.id.clone(), draft.id.clone()))
    }

    fn run<T: Send + 'static>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> jm_core::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, jm_core::Result<T>, &mut Window, &mut Context<Self>) + 'static,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_executor().spawn(async move { work() }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                done(this, result, window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn fill(&mut self, message: Message, window: &mut Window, cx: &mut Context<Self>) {
        let document = message.body.as_ref().map(|b| b.content.clone()).unwrap_or_default();
        let author = html::author_html(&document).map(str::to_string);
        let markers = author.is_some();
        // written elsewhere: its body, without the looks Outlook or Word gave it
        let text = author.unwrap_or_else(|| html::clean_for_editing(&document, true));
        // arrow keys once typed control characters into the text: a Just Mail draft saves again
        // without them
        self.body_html = html::without_control_characters(&text);
        self.saved_html = if markers { text } else { self.body_html.clone() };
        if !html::content_ids(&self.body_html).is_empty() {
            let mailbox = self.mailbox.clone();
            let (id, body) = (message.id.clone(), self.body_html.clone());
            self.run(window, cx, move || mailbox.inline_images(&id, &body), |this, result, _, _| {
                let found: Vec<(String, String)> =
                    result.unwrap_or_default().iter().map(|i| (i.content_id.to_lowercase(), html::data_url(i))).collect();
                this.show_pictures(&found);
                this.pictures.extend(found);
            });
        }
        self.has_markers = markers;
        self.show_cc = !message.cc_recipients.is_empty() || !message.bcc_recipients.is_empty();
        let subject = message.subject.clone().unwrap_or_default();
        // A Just Mail draft shows what follows the text: the signature, then a reply's quote.
        let quote = html::quote_html(&document).filter(|_| markers);
        self.quote = quote.map(|q| html::to_web_document(q, &[]));
        let answer = quote.is_some() || is_answer_subject(&subject);
        self.signature = if markers && html::has_signature(&document) { self.signature_html(answer) } else { None };
        self.to.update(cx, |f, cx| f.set(&message.to_recipients, window, cx));
        self.cc.update(cx, |f, cx| f.set(&message.cc_recipients, window, cx));
        self.bcc.update(cx, |f, cx| f.set(&message.bcc_recipients, window, cx));
        self.subject.update(cx, |s, cx| s.set_value(subject, window, cx));
        if message.has_attachments {
            let mailbox = self.mailbox.clone();
            let id = message.id.clone();
            self.run(window, cx, move || mailbox.attachments(&id), |this, result, _, _| {
                if let Ok(list) = result {
                    this.attachments = list.into_iter().filter(|a| !a.is_inline).collect();
                }
            });
        }
        *self.known.lock().unwrap_or_else(PoisonError::into_inner) = Some(message.id.clone());
        self.draft = Some(message);
        if self.body_html != self.saved_html {
            self.changed(window, cx);
        }
        self.upload_waiting(window, cx);
        self.load_page(cx);
        cx.notify();
    }

    /// Show the text in the editor's page; again after a change of the theme, with what was
    /// typed so far.
    fn load_page(&mut self, cx: &mut Context<Self>) {
        let Some(view) = &self.view else { return };
        let palette = Palette::of(cx);
        let content = editor::Content {
            page: self.page,
            html: &self.body_html,
            pictures: &self.pictures,
            signature: self.signature.as_deref(),
            quote: self.quote.as_deref(),
            focus: self.focus_text,
        };
        view.load(&editor::page(&content, &palette));
        if std::mem::take(&mut self.focus_text) {
            view.focus(cx);
        }
        self.page_dark = Some(palette.dark);
        self.page_ready = false;
    }

    /// Pictures of the text whose bytes came: shown, if the page is there (else it gets them
    /// once it is, see `EditorEvent::Ready`).
    fn show_pictures(&self, found: &[(String, String)]) {
        if let Some(view) = self.view.as_ref().filter(|_| self.page_ready && !found.is_empty()) {
            let map: std::collections::HashMap<&str, &str> = found.iter().map(|(id, url)| (id.as_str(), url.as_str())).collect();
            view.evaluate(&editor::call("pictures", map));
        }
    }

    /// What the editor's page reports.
    pub fn on_editor(&mut self, message: EditorMessage, window: &mut Window, cx: &mut Context<Self>) {
        if message.page != self.page {
            return; // an earlier draft's page, still talking
        }
        match message.event {
            EditorEvent::Ready => {
                self.page_ready = true;
                self.show_pictures(&self.pictures);
            }
            EditorEvent::Edited { html } => {
                if html != self.body_html {
                    self.body_html = html;
                    self.changed(window, cx);
                }
            }
            // the keyboard is in the text: ⌘↩ and ⌘S are the composer's, ⌘V no field's
            EditorEvent::Focused => self.focus.focus(window, cx),
            EditorEvent::Paste { plain } => self.paste(plain, window, cx),
            EditorEvent::Picture => self.pick_pictures(window, cx),
            EditorEvent::Leave => {
                // the web view would keep the keys
                if let Some(view) = &self.view {
                    view.release_keyboard(cx);
                }
                self.subject.update(cx, |s, cx| s.focus(window, cx));
            }
            EditorEvent::Flushed => {
                if std::mem::take(&mut self.send_after_flush) {
                    self.send_checked(window, cx);
                }
            }
        }
    }

    /// Put the keyboard into the text.
    fn focus_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = &self.view {
            self.focus.focus(window, cx);
            view.focus(cx);
            view.evaluate(&editor::call("focus", ()));
        }
    }

    fn changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dirty = true;
        self.edits += 1;
        let edits = self.edits;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(AUTOSAVE).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.edits == edits && this.dirty && !this.saving && !this.sending {
                    this.save(false, window, cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn value(input: &Entity<InputState>, cx: &App) -> String {
        input.read(cx).value().trim().to_string()
    }

    /// Recipients of all three fields; `strict` reports a typo instead of skipping the field.
    fn recipients(&self, cx: &App, strict: bool) -> Result<[Option<Vec<Recipient>>; 3], String> {
        let mut out: [Option<Vec<Recipient>>; 3] = [None, None, None];
        for (i, field) in [&self.to, &self.cc, &self.bcc].into_iter().enumerate() {
            match field.read(cx).recipients(cx) {
                Ok(list) => out[i] = Some(list),
                Err(e) if strict => return Err(e.message),
                Err(_) => {} // still typing: keep what the server has
            }
        }
        Ok(out)
    }

    /// The draft as it is to be saved; `None` for a new mail with nothing worth a draft yet.
    /// With `strict` an invalid address is an error instead of skipped.
    fn draft_input(&self, cx: &App, strict: bool) -> Result<Option<DraftInput>, String> {
        let [to, cc, bcc] = self.recipients(cx, strict)?;
        let subject = Self::value(&self.subject, cx);
        let is_new = self.draft.is_none();
        let no_text = html::html_to_text(&self.body_html).trim().is_empty() && html::content_ids(&self.body_html).is_empty();
        if is_new && no_text && subject.is_empty() && to.as_ref().is_none_or(|t| t.is_empty()) && self.waiting.is_empty() {
            return Ok(None);
        }
        let body_html = (is_new || self.body_html != self.saved_html).then(|| self.body_html.clone());
        Ok(Some(DraftInput { to, cc, bcc, subject: Some(subject), body_html, importance: None }))
    }

    /// The composer goes away (another mail was opened, the app quits): what was typed since
    /// the last save, and what still waits to go into the draft, goes there on a thread of its
    /// own. The receiver hears when that is done; `None` if there is nothing to save.
    fn save_last_edits(&mut self, cx: &App) -> Option<mpsc::Receiver<()>> {
        if self.gone || self.loading || (!self.dirty && self.waiting.is_empty()) {
            return None;
        }
        let input = self.draft_input(cx, false).ok().flatten()?;
        self.dirty = false;
        let uploads = std::mem::take(&mut self.waiting);
        let (mailbox, known) = (self.mailbox.clone(), self.known.clone());
        let (done, heard) = mpsc::channel();
        std::thread::spawn(move || {
            if let Ok(draft) = write_draft(&mailbox, &known, &input) {
                let _turn = turn();
                for upload in uploads {
                    let _ = upload.send(&mailbox, &draft.id);
                }
                // the list shows what the draft says now
                jm_core::events::emit(&jm_core::events::Event::Changed {
                    account: mailbox.id().to_string(),
                    folder: Some("drafts".into()),
                });
            }
            let _ = done.send(());
        });
        Some(heard)
    }

    /// Save to the server; with `explicit` an invalid address is an error instead of skipped.
    fn save(&mut self, explicit: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.attaching || self.loading {
            return; // its turn comes (see `settle`)
        }
        let input = match self.draft_input(cx, explicit) {
            Ok(Some(input)) => input,
            Ok(None) => return, // nothing worth a draft yet
            Err(e) => {
                self.error = Some(e);
                self.send_after_save = false;
                cx.notify();
                return;
            }
        };
        let body_html = self.body_html.clone();
        self.saving = true;
        self.error = None;
        let edits = self.edits;
        let (mailbox, known) = (self.mailbox.clone(), self.known.clone());
        cx.notify();
        self.run(
            window,
            cx,
            move || write_draft(&mailbox, &known, &input),
            move |this, result, window, cx| {
                this.saving = false;
                match result {
                    Ok(message) => {
                        this.saved_html = body_html;
                        if this.edits == edits {
                            this.dirty = false;
                        }
                        this.has_markers = html::author_html(message.body.as_ref().map_or("", |b| &b.content)).is_some();
                        this.draft = Some(message.clone());
                        cx.emit(ComposerEvent::Saved { account: this.account, message: Box::new(message) });
                        jm_core::events::emit(&jm_core::events::Event::Changed {
                            account: this.mailbox.id().to_string(),
                            folder: Some("drafts".into()),
                        });
                        this.settle(window, cx);
                    }
                    Err(e) => {
                        this.send_after_save = false;
                        this.error = Some(e.to_string());
                    }
                }
            },
        );
    }

    /// Send right away (no confirmation), once the page reported the last keystrokes.
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending || self.loading || self.send_after_flush {
            return;
        }
        let Some(view) = &self.view else {
            self.send_checked(window, cx);
            return;
        };
        self.send_after_flush = true;
        view.evaluate(&editor::call("flush", ()));
        // a page that does not answer must not keep the mail from going out
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(FLUSH_WAIT).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if std::mem::take(&mut this.send_after_flush) {
                    this.send_checked(window, cx);
                }
            });
        })
        .detach();
    }

    /// Send, saving pending edits first.
    fn send_checked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending || self.loading {
            return;
        }
        let [to, cc, bcc] = match self.recipients(cx, true) {
            Ok(r) => r,
            Err(e) => {
                self.error = Some(e);
                cx.notify();
                return;
            }
        };
        let nobody = |list: &Option<Vec<Recipient>>| list.as_ref().is_none_or(|l| l.is_empty());
        if nobody(&to) && nobody(&cc) && nobody(&bcc) {
            self.error = Some(tr!("Add at least one recipient.", "Mindestens einen Empfänger eintragen.").into());
            cx.notify();
            return;
        }
        self.error = None;
        let unsaved = self.dirty || self.draft.is_none();
        if unsaved || self.saving || self.uploading > 0 {
            self.send_after_save = true;
            if unsaved {
                self.save(true, window, cx);
            }
        } else {
            self.send_now(window, cx);
        }
    }

    /// A save or the uploads ended: what waited for its turn goes on. First what is to go into
    /// the draft, then a save of what was typed meanwhile, then a send that waited for both.
    fn settle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.attaching {
            return;
        }
        if !self.waiting.is_empty() {
            self.upload_waiting(window, cx);
        } else if self.dirty {
            self.save(false, window, cx);
        } else if self.send_after_save && self.uploading == 0 {
            self.send_after_save = false;
            self.send_checked(window, cx);
        }
        cx.notify();
    }

    fn send_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.clone() else { return };
        self.sending = true;
        let mailbox = self.mailbox.clone();
        let id = draft.id.clone();
        let recipients =
            draft.to_recipients.iter().chain(&draft.cc_recipients).map(|r| r.display().to_string()).collect::<Vec<_>>().join(", ");
        cx.notify();
        let send = move || {
            mailbox.drop_unused_images(&id)?;
            mailbox.send_draft(&id)
        };
        self.run(window, cx, send, move |this, result, _, cx| {
            this.sending = false;
            match result {
                Ok(()) => {
                    this.gone = true;
                    jm_core::events::emit(&jm_core::events::Event::Changed {
                        account: this.mailbox.id().to_string(),
                        folder: Some("sentitems".into()),
                    });
                    cx.emit(ComposerEvent::Sent { account: this.account, id: draft.id.clone(), recipients });
                }
                Err(e) => this.error = Some(format!("{}: {e}", tr!("Not sent", "Nicht gesendet"))),
            }
        });
    }

    fn discard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.clone() else {
            cx.emit(ComposerEvent::Discarded { account: self.account, id: None });
            return;
        };
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            let draft = draft.clone();
            alert
                .title(tr!("Delete this draft?", "Diesen Entwurf löschen?"))
                .show_cancel(true)
                .ok_text(tr!("Delete", "Löschen"))
                .cancel_text(tr!("Cancel", "Abbrechen"))
                .on_ok(move |_, window, cx| {
                    let id = draft.id.clone();
                    let work_id = id.clone();
                    let _ = this.update(cx, |c, cx| {
                        let mailbox = c.mailbox.clone();
                        c.sending = true; // blocks autosave
                        c.run(window, cx, move || mailbox.delete_draft(&work_id), move |c, result, _, cx| match result {
                            Ok(()) => {
                                c.gone = true;
                                cx.emit(ComposerEvent::Discarded { account: c.account, id: Some(id) });
                            }
                            Err(e) => {
                                c.sending = false;
                                c.error = Some(e.to_string());
                            }
                        });
                    });
                    true
                })
        });
    }

    fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_files(window, cx, Self::add_files);
    }

    /// The picture button of the text: pictures go into the text (anything else is attached).
    fn pick_pictures(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_files(window, cx, Self::add_dropped);
    }

    /// Ask for files and hand them to `then`.
    fn pick_files(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: fn(&mut Self, Vec<PathBuf>, &mut Window, &mut Context<Self>),
    ) {
        let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let _ = this.update_in(cx, |this, window, cx| then(this, paths, window, cx));
        })
        .detach();
    }

    /// Attach files: read first (off the UI thread), then they go up once the draft exists.
    fn add_files(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let count = paths.len();
        self.uploading += count;
        cx.notify();
        let read = move || paths.iter().map(|path| NewAttachment::from_path(path)).collect::<jm_core::Result<Vec<_>>>();
        self.run(window, cx, read, move |this, result, window, cx| match result {
            Ok(files) => {
                this.waiting.extend(files.into_iter().map(Upload::File));
                this.upload_waiting(window, cx);
            }
            Err(e) => {
                this.upload_failed(e.to_string());
                this.uploading = this.uploading.saturating_sub(count);
                this.settle(window, cx);
            }
        });
    }

    /// Files pasted, dropped on the text or picked for it: pictures go into the text where the
    /// cursor is, anything else is attached.
    fn add_dropped(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let (pictures, files): (Vec<_>, Vec<_>) = paths.into_iter().partition(|path| picture::is_picture(path));
        for path in pictures {
            let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            self.add_picture(name, move || picture::from_file(&path), window, cx);
        }
        if !files.is_empty() {
            self.add_files(files, window, cx);
        }
    }

    /// Files are dragged over the text (`on`) or left it again.
    pub fn on_dragging(&mut self, on: bool) {
        if let Some(view) = &self.view {
            view.evaluate(&editor::call("dropping", on));
        }
    }

    /// Files were dropped on the text at `(x, y)` of its page: the cursor goes there.
    pub fn on_dropped(&mut self, paths: Vec<PathBuf>, (x, y): (i32, i32), window: &mut Window, cx: &mut Context<Self>) {
        self.on_dragging(false);
        if let Some(view) = &self.view {
            view.evaluate(&editor::call("caret", (x, y)));
        }
        self.add_dropped(paths, window, cx);
    }

    /// ⌘V in the text: copied files as if dropped, a picture goes in where the cursor is, HTML
    /// without the looks it brought along, a web address as a link, text as text (`plain`: text
    /// only, whatever else there is).
    fn paste(&mut self, plain: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.view.clone() else { return };
        let (mut paths, mut images, mut text) = (Vec::new(), Vec::new(), None);
        for entry in cx.read_from_clipboard().iter().flat_map(|item| item.entries().to_vec()) {
            match entry {
                ClipboardEntry::ExternalPaths(p) => paths.extend_from_slice(p.paths()),
                ClipboardEntry::Image(image) => images.push(image),
                ClipboardEntry::String(s) => text = Some(s.text().clone()),
            }
        }
        if plain {
            if let Some(text) = text {
                view.evaluate(&editor::call("insert", html::text_to_html(&text)));
            }
            return;
        }
        if !paths.is_empty() {
            self.add_dropped(paths, window, cx);
            return;
        }
        // a picture copied from a web page comes as HTML too, with nothing but the picture in it
        let html = editor::pasteboard_html()
            .map(|h| html::clean_for_editing(&h, false))
            .filter(|h| !html::html_to_text(h).trim().is_empty());
        if html.is_none() && !images.is_empty() {
            for image in &images {
                let (format, bytes) = (image.format(), image.bytes().to_vec());
                self.add_picture(self.picture_stem(), move || picture::prepare(format, bytes), window, cx);
            }
        } else if let Some(markup) = html {
            view.evaluate(&editor::call("insert", markup));
        } else if let Some(text) = text {
            match web_address(&text) {
                Some(url) => view.evaluate(&editor::call("link", url)),
                None => view.evaluate(&editor::call("insert", html::text_to_html(&text))),
            }
        }
    }

    /// The picture `prepare` makes (converted, shrunk if need be) goes into the text once it is
    /// ready, as `name` plus the extension of what it became, and into the draft as soon as
    /// there is one.
    fn add_picture(
        &mut self,
        name: String,
        prepare: impl FnOnce() -> Result<picture::Prepared, String> + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picture_names.push(name.clone());
        self.uploading += 1;
        cx.notify();
        self.run(window, cx, move || prepare().map_err(jm_core::Error::general), move |this, result, window, cx| match result {
            Ok(prepared) => {
                let content_id = format!("picture.{}@{}", prepared.extension, html::content_id_token());
                let image = InlineImage {
                    content_id: content_id.clone(),
                    name: format!("{name}.{}", prepared.extension),
                    content_type: prepared.content_type.into(),
                    bytes: prepared.bytes,
                };
                let shown = (content_id.to_lowercase(), html::data_url(&image));
                // shown from its bytes, `cid:` in what the page reports
                this.show_pictures(std::slice::from_ref(&shown));
                if let Some(view) = &this.view {
                    view.evaluate(&editor::call("insert", html::picture_tag(&image.name, &content_id, prepared.size)));
                }
                this.pictures.push(shown);
                this.waiting.push(Upload::Picture(image));
                this.upload_waiting(window, cx);
            }
            Err(e) => {
                this.upload_failed(format!("{name}: {e}"));
                this.uploading = this.uploading.saturating_sub(1);
                this.settle(window, cx);
            }
        });
    }

    /// `Bild 1`, `Bild 2`, …: a name for a pasted picture the draft doesn't use yet.
    fn picture_stem(&self) -> String {
        let stem = tr!("Picture", "Bild");
        (1..)
            .map(|n| format!("{stem} {n}"))
            .find(|name| {
                let alt = format!("alt=\"{}.", html_escape::encode_double_quoted_attribute(name));
                !self.body_html.contains(&alt) && !self.picture_names.contains(name)
            })
            .unwrap_or_default()
    }

    /// Send what waits into the draft, one after the other, once the draft exists (saving it
    /// first if need be) and no save is on its way.
    fn upload_waiting(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.waiting.is_empty() || self.attaching || self.saving || self.loading {
            return;
        }
        let Some(id) = self.draft.as_ref().map(|d| d.id.clone()) else {
            self.save(false, window, cx);
            return;
        };
        let uploads = std::mem::take(&mut self.waiting);
        let mailbox = self.mailbox.clone();
        self.attaching = true;
        cx.notify();
        let work = move || {
            let _turn = turn();
            Ok(uploads.into_iter().map(|upload| upload.send(&mailbox, &id)).collect::<Vec<_>>())
        };
        self.run(window, cx, work, |this, done, window, cx| {
            this.attaching = false;
            for (name, picture, added) in done.unwrap_or_default() {
                this.uploading = this.uploading.saturating_sub(1);
                match added {
                    Ok(attachment) => this.attachments.extend(attachment),
                    Err(e) => {
                        this.upload_failed(format!("{name}: {e}"));
                        // out of the text: the mail would show it broken
                        if let (Some(view), Some(content_id)) = (&this.view, picture) {
                            view.evaluate(&editor::call("drop", content_id));
                        }
                    }
                }
            }
            this.settle(window, cx);
        });
    }

    fn upload_failed(&mut self, error: String) {
        self.error = Some(error);
        self.send_after_save = false;
    }

    fn remove_attachment(&mut self, attachment_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = &self.draft else { return };
        let mailbox = self.mailbox.clone();
        let id = draft.id.clone();
        self.attachments.retain(|a| a.id != attachment_id);
        cx.notify();
        self.run(window, cx, move || mailbox.remove_attachment(&id, &attachment_id), |this, result, _, _| {
            if let Err(e) = result {
                this.error = Some(e.to_string());
            }
        });
    }

    fn open_attachment(&mut self, attachment: Attachment, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = &self.draft else { return };
        let mailbox = self.mailbox.clone();
        let id = draft.id.clone();
        self.run(window, cx, move || util::save_attachment(&mailbox, &id, &attachment), |this, result, _, cx| match result {
            Ok(path) => cx.open_with_system(&path),
            Err(e) => this.error = Some(e.to_string()),
        });
    }

    fn status(&self) -> String {
        if self.sending {
            tr!("Sending…", "Wird gesendet…").into()
        } else if self.saving {
            tr!("Saving…", "Speichert…").into()
        } else if self.uploading > 0 {
            tr!("Uploading…", "Lädt hoch…").into()
        } else if self.dirty {
            tr!("Edited", "Bearbeitet").into()
        } else if self.draft.is_some() {
            tr!("Saved in Drafts", "In Entwürfen gespeichert").into()
        } else {
            String::new()
        }
    }
}

impl Focusable for Composer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn field_row(label: &'static str, input: impl IntoElement, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .px_5()
        .py_1()
        .gap_3()
        .items_center()
        .border_b_1()
        .border_color(theme.border.opacity(0.6))
        .child(div().w(px(64.)).flex_none().text_size(theme::TEXT).text_color(theme.muted_foreground).child(label))
        .child(div().flex_1().min_w_0().child(input))
}

/// A header field without a frame, its text in line with the From address above it.
fn field(state: &Entity<InputState>) -> Input {
    Input::new(state).appearance(false).px_0().text_size(theme::TEXT)
}

impl Render for Composer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let busy = self.sending || self.loading;
        let account = self.mailbox.account.email.clone();
        // the page has the colors of the theme it was made for
        if self.page_dark.is_some_and(|dark| dark != theme.mode.is_dark()) {
            self.load_page(cx);
        }
        // GPUI cannot draw over the web view: while suggestions hang over it, it stays hidden
        let suggesting = [&self.to, &self.cc, &self.bcc].iter().any(|f| f.read(cx).suggesting());
        if !suggesting {
            self.cover = None;
        } else if self.cover.is_none() {
            self.cover = self.view.as_ref().and_then(|v| v.cover(px(f32::MAX)));
        }

        let toolbar = h_flex()
            .h(px(TOP_H))
            .flex_none()
            .px_4()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .window_control_area(WindowControlArea::Drag)
            .child(
                Button::new("send")
                    .primary()
                    .small()
                    .icon(IconName::Send)
                    .label(tr!("Send", "Senden"))
                    .disabled(busy)
                    .tooltip(tr!("Send now (⌘↩)", "Jetzt senden (⌘↩)"))
                    .on_click(cx.listener(|this, _, w, cx| this.send(w, cx))),
            )
            .child(
                Button::new("attach")
                    .ghost()
                    .small()
                    .icon(IconName::Paperclip)
                    .tooltip(tr!("Attach files", "Dateien anhängen"))
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, w, cx| this.attach(w, cx))),
            )
            .child(
                Button::new("cc")
                    .ghost()
                    .small()
                    .label("Cc/Bcc")
                    .selected(self.show_cc)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_cc = !this.show_cc;
                        cx.notify();
                    })),
            )
            .child(div().flex_1().text_size(theme::TEXT_SMALL).text_color(theme.muted_foreground).child(self.status()))
            .child(
                Button::new("save")
                    .ghost()
                    .small()
                    .icon(IconName::Save)
                    .tooltip(tr!("Save now (⌘S)", "Jetzt speichern (⌘S)"))
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, w, cx| this.save(true, w, cx))),
            )
            .child(
                Button::new("discard")
                    .ghost()
                    .small()
                    .icon(IconName::Trash)
                    .tooltip(tr!("Delete draft", "Entwurf löschen"))
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, w, cx| this.discard(w, cx))),
            );

        let mut attachments = h_flex().px_5().py_2().gap_2().flex_wrap();
        for (i, a) in self.attachments.iter().enumerate() {
            let id = a.id.clone();
            let att = a.clone();
            attachments = attachments.child(
                h_flex()
                    .id(("att", i))
                    .gap_1p5()
                    .pl_2p5()
                    .pr_1()
                    .py_0p5()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(theme.border)
                    .text_xs()
                    .items_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.secondary_hover))
                    .child(Icon::new(IconName::Paperclip).xsmall())
                    .child(a.name.clone())
                    .child(div().text_color(theme.muted_foreground).child(util::size(a.size)))
                    .child(
                        Button::new(SharedString::from(format!("rm-{}", a.id)))
                            .icon(IconName::Close)
                            .ghost()
                            .xsmall()
                            .tooltip(tr!("Remove", "Entfernen"))
                            .on_click(cx.listener(move |this, _, w, cx| this.remove_attachment(id.clone(), w, cx))),
                    )
                    .tooltip({
                        let tip = tr!("Download and open", "Herunterladen und öffnen");
                        move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx)
                    })
                    .on_click(cx.listener(move |this, _, w, cx| this.open_attachment(att.clone(), w, cx))),
            );
        }

        // the mail as it goes out: your text, the signature, then a reply's quote
        let text = match &self.view {
            Some(view) if !self.loading => view.element().into_any_element(),
            Some(_) => h_flex()
                .size_full()
                .justify_center()
                .items_center()
                .child(Icon::new(IconName::LoaderCircle).text_color(theme.muted_foreground))
                .into_any_element(),
            None => div()
                .p_5()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(tr!("The text needs WebKit, which could not start.", "Der Text braucht WebKit, das nicht starten konnte."))
                .into_any_element(),
        };

        v_flex()
            .key_context(actions::COMPOSER)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SendDraft, w, cx| this.send(w, cx)))
            .on_action(cx.listener(|this, _: &SaveDraft, w, cx| this.save(true, w, cx)))
            .size_full()
            .child(toolbar)
            .when_some(self.error.clone(), |d, e| {
                d.child(div().px_5().py_2().bg(theme.danger.opacity(0.12)).text_sm().text_color(theme.danger).child(e))
            })
            .when(!self.has_markers, |d| {
                d.child(div().px_5().py_2().bg(theme.warning.opacity(0.12)).text_xs().text_color(theme.warning).child(tr!(
                    "This draft was not written in Just Mail: editing the text saves it anew, with Just Mail's signature and without Outlook's looks.",
                    "Dieser Entwurf stammt nicht aus Just Mail: Änderungen am Text speichern ihn neu, mit der Signatur aus Just Mail und ohne Outlooks Gestaltung."
                )))
            })
            .child(field_row(tr!("From", "Von"), h_flex().h_8().text_size(theme::TEXT).child(account), cx))
            .child(field_row(tr!("To", "An"), self.to.clone(), cx))
            .when(self.show_cc, |d| {
                d.child(field_row("Cc", self.cc.clone(), cx)).child(field_row("Bcc", self.bcc.clone(), cx))
            })
            .child(field_row(
                tr!("Subject", "Betreff"),
                // ⇥ goes on to the text, which GPUI's focus order doesn't know
                div().capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                    this.focus_text(window, cx);
                    cx.stop_propagation();
                }))
                .child(field(&self.subject)),
                cx,
            ))
            .when(!self.attachments.is_empty(), |d| d.child(attachments))
            .child(div().flex_1().min_h_0().child(text))
    }
}

/// The text, if it is nothing but one web address.
fn web_address(text: &str) -> Option<&str> {
    let text = text.trim();
    let web = text.starts_with("https://") || text.starts_with("http://");
    (web && !text.contains(char::is_whitespace)).then_some(text)
}

/// Subject of a reply or forward ("RE:", "AW:", "FW:", "WG:" …).
fn is_answer_subject(subject: &str) -> bool {
    let subject = subject.trim_start().to_lowercase();
    ["re:", "aw:", "fw:", "fwd:", "wg:", "antw:"].iter().any(|p| subject.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::web_address;

    #[test]
    fn a_pasted_text_is_a_link_when_it_is_one_web_address() {
        assert_eq!(web_address(" https://example.com/a?b=1\n"), Some("https://example.com/a?b=1"));
        assert_eq!(web_address("see https://example.com"), None);
        assert_eq!(web_address("https://example.com and more"), None);
        assert_eq!(web_address("javascript:alert(1)"), None);
    }
}
