//! The draft editor. Every draft lives on the server (Drafts folder), so the CLI, Outlook and
//! this window all see the same thing. Edits are saved two seconds after the last keystroke;
//! sending always asks first.
//!
//! The body field holds only the author's part: the signature and a reply's quote stay in the
//! draft's HTML untouched (see `jm_core::html`).

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme, Disableable, Icon, Selectable as _, Sizable, WindowExt};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use jm_core::{Attachment, DraftInput, Mailbox, Message, NewAttachment, Recipient, html};

use crate::actions::{self, SaveDraft, SendDraft};
use crate::sidebar::TOP_H;
use crate::signature::Preview;
use crate::{tr, util};

const AUTOSAVE: Duration = Duration::from_secs(2);

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
    to: Entity<InputState>,
    cc: Entity<InputState>,
    bcc: Entity<InputState>,
    subject: Entity<InputState>,
    body: Entity<TextareaState>,
    show_cc: bool,
    attachments: Vec<Attachment>,
    /// Draft written by Just Mail (body field = author's part only).
    has_markers: bool,
    /// The signature as it goes out, shown below the text (long for new mails, short for answers).
    signature: Option<Preview>,
    /// The quoted mail of a reply or forward (Markdown), shown below the signature on request.
    quote: Option<SharedString>,
    show_quote: bool,
    /// Body text as last saved, to tell whether the body needs saving.
    saved_body: String,
    dirty: bool,
    saving: bool,
    sending: bool,
    /// Send as soon as the running save finishes.
    send_after_save: bool,
    /// Bumped on every edit; autosave only fires for the latest one.
    edits: u64,
    error: Option<String>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl Composer {
    fn base(account: usize, mailbox: Arc<Mailbox>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let to = field(tr!("Recipients", "Empfänger"), window, cx);
        let cc = field(tr!("Copy", "Kopie"), window, cx);
        let bcc = field(tr!("Blind copy", "Blindkopie"), window, cx);
        let subject = field(tr!("Subject", "Betreff"), window, cx);
        let body = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(8, 100_000).placeholder(tr!("Write your message…", "Deine Nachricht…"))
        });
        let mut subscriptions = Vec::new();
        for input in [&to, &cc, &bcc, &subject] {
            subscriptions.push(cx.subscribe_in(input, window, |this, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.changed(window, cx);
                }
            }));
        }
        subscriptions.push(cx.subscribe_in(&body, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::Change) {
                this.changed(window, cx);
            }
        }));
        Self {
            account,
            mailbox,
            draft: None,
            loading: false,
            to,
            cc,
            bcc,
            subject,
            body,
            show_cc: false,
            attachments: Vec::new(),
            has_markers: true,
            signature: None,
            quote: None,
            show_quote: false,
            saved_body: String::new(),
            dirty: false,
            saving: false,
            sending: false,
            send_after_save: false,
            edits: 0,
            error: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    /// A new, empty mail.
    pub fn blank(account: usize, mailbox: Arc<Mailbox>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self::base(account, mailbox, window, cx);
        this.signature = this.signature_preview(false, "new");
        this.to.update(cx, |s, cx| s.focus(window, cx));
        this
    }

    /// Preview of the account signature a new mail (`answer == false`) or a reply gets.
    fn signature_preview(&self, answer: bool, key: &str) -> Option<Preview> {
        let account = &self.mailbox.account;
        let block = if answer { account.reply_signature_block() } else { account.signature_block() }?;
        Some(Preview::new(format!("compose-signature-{}-{key}", account.id), &block))
    }

    /// An existing draft, fetched first.
    pub fn existing(account: usize, mailbox: Arc<Mailbox>, id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self::base(account, mailbox.clone(), window, cx);
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
    pub fn loaded(account: usize, mailbox: Arc<Mailbox>, draft: Message, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self::base(account, mailbox, window, cx);
        let forward = draft.to_recipients.is_empty();
        this.fill(draft, window, cx);
        if forward {
            this.to.update(cx, |s, cx| s.focus(window, cx));
        } else {
            this.body.update(cx, |s, cx| s.focus(window, cx));
        }
        this
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
        let join = |list: &[Recipient]| list.iter().map(Recipient::full).collect::<Vec<_>>().join(", ");
        let document = message.body.as_ref().map(|b| b.content.clone()).unwrap_or_default();
        let (text, markers) = match html::author_html(&document) {
            Some(author) => (html::html_to_text(author), true),
            None => (html::html_to_text(&document), false),
        };
        self.has_markers = markers;
        self.show_cc = !message.cc_recipients.is_empty() || !message.bcc_recipients.is_empty();
        let (to, cc, bcc) = (join(&message.to_recipients), join(&message.cc_recipients), join(&message.bcc_recipients));
        let subject = message.subject.clone().unwrap_or_default();
        // A Just Mail draft shows what follows the text: the signature, then a reply's quote.
        let quote = html::quote_html(&document).filter(|_| markers);
        self.quote = quote.map(|q| html::to_display_markdown(q, false, &[]).into());
        let answer = quote.is_some() || is_answer_subject(&subject);
        self.signature = if markers && html::has_signature(&document) { self.signature_preview(answer, &message.id) } else { None };
        self.to.update(cx, |s, cx| s.set_value(to, window, cx));
        self.cc.update(cx, |s, cx| s.set_value(cc, window, cx));
        self.bcc.update(cx, |s, cx| s.set_value(bcc, window, cx));
        self.subject.update(cx, |s, cx| s.set_value(subject, window, cx));
        self.body.update(cx, |s, cx| s.set_value(text.clone(), window, cx));
        self.saved_body = text;
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
        cx.notify();
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

    fn body_text(&self, cx: &App) -> String {
        self.body.read(cx).value().to_string()
    }

    /// Recipients of all three fields; `strict` reports a typo instead of skipping the field.
    fn recipients(&self, cx: &App, strict: bool) -> Result<[Option<Vec<Recipient>>; 3], String> {
        let mut out: [Option<Vec<Recipient>>; 3] = [None, None, None];
        for (i, input) in [&self.to, &self.cc, &self.bcc].into_iter().enumerate() {
            match Recipient::parse_list(&Self::value(input, cx)) {
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
        let body = self.body_text(cx);
        let subject = Self::value(&self.subject, cx);
        let is_new = self.draft.is_none();
        if is_new && body.trim().is_empty() && subject.is_empty() && to.as_ref().is_none_or(|t| t.is_empty()) {
            return; // nothing worth a draft yet
        }
        let input = DraftInput {
            to,
            cc,
            bcc,
            subject: Some(subject),
            body_html: (is_new || body != self.saved_body).then(|| html::text_to_html(&body)),
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
                        this.saved_body = body;
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
                        if this.send_after_save {
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

    /// Send right away (no confirmation): saves pending edits first.
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        if self.dirty || self.draft.is_none() || self.saving {
            self.send_after_save = true;
            if !self.saving {
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
        self.run(window, cx, move || mailbox.send_draft(&id), move |this, result, _, cx| {
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
        self.saving = true;
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
            |this, result, _, _| {
                this.saving = false;
                match result {
                    Ok(added) => this.attachments.extend(added),
                    Err(e) => this.error = Some(e.to_string()),
                }
            },
        );
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

    fn status(&self) -> String {
        if self.sending {
            tr!("Sending…", "Wird gesendet…").into()
        } else if self.saving {
            tr!("Saving…", "Speichert…").into()
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
        .child(div().w(px(64.)).flex_none().text_sm().text_color(theme.muted_foreground).child(label))
        .child(div().flex_1().min_w_0().child(input))
}

impl Render for Composer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let busy = self.sending || self.loading;
        let account = self.mailbox.account.email.clone();
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
            .child(div().flex_1().text_xs().text_color(theme.muted_foreground).child(self.status()))
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
        for a in &self.attachments {
            let id = a.id.clone();
            attachments = attachments.child(
                h_flex()
                    .gap_1p5()
                    .pl_2p5()
                    .pr_1()
                    .py_0p5()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(theme.border)
                    .text_xs()
                    .items_center()
                    .child(Icon::new(IconName::Paperclip).xsmall())
                    .child(a.name.clone())
                    .child(div().text_color(theme.muted_foreground).child(util::size(a.size)))
                    .child(
                        Button::new(SharedString::from(format!("rm-{}", a.id)))
                            .icon(IconName::Close)
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(move |this, _, w, cx| this.remove_attachment(id.clone(), w, cx))),
                    ),
            );
        }

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
                    "This draft was not written in Just Mail: editing the text saves it as plain text.",
                    "Dieser Entwurf stammt nicht aus Just Mail: Änderungen am Text speichern ihn als einfachen Text."
                )))
            })
            .child(
                h_flex()
                    .px_5()
                    .py_2()
                    .gap_3()
                    .border_b_1()
                    .border_color(theme.border.opacity(0.6))
                    .child(div().w(px(64.)).flex_none().text_sm().text_color(theme.muted_foreground).child(tr!("From", "Von")))
                    .child(div().text_sm().child(account)),
            )
            .child(field_row(tr!("To", "An"), Input::new(&self.to).appearance(false), cx))
            .when(self.show_cc, |d| {
                d.child(field_row("Cc", Input::new(&self.cc).appearance(false), cx))
                    .child(field_row("Bcc", Input::new(&self.bcc).appearance(false), cx))
            })
            .child(field_row(tr!("Subject", "Betreff"), Input::new(&self.subject).appearance(false), cx))
            .when(!self.attachments.is_empty(), |d| d.child(attachments))
            .child(
                // the mail as it goes out: your text, the signature, then a reply's quote
                div().id("compose-scroll").flex_1().min_h_0().overflow_y_scroll().child(
                    v_flex()
                        .px_3()
                        .pt_2()
                        .pb_6()
                        .child(Textarea::new(&self.body).appearance(false))
                        .when_some(self.signature.as_ref(), |d, signature| {
                            d.child(
                                div()
                                    .id("compose-signature")
                                    .mx_2()
                                    .mt_2()
                                    .pt_3()
                                    .border_t_1()
                                    .border_color(theme.border.opacity(0.6))
                                    .text_sm()
                                    .child(signature.render())
                                    .tooltip(|w, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(tr!(
                                            "Signature: change it under Accounts & signatures",
                                            "Signatur: änderbar unter Konten & Signaturen"
                                        ))
                                        .build(w, cx)
                                    }),
                            )
                        })
                        .when_some(self.quote.clone(), |d, quote| {
                            let toggle = Button::new("toggle-quote")
                                .ghost()
                                .xsmall()
                                .icon(if self.show_quote { IconName::ChevronUp } else { IconName::Ellipsis })
                                .label(if self.show_quote {
                                    tr!("Hide quoted mail", "Zitierte E-Mail ausblenden")
                                } else {
                                    tr!("Show quoted mail", "Zitierte E-Mail anzeigen")
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_quote = !this.show_quote;
                                    cx.notify();
                                }));
                            d.child(h_flex().mx_2().mt_4().child(toggle)).when(self.show_quote, |d| {
                                d.child(
                                    div()
                                        .mx_2()
                                        .mt_2()
                                        .p_4()
                                        .rounded(px(8.))
                                        .bg(theme.muted)
                                        .text_sm()
                                        .text_color(theme.muted_foreground)
                                        .child(TextView::markdown("compose-quote", quote)),
                                )
                            })
                        }),
                ),
            )
    }
}

/// Subject of a reply or forward ("RE:", "AW:", "FW:", "WG:" …).
fn is_answer_subject(subject: &str) -> bool {
    let subject = subject.trim_start().to_lowercase();
    ["re:", "aw:", "fw:", "fwd:", "wg:", "antw:"].iter().any(|p| subject.starts_with(p))
}
