//! The draft editor. Every draft lives on the server (Drafts folder), so the CLI, Outlook and
//! this window all see the same thing. Edits are saved two seconds after the last keystroke.
//!
//! The text is rich text in WebKit (see `editor`): it holds only the author's part, as HTML, so
//! bold, lists, tables and pictures stay as they are, whoever wrote them (an agent through `jm`
//! too). The signature and a reply's quote stay in the draft's HTML untouched (see
//! `jm_core::html`). A pasted picture goes in where the cursor is and out as an inline image of
//! the draft.

use std::sync::Arc;
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
    /// Names given to pasted pictures (`Bild 1.png`, …).
    picture_names: Vec<String>,
    /// Pasted pictures waiting for the draft to exist.
    waiting: Vec<InlineImage>,
    /// Pictures and files on their way into the draft.
    uploading: usize,
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
        // shown by the reader until now: the text field gets it once its page is there, and the
        // keyboard goes to the fields meanwhile
        if let Some(view) = &view {
            view.hide(cx);
        }
        Self {
            account,
            mailbox,
            draft: None,
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
        this.run(window, cx, move || mailbox.message(&id), |this, result, window, cx| {
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
        let draft = self.draft.as_ref().filter(|_| settled && self.waiting.is_empty())?;
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
        self.draft = Some(message);
        if self.body_html != self.saved_html {
            self.changed(window, cx);
        }
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
            EditorEvent::Paste => self.paste(window, cx),
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

    /// A toolbar button: `command` on the selection in the text, which keeps the keyboard.
    fn format(&mut self, command: &str, cx: &mut Context<Self>) {
        if let Some(view) = &self.view {
            view.focus(cx);
            view.evaluate(&editor::call("command", command));
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

    /// Save to the server; with `explicit` an invalid address is an error instead of skipped.
    fn save(&mut self, explicit: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.loading {
            return;
        }
        let [to, cc, bcc] = match self.recipients(cx, explicit) {
            Ok(r) => r,
            Err(e) => {
                self.error = Some(e);
                self.send_after_save = false;
                cx.notify();
                return;
            }
        };
        let body_html = self.body_html.clone();
        let subject = Self::value(&self.subject, cx);
        let is_new = self.draft.is_none();
        let no_text = html::html_to_text(&body_html).trim().is_empty() && html::content_ids(&body_html).is_empty();
        if is_new && no_text && subject.is_empty() && to.as_ref().is_none_or(|t| t.is_empty()) {
            return; // nothing worth a draft yet
        }
        let input = DraftInput {
            to,
            cc,
            bcc,
            subject: Some(subject),
            body_html: (is_new || body_html != self.saved_html).then(|| body_html.clone()),
            importance: None,
        };
        self.saving = true;
        self.error = None;
        let edits = self.edits;
        let mailbox = self.mailbox.clone();
        let id = self.draft.as_ref().map(|d| d.id.clone());
        cx.notify();
        self.run(
            window,
            cx,
            move || match id {
                Some(id) => mailbox.update_draft(&id, &input),
                None => mailbox.create_draft(&input, true, &[]),
            },
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
                        this.upload_waiting(window, cx);
                        if this.send_after_save && this.uploading == 0 {
                            this.send_after_save = false;
                            this.send_now(window, cx);
                        } else if this.dirty {
                            this.save(false, window, cx);
                        }
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
            if unsaved && !self.saving {
                self.save(true, window, cx);
            }
        } else {
            self.send_now(window, cx);
        }
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
                            Ok(()) => cx.emit(ComposerEvent::Discarded { account: c.account, id: Some(id) }),
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
        let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let _ = this.update_in(cx, |this, window, cx| this.add_files(paths, window, cx));
        })
        .detach();
    }

    fn add_files(&mut self, paths: Vec<std::path::PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = &self.draft else {
            self.error = Some(tr!(
                "Write something first, then attach (the draft has to exist).",
                "Erst etwas schreiben, dann anhängen (der Entwurf muss existieren)."
            ).into());
            self.save(false, window, cx);
            cx.notify();
            return;
        };
        let mailbox = self.mailbox.clone();
        let id = draft.id.clone();
        self.uploading += 1;
        cx.notify();
        self.run(
            window,
            cx,
            move || {
                let mut added = Vec::new();
                for path in paths {
                    let file = NewAttachment::from_path(&path)?;
                    added.push(mailbox.add_attachment(&id, &file)?);
                }
                Ok(added)
            },
            |this, result, window, cx| {
                match result {
                    Ok(added) => this.attachments.extend(added),
                    Err(e) => this.upload_failed(e.to_string()),
                }
                this.uploaded(window, cx);
            },
        );
    }

    /// ⌘V in the text: copied files become attachments, a picture goes in where the cursor is,
    /// HTML without the looks it brought along, text as text.
    fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.view.clone() else { return };
        let (mut paths, mut images, mut text) = (Vec::new(), Vec::new(), None);
        for entry in cx.read_from_clipboard().iter().flat_map(|item| item.entries().to_vec()) {
            match entry {
                ClipboardEntry::ExternalPaths(p) => paths.extend_from_slice(p.paths()),
                ClipboardEntry::Image(image) => images.push(image),
                ClipboardEntry::String(s) => text = Some(s.text().clone()),
            }
        }
        if !paths.is_empty() {
            self.add_files(paths, window, cx);
            return;
        }
        // a picture copied from a web page comes as HTML too, with nothing but the picture in it
        let html = editor::pasteboard_html()
            .map(|h| html::clean_for_editing(&h, false))
            .filter(|h| !html::html_to_text(h).trim().is_empty());
        if html.is_none() && !images.is_empty() {
            for image in &images {
                self.paste_picture(image, window, cx);
            }
        } else if let Some(markup) = html.or_else(|| text.map(|t| html::text_to_html(&t))) {
            view.evaluate(&editor::call("insert", markup));
        }
    }

    /// The picture goes into the text once it is ready (converted, shrunk if need be), and into
    /// the draft as soon as there is one.
    fn paste_picture(&mut self, image: &Image, window: &mut Window, cx: &mut Context<Self>) {
        let extension = picture::extension(image.format());
        let name = self.picture_name(extension);
        self.picture_names.push(name.clone());
        self.uploading += 1;
        let (format, bytes) = (image.format(), image.bytes().to_vec());
        let prepare = move || picture::prepare(format, bytes).map_err(jm_core::Error::general);
        self.run(window, cx, prepare, move |this, result, window, cx| match result {
            Ok(prepared) => {
                let content_id = format!("picture.{extension}@{}", html::content_id_token());
                let image =
                    InlineImage { content_id: content_id.clone(), name, content_type: prepared.content_type.into(), bytes: prepared.bytes };
                let url = html::data_url(&image);
                // shown from its bytes, `cid:` again in what the page reports
                let tag = html::picture_tag(&image.name, &content_id, prepared.size)
                    .replacen(&format!("src=\"cid:{content_id}\""), &format!("src=\"{url}\" data-cid=\"{content_id}\""), 1);
                if let Some(view) = &this.view {
                    view.evaluate(&editor::call("insert", tag));
                }
                this.pictures.push((content_id.to_lowercase(), url));
                this.waiting.push(image);
                this.upload_waiting(window, cx);
            }
            Err(e) => {
                this.upload_failed(format!("{name}: {e}"));
                this.uploaded(window, cx);
            }
        });
    }

    /// `Bild 1.png`, `Bild 2.png`, …: a name the draft doesn't use yet.
    fn picture_name(&self, extension: &str) -> String {
        let stem = tr!("Picture", "Bild");
        (1..)
            .map(|n| format!("{stem} {n}.{extension}"))
            .find(|name| {
                let alt = format!("alt=\"{}\"", html_escape::encode_double_quoted_attribute(name));
                !self.body_html.contains(&alt) && !self.picture_names.contains(name)
            })
            .unwrap_or_default()
    }

    /// Attach the pasted pictures once the draft exists (saving it first if need be).
    fn upload_waiting(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.waiting.is_empty() {
            return;
        }
        let Some(id) = self.draft.as_ref().map(|d| d.id.clone()) else {
            self.save(false, window, cx);
            return;
        };
        for image in std::mem::take(&mut self.waiting) {
            let mailbox = self.mailbox.clone();
            let id = id.clone();
            let (name, content_id) = (image.name.clone(), image.content_id.clone());
            self.run(window, cx, move || mailbox.add_inline_image(&id, &image), move |this, result, window, cx| {
                if let Err(e) = result {
                    this.upload_failed(format!("{name}: {e}"));
                    // out of the text: the mail would show it broken
                    if let Some(view) = &this.view {
                        view.evaluate(&editor::call("drop", content_id));
                    }
                }
                this.uploaded(window, cx);
            });
        }
    }

    fn upload_failed(&mut self, error: String) {
        self.error = Some(error);
        self.send_after_save = false;
    }

    /// An upload ended; a send that waited for the uploads goes on.
    fn uploaded(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.uploading = self.uploading.saturating_sub(1);
        if self.uploading == 0 && self.send_after_save && !self.saving {
            self.send_after_save = false;
            self.send_checked(window, cx);
        }
        cx.notify();
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

        let no_text = busy || self.view.is_none();
        let format = |id: &'static str, icon: IconName, tip: &'static str, command: &'static str| {
            Button::new(id)
                .ghost()
                .small()
                .icon(icon)
                .tooltip(tip)
                .disabled(no_text)
                .on_click(cx.listener(move |this, _, _, cx| this.format(command, cx)))
        };
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
            .child(div().w(px(1.)).h(px(18.)).mx_1().bg(theme.border))
            .child(
                h_flex()
                    .gap_0p5()
                    .child(format("bold", IconName::Bold, tr!("Bold (⌘B)", "Fett (⌘B)"), "bold"))
                    .child(format("italic", IconName::Italic, tr!("Italic (⌘I)", "Kursiv (⌘I)"), "italic"))
                    .child(format("underline", IconName::Underline, tr!("Underline (⌘U)", "Unterstrichen (⌘U)"), "underline"))
                    .child(format("bullets", IconName::List, tr!("Bulleted list (⇧⌘8)", "Aufzählung (⇧⌘8)"), "insertUnorderedList"))
                    .child(format(
                        "numbers",
                        IconName::ListOrdered,
                        tr!("Numbered list (⇧⌘7)", "Nummerierte Liste (⇧⌘7)"),
                        "insertOrderedList",
                    )),
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

/// Subject of a reply or forward ("RE:", "AW:", "FW:", "WG:" …).
fn is_answer_subject(subject: &str) -> bool {
    let subject = subject.trim_start().to_lowercase();
    ["re:", "aw:", "fw:", "fwd:", "wg:", "antw:"].iter().any(|p| subject.starts_with(p))
}
