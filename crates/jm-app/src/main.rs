//! Just Mail: Microsoft 365 mail without the noise.
//!
//! Environment: `JUST_MAIL_HOME=<dir>` uses another config directory (tests, screenshots),
//! `JUST_MAIL_THEME=light|dark` overrides the system appearance, `JUST_MAIL_LANG=en|de` the
//! language.

mod actions;
mod app;
mod composer;
mod i18n;
mod list;
mod reader;
mod setup;
mod sidebar;
mod theme;
mod util;

use std::borrow::Cow;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;

gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        Mail, MailOpen, Send, Archive, Trash, Flag, FlagOff, Reply, ReplyAll, Forward, Paperclip, SquarePen, Pencil, Lock,
        Image, Download, FolderInput, CircleCheckBig, Save, ListTodo, LogIn, KeyRound
    ]
);

struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        if let Some(b) = ExtraIcons.load(path)? {
            return Ok(Some(b));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        let mut v = gpui_kit::assets::Assets.list(path)?;
        v.extend(ExtraIcons.list(path)?);
        v.sort();
        v.dedup();
        Ok(v)
    }
}

gpui_kit::actions!(window_actions, [Quit, CloseWindow, Hide]);

fn menus(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &CloseWindow, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-h", Hide, None),
    ]);
    cx.set_menus([
        Menu::new("Just Mail").items([
            MenuItem::action(tr!("Hide Just Mail", "Just Mail ausblenden"), Hide),
            MenuItem::separator(),
            MenuItem::action(tr!("Quit Just Mail", "Just Mail beenden"), Quit),
        ]),
        Menu::new(tr!("Message", "E-Mail")).items([
            MenuItem::action(tr!("New Message", "Neue E-Mail"), actions::NewMessage),
            MenuItem::action(tr!("Reply", "Antworten"), actions::Reply),
            MenuItem::action(tr!("Reply All", "Allen antworten"), actions::ReplyAll),
            MenuItem::action(tr!("Forward", "Weiterleiten"), actions::Forward),
            MenuItem::separator(),
            MenuItem::action(tr!("Flag / Unflag", "Markieren / Markierung entfernen"), actions::ToggleFlag),
            MenuItem::action(tr!("Mark Read / Unread", "Gelesen / Ungelesen"), actions::ToggleRead),
            MenuItem::action(tr!("Archive", "Archivieren"), actions::ArchiveMessage),
            MenuItem::action(tr!("Move to Trash", "In den Papierkorb"), actions::TrashMessage),
            MenuItem::separator(),
            MenuItem::action(tr!("Check for New Mail", "Neue E-Mails abrufen"), actions::Refresh),
        ]),
    ]);
}

fn apply_theme(window: Option<&mut Window>, cx: &mut App) {
    match std::env::var("JUST_MAIL_THEME").as_deref() {
        Ok("dark") => Theme::change(ThemeMode::Dark, window, cx),
        Ok("light") => Theme::change(ThemeMode::Light, window, cx),
        _ => Theme::sync_system_appearance(window, cx),
    }
}

fn main() {
    gpui_kit::application().with_assets(AppAssets).run(|cx| {
        gpui_kit::init(cx);
        theme::register(cx);
        apply_theme(None, cx);
        menus(cx);
        actions::bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1360.), px(860.)), cx);
        let opened = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Just Mail".into()),
                    appears_transparent: cfg!(target_os = "macos"),
                    traffic_light_position: Some(point(px(16.), px(16.))),
                }),
                window_min_size: Some(size(px(980.), px(600.))),
                ..Default::default()
            },
            cx,
            |window, cx| {
                window.observe_window_appearance(|window, cx| apply_theme(Some(window), cx)).detach();
                window.on_window_should_close(cx, |_, cx| {
                    cx.defer(|cx| cx.quit());
                    true
                });
                let view = cx.new(|cx| app::MailApp::new(window, cx));
                view.update(cx, |app, cx| app.focus_list(window, cx));
                view
            },
        );
        if let Err(e) = opened {
            eprintln!("could not open the window: {e:#}");
            cx.quit();
        }
        cx.activate(true);
    });
}
