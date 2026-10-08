//! Middle column: search field and the message list.

use std::ops::Range;

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme, Icon, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::actions;
use crate::app::{MailApp, RowAction, View};
use crate::sidebar::TOP_H;
use crate::{theme, tr, util};

pub const LIST_W: f32 = 380.;
const ROW_H: f32 = 78.;

impl MailApp {
    fn view_title(&self) -> String {
        match &self.view {
            Some(View::Pinned) => tr!("Pinned", "Angeheftet").to_string(),
            Some(View::Search { query, .. }) => format!("„{query}“"),
            Some(View::Folder { account, folder }) => {
                self.accounts
                    .get(*account)
                    .and_then(|a| a.folder(folder))
                    .map(|f| match f.well_known.as_deref() {
                        Some("inbox") => tr!("Inbox", "Posteingang").to_string(),
                        Some("drafts") => tr!("Drafts", "Entwürfe").to_string(),
                        Some("sentitems") => tr!("Sent", "Gesendet").to_string(),
                        Some("archive") => tr!("Archive", "Archiv").to_string(),
                        Some("deleteditems") => tr!("Trash", "Papierkorb").to_string(),
                        _ => f.display_name.clone(),
                    })
                    .unwrap_or_else(|| tr!("Inbox", "Posteingang").to_string())
            }
            None => String::new(),
        }
    }

    fn view_subtitle(&self) -> Option<String> {
        match &self.view {
            Some(View::Folder { account, .. } | View::Search { account, .. }) => {
                self.accounts.get(*account).map(|a| a.config.email.clone())
            }
            Some(View::Pinned) => Some(tr!("all accounts", "alle Konten").to_string()),
            None => None,
        }
    }

    pub fn render_list(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let count = self.rows.len();
        let empty = count == 0 && !self.loading;
        let header = h_flex()
            .h(px(TOP_H))
            .flex_none()
            .px_4()
            .gap_2()
            .items_center()
            .window_control_area(WindowControlArea::Drag)
            .child(
                // one line, centred on the traffic lights and the toolbar next door
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .items_baseline()
                    .child(div().flex_none().text_base().font_weight(FontWeight::SEMIBOLD).child(self.view_title()))
                    .when_some(self.view_subtitle(), |d, s| {
                        d.child(div().flex_1().min_w_0().text_xs().text_color(theme.muted_foreground).truncate().child(s))
                    }),
            )
            .when(self.loading, |d| {
                d.child(Icon::new(IconName::LoaderCircle).small().text_color(theme.muted_foreground))
            })
            .child(
                // the one round blue button: write a mail
                div()
                    .id("compose")
                    .size(px(30.))
                    .flex_none()
                    .rounded_full()
                    .bg(theme.primary)
                    .hover(|s| s.bg(theme.primary_hover))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(IconName::Pencil).small().text_color(theme.primary_foreground))
                    .tooltip(|w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tr!("New message (⌘N)", "Neue E-Mail (⌘N)")).build(w, cx)
                    })
                    .on_click(cx.listener(|this, _, w, cx| this.new_message(w, cx))),
            );

        let list = uniform_list(
            "messages",
            count,
            cx.processor(|this, range: Range<usize>, window, cx| {
                if range.end + 5 >= this.rows.len() && !this.next.is_empty() && !this.loading_more {
                    cx.defer_in(window, |this, window, cx| this.load_more(window, cx));
                }
                range.map(|ix| this.render_row(ix, cx)).collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.list_scroll)
        .flex_1()
        .min_h_0();

        v_flex()
            .w(px(LIST_W))
            .flex_none()
            .h_full()
            .bg(theme.colors.list)
            .border_r_1()
            .border_color(theme.border)
            .key_context(actions::LIST)
            .track_focus(&self.list_focus)
            .child(header)
            .child(div().px_3().pb_2().flex_none().child(Input::new(&self.search).small().cleanable(true).prefix(
                Icon::new(IconName::Search).small().text_color(theme.muted_foreground),
            )))
            .child(div().h(px(1.)).flex_none().bg(theme.border))
            .when_some(self.list_error.clone().filter(|_| count == 0), |d, e| {
                d.child(div().p_4().text_sm().text_color(theme.danger).child(e))
            })
            .when(empty && self.list_error.is_none(), |d| {
                d.child(
                    div()
                        .p_6()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(tr!("Nothing here.", "Hier ist nichts.")),
                )
            })
            .child(list)
    }

    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let x = theme::extra(cx);
        let row = &self.rows[ix];
        let m = &row.message;
        let selected = self.selected.as_deref() == Some(m.id.as_str());
        let unread = !m.is_read && !m.is_draft;
        let who = if m.is_draft || self.is_sent_view() {
            let to: Vec<String> = m.to_recipients.iter().map(|r| r.display().to_string()).collect();
            if to.is_empty() { tr!("(no recipient)", "(kein Empfänger)").to_string() } else { to.join(", ") }
        } else {
            m.sender()
        };
        let account_tag = (self.view == Some(View::Pinned) && self.accounts.len() > 1)
            .then(|| self.accounts.get(row.account).map(|a| (a.config.id.clone(), theme::account_color(row.account))))
            .flatten();
        let (fg, muted) = if selected { (x.on_selected, x.on_selected_muted) } else { (theme.foreground, theme.muted_foreground) };
        let date_color = if unread && !selected { theme.primary } else { muted };
        let menu = self.row_menu(ix, cx);
        h_flex()
            .id(("row", ix))
            .h(px(ROW_H))
            .w_full()
            .px_3()
            .gap_2()
            // the three lines sit in the middle, the same air above and below
            .items_center()
            .border_b_1()
            .border_color(theme.border.opacity(0.5))
            .cursor_pointer()
            .text_color(fg)
            .when(selected, |d| d.bg(theme.list_active))
            .when(!selected, |d| d.hover(|s| s.bg(theme.list_hover)))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.focus_list(window, cx);
                this.select_index(ix, window, cx);
            }))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .items_start()
                    .child(
                        // unread dot, centred on the first line
                        div().mt(px(6.)).size(px(8.)).flex_none().rounded_full().when(unread, |d| {
                            d.bg(if selected { fg } else { theme.primary })
                        }),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(1.))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .when(m.is_draft, |d| {
                                        d.child(div().flex_none().text_sm().text_color(if selected { fg } else { x.drafts }).child(
                                            tr!("Draft", "Entwurf"),
                                        ))
                                    })
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_sm()
                                            .when(unread, |d| d.font_weight(FontWeight::BOLD))
                                            .when(!unread, |d| d.font_weight(FontWeight::MEDIUM))
                                            .child(who),
                                    )
                                    .when(m.has_attachments, |d| {
                                        d.child(Icon::new(IconName::Paperclip).xsmall().text_color(muted))
                                    })
                                    .when(m.is_flagged(), |d| {
                                        d.child(Icon::new(IconName::Pin).xsmall().text_color(if selected { fg } else { x.flagged }))
                                    })
                                    .child(div().flex_none().text_xs().text_color(date_color).child(util::list_date(m.date()))),
                            )
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .items_center()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_sm()
                                            .when(unread, |d| d.font_weight(FontWeight::MEDIUM))
                                            .child(m.subject().to_string()),
                                    )
                                    .when_some(account_tag, |d, (tag, color)| {
                                        d.child(
                                            h_flex()
                                                .flex_none()
                                                .gap_1()
                                                .items_center()
                                                .text_xs()
                                                .text_color(muted)
                                                .child(div().size(px(6.)).rounded_full().bg(color))
                                                .child(tag),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(util::ellipsize(&m.body_preview.replace(['\r', '\n'], " "), 160)),
                            ),
                    ),
            )
            .context_menu(menu)
            .into_any_element()
    }

    /// The right-click menu of a row: its actions apply to that row, whatever is selected.
    fn row_menu(
        &self,
        ix: usize,
        cx: &Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let row = &self.rows[ix];
        let id = row.message.id.clone();
        let draft = row.message.is_draft;
        let pinned = row.message.is_flagged();
        let read = row.message.is_read;
        let read_only = self.accounts.get(row.account).is_none_or(|a| a.config.read_only);
        let app = cx.entity().downgrade();
        move |menu, _, _| {
            let item = |label: &'static str, icon: IconName, action: RowAction, enabled: bool| {
                let (app, id) = (app.clone(), id.clone());
                PopupMenuItem::new(label).icon(icon).disabled(!enabled).on_click(move |_, window, cx| {
                    let _ = app.update(cx, |this, cx| this.row_action(id.clone(), action, window, cx));
                })
            };
            let writable = !read_only;
            let mut menu = menu;
            if !draft {
                menu = menu
                    .item(item(tr!("Reply", "Antworten"), IconName::Reply, RowAction::Reply, writable))
                    .item(item(tr!("Reply all", "Allen antworten"), IconName::ReplyAll, RowAction::ReplyAll, writable))
                    .item(item(tr!("Forward", "Weiterleiten"), IconName::Forward, RowAction::Forward, writable))
                    .separator();
            }
            menu = menu
                .item(if pinned {
                    item(tr!("Unpin", "Nicht mehr anheften"), IconName::PinOff, RowAction::Pin, writable)
                } else {
                    item(tr!("Pin", "Anheften"), IconName::Pin, RowAction::Pin, writable)
                })
                .when(!draft, |m| {
                    m.item(if read {
                        item(tr!("Mark as unread", "Als ungelesen markieren"), IconName::Mail, RowAction::Read, writable)
                    } else {
                        item(tr!("Mark as read", "Als gelesen markieren"), IconName::MailOpen, RowAction::Read, writable)
                    })
                })
                .separator();
            menu = if draft {
                menu.item(item(tr!("Discard draft", "Entwurf verwerfen"), IconName::Trash, RowAction::Trash, writable))
            } else {
                menu.item(item(tr!("Archive", "Archivieren"), IconName::Archive, RowAction::Archive, writable))
                    .item(item(tr!("Delete", "Löschen"), IconName::Trash, RowAction::Trash, writable))
            };
            menu.separator().item(item(tr!("Copy id for jm", "ID für jm kopieren"), IconName::Copy, RowAction::CopyId, true))
        }
    }

    fn is_sent_view(&self) -> bool {
        match &self.view {
            Some(View::Folder { account, folder }) => {
                folder == "sentitems"
                    || self.accounts.get(*account).and_then(|a| a.folder(folder)).and_then(|f| f.well_known.as_deref())
                        == Some("sentitems")
            }
            _ => false,
        }
    }
}
