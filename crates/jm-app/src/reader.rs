//! Right column when a received mail is open: toolbar, header, attachments, body, and below it
//! what came after it in the conversation.

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme, Disableable, Icon, Selectable as _, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use jm_core::Recipient;

use crate::app::{MailApp, Pane, ThreadMessage};
use crate::sidebar::TOP_H;
use crate::{theme, tr, util};

fn people(list: &[Recipient]) -> String {
    list.iter().map(Recipient::full).collect::<Vec<_>>().join(", ")
}

impl MailApp {
    pub fn render_reader(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let top = h_flex().h(px(TOP_H)).flex_none().w_full().window_control_area(WindowControlArea::Drag);
        let opened = match &self.pane {
            Pane::Reader(o) => o,
            Pane::Loading => {
                return v_flex().size_full().child(top).child(
                    h_flex()
                        .flex_1()
                        .justify_center()
                        .items_center()
                        .child(Icon::new(IconName::LoaderCircle).text_color(theme.muted_foreground)),
                );
            }
            _ => {
                let hint = if self.accounts.is_empty() {
                    tr!("Add an account to start.", "Füge ein Konto hinzu, um loszulegen.")
                } else {
                    tr!("No mail selected", "Keine E-Mail ausgewählt")
                };
                return v_flex().size_full().child(top).child(
                    v_flex()
                        .flex_1()
                        .justify_center()
                        .items_center()
                        .gap_2()
                        .text_color(theme.muted_foreground)
                        .child(Icon::new(IconName::Mail).large())
                        .child(div().text_sm().child(hint)),
                );
            }
        };
        let m = &opened.message;
        let read_only = self.accounts.get(opened.account).is_none_or(|a| a.config.read_only);
        let flagged = m.is_flagged();
        let short = jm_core::ids::short(&m.id);

        let toolbar = h_flex()
            .h(px(TOP_H))
            .flex_none()
            .px_4()
            .gap_1()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .window_control_area(WindowControlArea::Drag)
            .child(tool("reply", IconName::Reply, tr!("Reply (⌘R)", "Antworten (⌘R)"), read_only, cx, |t, w, cx| t.answer(false, false, w, cx)))
            .child(tool("reply-all", IconName::ReplyAll, tr!("Reply all (⇧⌘R)", "Allen antworten (⇧⌘R)"), read_only, cx, |t, w, cx| t.answer(true, false, w, cx)))
            .child(tool("forward", IconName::Forward, tr!("Forward (⇧⌘F)", "Weiterleiten (⇧⌘F)"), read_only, cx, |t, w, cx| t.answer(false, true, w, cx)))
            .child(div().w(px(1.)).h(px(18.)).mx_2().bg(theme.border))
            .child(
                Button::new("flag")
                    .icon(Icon::new(IconName::Pin).when(flagged, |i| i.text_color(theme::extra(cx).flagged)))
                    .ghost()
                    .small()
                    .disabled(read_only)
                    .selected(flagged)
                    .tooltip(if flagged {
                        tr!("Unpin (S)", "Nicht mehr anheften (S)")
                    } else {
                        tr!("Pin (S), also flags it in Outlook", "Anheften (S), in Outlook als Kennzeichnung")
                    })
                    .on_click(cx.listener(|this, _, w, cx| this.toggle_flag(w, cx))),
            )
            .child(tool("archive", IconName::Archive, tr!("Archive (E)", "Archivieren (E)"), read_only, cx, |t, w, cx| t.file_away(false, w, cx)))
            .child(tool("trash", IconName::Trash, tr!("Move to trash (⌫)", "In den Papierkorb (⌫)"), read_only, cx, |t, w, cx| t.file_away(true, w, cx)))
            .child(tool(
                "unread",
                IconName::MailOpen,
                tr!("Mark as unread (U)", "Als ungelesen markieren (U)"),
                read_only,
                cx,
                |t, w, cx| t.toggle_read(w, cx),
            ))
            .child(div().flex_1())
            .when(read_only, |d| {
                d.child(
                    h_flex()
                        .gap_1()
                        .text_size(theme::TEXT_SMALL)
                        .text_color(theme.muted_foreground)
                        .child(Icon::new(IconName::Lock).xsmall())
                        .child(tr!("read-only", "nur lesen")),
                )
            })
            .child(
                Button::new("copy-id")
                    .icon(IconName::Copy)
                    .ghost()
                    .small()
                    .tooltip(format!("{} · jm {short}", tr!("Copy the id for the jm CLI (Y)", "ID für die jm-CLI kopieren (Y)")))
                    .on_click(cx.listener(|this, _, w, cx| this.copy_id(w, cx))),
            );

        let mut recipients = format!("{} {}", tr!("To", "An"), people(&m.to_recipients));
        if !m.cc_recipients.is_empty() {
            recipients.push_str(&format!("   ·   Cc {}", people(&m.cc_recipients)));
        }
        let from = m.from.as_ref().map(Recipient::full).unwrap_or_default();

        let header = v_flex()
            .px_6()
            .pt_5()
            .pb_4()
            .gap_1()
            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(m.subject().to_string()))
            .child(
                h_flex()
                    .gap_3()
                    .items_baseline()
                    .child(div().flex_1().min_w_0().text_size(theme::TEXT).font_weight(FontWeight::MEDIUM).child(from))
                    .child(div().flex_none().text_xs().text_color(theme.muted_foreground).child(util::long_date(m.date()))),
            )
            .child(div().text_xs().text_color(theme.muted_foreground).child(recipients));

        let attachments = (!opened.attachments.is_empty()).then(|| {
            let mut row = h_flex().px_6().pb_3().gap_2().flex_wrap();
            for (i, a) in opened.attachments.iter().enumerate() {
                let att = a.clone();
                row = row.child(
                    h_flex()
                        .id(("att", i))
                        .gap_1p5()
                        .px_2p5()
                        .py_1()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme.border)
                        .text_xs()
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.secondary_hover))
                        .child(Icon::new(IconName::Paperclip).xsmall())
                        .child(a.name.clone())
                        .child(div().text_color(theme.muted_foreground).child(util::size(a.size)))
                        .tooltip({
                            let tip = tr!("Download and open", "Herunterladen und öffnen");
                            move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx)
                        })
                        .on_click(cx.listener(move |this, _, w, cx| this.download(att.clone(), w, cx))),
                );
            }
            row
        });

        let images_banner = (opened.has_remote_images && !opened.images_loaded).then(|| {
            h_flex()
                .mx_6()
                .mb_3()
                .px_3()
                .py_1p5()
                .gap_2()
                .items_center()
                .rounded(px(6.))
                .bg(theme.muted)
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(Icon::new(IconName::Image).xsmall())
                .child(div().flex_1().child(tr!(
                    "Remote images are blocked.",
                    "Externe Bilder sind blockiert."
                )))
                .child(
                    Button::new("load-images")
                        .label(tr!("Load images", "Bilder laden"))
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| this.load_images(cx))),
                )
        });

        let body_id = SharedString::from(format!("body-{}-{}-{}", m.id, opened.images_loaded, opened.pictures.len()));
        let body = TextView::markdown(ElementId::Name(body_id), opened.html.clone()).selectable(true);
        let later: Vec<_> = (opened.thread.iter().enumerate())
            .map(|(i, t)| later_card(i, t, opened.account, opened.images_loaded, cx))
            .collect();

        v_flex()
            .size_full()
            .child(toolbar)
            .child(
                div().id("reader-scroll").flex_1().min_h_0().overflow_y_scroll().child(
                    v_flex()
                        .w_full()
                        .child(header)
                        .children(attachments)
                        .children(images_banner)
                        .child(
                            v_flex()
                                .mx_6()
                                .mb_6()
                                .gap_3()
                                .child(card(cx).px_6().py_5().child(body))
                                .children(later),
                        ),
                ),
            )
    }
}

/// The rounded box a mail body sits in.
fn card(cx: &App) -> Div {
    let theme = cx.theme();
    div().rounded(px(10.)).border_1().border_color(theme.border).bg(theme::extra(cx).card).text_sm()
}

/// A later message of the conversation (an answer, maybe our own or a draft): who, when, and only
/// what it adds to the mail above.
fn later_card(i: usize, later: &ThreadMessage, account: usize, images: bool, cx: &mut Context<MailApp>) -> Div {
    let theme = cx.theme().clone();
    let m = &later.message;
    let to = m.to_recipients.iter().chain(&m.cc_recipients).map(Recipient::display).collect::<Vec<_>>().join(", ");
    let draft = m.is_draft.then(|| {
        let id = m.id.clone();
        Button::new(("edit-draft", i))
            .label(tr!("Edit", "Bearbeiten"))
            .icon(IconName::Pencil)
            .xsmall()
            .ghost()
            .on_click(cx.listener(move |this, _, w, cx| this.open_draft(account, id.clone(), w, cx)))
    });
    let sender = if m.is_draft { tr!("Draft", "Entwurf").to_string() } else { m.sender() };
    let body_id = SharedString::from(format!("later-{}-{images}", m.id));
    card(cx)
        .child(
            v_flex()
                .px_6()
                .pt_4()
                .gap_0p5()
                .child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(div().flex_1().min_w_0().font_weight(FontWeight::MEDIUM).child(sender))
                        .children(draft)
                        .child(div().flex_none().text_xs().text_color(theme.muted_foreground).child(util::long_date(m.date()))),
                )
                .when(!to.is_empty(), |d| {
                    d.child(div().text_xs().text_color(theme.muted_foreground).child(format!("{} {to}", tr!("To", "An"))))
                }),
        )
        .child(
            div().px_6().pt_3().pb_5().child(TextView::markdown(ElementId::Name(body_id), later.html.clone()).selectable(true)),
        )
}

fn tool(
    id: &'static str,
    icon: IconName,
    tip: &'static str,
    disabled: bool,
    cx: &mut Context<MailApp>,
    f: impl Fn(&mut MailApp, &mut Window, &mut Context<MailApp>) + 'static,
) -> impl IntoElement {
    Button::new(id)
        .icon(icon)
        .ghost()
        .small()
        .disabled(disabled)
        .tooltip(tip)
        .on_click(cx.listener(move |this, _, w, cx| f(this, w, cx)))
}
