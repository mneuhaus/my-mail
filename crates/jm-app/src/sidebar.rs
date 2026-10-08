//! Left column: flagged mail of all accounts, then each account with its folders.

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::{ActiveTheme, Icon, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use jm_core::Folder;

use crate::app::{MailApp, Pane, View};
use crate::tr;

pub const SIDEBAR_W: f32 = 232.;
/// Height of the top strip of every column (room for the traffic lights, window drag area).
pub const TOP_H: f32 = 48.;

fn folder_icon(f: &Folder) -> IconName {
    match f.well_known.as_deref() {
        Some("inbox") => IconName::Inbox,
        Some("drafts") => IconName::SquarePen,
        Some("sentitems") => IconName::Send,
        Some("archive") => IconName::Archive,
        Some("deleteditems") => IconName::Trash,
        Some("junkemail") => IconName::Ban,
        _ => IconName::Folder,
    }
}

fn folder_label(f: &Folder) -> String {
    let german = crate::i18n::german();
    match (f.well_known.as_deref(), german) {
        (Some("inbox"), false) => "Inbox".into(),
        (Some("inbox"), true) => "Posteingang".into(),
        (Some("drafts"), false) => "Drafts".into(),
        (Some("drafts"), true) => "Entwürfe".into(),
        (Some("sentitems"), false) => "Sent".into(),
        (Some("sentitems"), true) => "Gesendet".into(),
        (Some("archive"), false) => "Archive".into(),
        (Some("archive"), true) => "Archiv".into(),
        (Some("deleteditems"), false) => "Trash".into(),
        (Some("deleteditems"), true) => "Papierkorb".into(),
        (Some("junkemail"), _) => "Spam".into(),
        _ => f.display_name.clone(),
    }
}

impl MailApp {
    pub fn render_sidebar(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let flagged_active = self.view == Some(View::Flagged);
        let mut list = v_flex().gap_0p5().px_2().pb_4();

        list = list.child(item(
            "flagged",
            IconName::Flag,
            tr!("Flagged", "Markiert").into(),
            None,
            flagged_active,
            0,
            cx,
            cx.listener(|this, _, w, cx| this.show(View::Flagged, w, cx)),
        ));

        for (ix, account) in self.accounts.iter().enumerate() {
            let active_folder = match &self.view {
                Some(View::Folder { account: a, folder }) if *a == ix => Some(folder.clone()),
                _ => None,
            };
            let title = account.config.email.clone();
            list = list.child(
                h_flex()
                    .mt_4()
                    .mb_1()
                    .px_2()
                    .gap_1p5()
                    .items_center()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child(div().flex_1().min_w_0().truncate().child(title))
                    .when(account.config.read_only, |d| {
                        d.child(Icon::new(IconName::Lock).xsmall().text_color(theme.muted_foreground))
                    }),
            );
            if let Some(err) = &account.folders_error {
                list = list.child(div().px_2().text_xs().text_color(theme.danger).child(err.clone()));
            }
            let (main, rest): (Vec<&Folder>, Vec<&Folder>) =
                account.folders.iter().partition(|f| f.well_known.is_some() && f.depth == 0);
            let folders: Vec<&Folder> = if account.folders.is_empty() {
                Vec::new()
            } else if account.show_all_folders {
                main.into_iter().chain(rest).collect()
            } else {
                main
            };
            if account.folders.is_empty() && account.folders_error.is_none() {
                // placeholder until the folder list arrives: the inbox is always there
                list = list.child(item(
                    SharedString::from(format!("inbox-{ix}")),
                    IconName::Inbox,
                    tr!("Inbox", "Posteingang").into(),
                    None,
                    active_folder.as_deref() == Some("inbox"),
                    0,
                    cx,
                    cx.listener(move |this, _, w, cx| this.show(View::Folder { account: ix, folder: "inbox".into() }, w, cx)),
                ));
            }
            for f in folders {
                let active = active_folder.as_deref().is_some_and(|a| a == f.id || f.well_known.as_deref() == Some(a));
                let count = match f.well_known.as_deref() {
                    Some("drafts") => f.total_item_count,
                    Some("sentitems" | "deleteditems" | "junkemail" | "archive") => 0,
                    _ => f.unread_item_count,
                };
                let id = f.id.clone();
                let depth = if account.show_all_folders { f.depth } else { 0 };
                list = list.child(item(
                    SharedString::from(format!("f-{ix}-{}", f.id)),
                    folder_icon(f),
                    folder_label(f).into(),
                    (count > 0).then_some(count),
                    active,
                    depth,
                    cx,
                    cx.listener(move |this, _, w, cx| this.show(View::Folder { account: ix, folder: id.clone() }, w, cx)),
                ));
            }
            let extra = account.folders.iter().filter(|f| !(f.well_known.is_some() && f.depth == 0)).count();
            if extra > 0 {
                let label = if account.show_all_folders {
                    tr!("Fewer folders", "Weniger Ordner").to_string()
                } else if crate::i18n::german() {
                    format!("{extra} weitere Ordner")
                } else {
                    format!("{extra} more folders")
                };
                list = list.child(
                    div()
                        .id(SharedString::from(format!("more-{ix}")))
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme.foreground))
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(a) = this.accounts.get_mut(ix) {
                                a.show_all_folders = !a.show_all_folders;
                            }
                            cx.notify();
                        })),
                );
            }
        }

        let settings_active = matches!(self.pane, Pane::Setup(_));
        v_flex()
            .w(px(SIDEBAR_W))
            .flex_none()
            .h_full()
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(
                h_flex()
                    .h(px(TOP_H))
                    .flex_none()
                    .w_full()
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(div().id("sidebar-scroll").flex_1().min_h_0().overflow_y_scroll().child(list))
            .child(
                h_flex()
                    .flex_none()
                    .px_2()
                    .py_2()
                    .border_t_1()
                    .border_color(theme.sidebar_border)
                    .child(item(
                        "settings",
                        IconName::Settings,
                        tr!("Accounts & signatures", "Konten & Signaturen").into(),
                        None,
                        settings_active,
                        0,
                        cx,
                        cx.listener(|this, _, w, cx| this.open_setup(w, cx)),
                    )),
            )
    }
}

#[allow(clippy::too_many_arguments)]
fn item<F: Fn(&ClickEvent, &mut Window, &mut App) + 'static>(
    id: impl Into<ElementId>,
    icon: IconName,
    label: SharedString,
    count: Option<u32>,
    active: bool,
    depth: u32,
    cx: &Context<MailApp>,
    on_click: F,
) -> Stateful<Div> {
    let theme = cx.theme();
    h_flex()
        .id(id.into())
        .w_full()
        .h(px(28.))
        .pl(px(8. + depth as f32 * 12.))
        .pr_2()
        .gap_2()
        .items_center()
        .rounded(px(6.))
        .text_sm()
        .cursor_pointer()
        .when(active, |d| d.bg(theme.sidebar_accent).text_color(theme.sidebar_accent_foreground))
        .when(!active, |d| d.hover(|s| s.bg(theme.sidebar_accent.opacity(0.5))))
        .child(Icon::new(icon).small().text_color(if active { theme.sidebar_accent_foreground } else { theme.muted_foreground }))
        .child(div().flex_1().min_w_0().truncate().child(label))
        .when_some(count, |d, n| {
            d.child(div().text_xs().text_color(theme.muted_foreground).child(n.to_string()))
        })
        .on_click(on_click)
}
