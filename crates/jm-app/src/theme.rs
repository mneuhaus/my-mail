//! Look: calm greys instead of black, one blue for what matters (selection, unread, send),
//! colored folder icons so the sidebar reads at a glance. Close to Spark's dark and light themes.

use std::rc::Rc;

use gpui_kit::component::{ActiveTheme, Theme, ThemeConfig};
use gpui_kit::*;

/// Text sizes of macOS: 13 for the interface, 11 for secondary text (gpui's text_sm/xs are 14/12).
pub const TEXT: Pixels = px(13.);
pub const TEXT_SMALL: Pixels = px(11.);

/// (field, light, dark)
const COLORS: &[(&str, &str, &str)] = &[
    ("background", "#ffffff", "#1c1c1e"),
    ("foreground", "#1d1d1f", "#ececee"),
    ("border", "#e5e5ea", "#2f3034"),
    ("input", "#d1d1d6", "#3a3b40"),
    ("muted", "#f2f2f7", "#2a2b2f"),
    ("muted_foreground", "#6e6e73", "#9a9ca3"),
    // slightly translucent: the window blurs the desktop behind it, like the sidebar material of macOS
    ("sidebar", "#ececf0ee", "#262629ec"),
    ("sidebar_border", "#00000017", "#00000099"),
    ("sidebar_foreground", "#1d1d1f", "#e4e4e7"),
    ("sidebar_accent", "#00000014", "#ffffff17"),
    ("sidebar_accent_foreground", "#1d1d1f", "#ffffff"),
    ("list", "#f8f8fa", "#212124"),
    ("list_hover", "#eeeef3", "#2a2b30"),
    ("list_active", "#d6e4fd", "#1f4f99"),
    ("list_active_border", "#2f6fe4", "#3b82f6"),
    ("secondary", "#f2f2f7", "#2a2b2f"),
    ("secondary_hover", "#e8e8ed", "#323338"),
    ("secondary_active", "#dcdce3", "#3a3b40"),
    ("secondary_foreground", "#1d1d1f", "#ececee"),
    ("accent", "#ececf1", "#2c2d32"),
    ("accent_foreground", "#1d1d1f", "#ececee"),
    ("primary", "#2f6fe4", "#3b82f6"),
    ("primary_hover", "#3b7bf0", "#5390f7"),
    ("primary_active", "#255fd0", "#2f6fdb"),
    ("primary_foreground", "#ffffff", "#ffffff"),
    ("link", "#2f6fe4", "#6aa4fa"),
    ("link_hover", "#255fd0", "#8cb9fb"),
    ("success", "#28a745", "#34c759"),
    ("success_foreground", "#ffffff", "#ffffff"),
    ("warning", "#d97c00", "#ff9f0a"),
    ("warning_foreground", "#ffffff", "#1c1c1e"),
    ("danger", "#e5372b", "#ff5a4f"),
    ("danger_foreground", "#ffffff", "#ffffff"),
    ("ring", "#2f6fe4", "#3b82f6"),
    ("selection", "#cfe0fc", "#2f4f7f"),
    ("popover", "#ffffff", "#2a2b2f"),
    ("popover_foreground", "#1d1d1f", "#ececee"),
    ("switch", "#d1d1d6", "#3a3b40"),
    ("title_bar", "#f0f0f3", "#252528"),
    ("title_bar_border", "#e0e0e5", "#2f3034"),
];

fn apply(config: &Rc<ThemeConfig>, dark: bool) -> Rc<ThemeConfig> {
    let mut c = (**config).clone();
    let value = |name: &str| COLORS.iter().find(|(k, _, _)| *k == name).map(|(_, l, d)| if dark { *d } else { *l });
    macro_rules! set {
        ($($field:ident),*) => { $( if let Some(v) = value(stringify!($field)) { c.colors.$field = Some(v.to_string().into()); } )* };
    }
    set!(
        background, foreground, border, input, muted, muted_foreground, sidebar, sidebar_border, sidebar_foreground,
        sidebar_accent, sidebar_accent_foreground, list, list_hover, list_active, list_active_border, secondary,
        secondary_hover, secondary_active, secondary_foreground, accent, accent_foreground, primary, primary_hover,
        primary_active, primary_foreground, link, link_hover, success, success_foreground, warning,
        warning_foreground, danger, danger_foreground, ring, selection, popover, popover_foreground, switch, title_bar,
        title_bar_border
    );
    c.colors.button_primary = c.colors.primary.clone();
    c.colors.button_primary_hover = c.colors.primary_hover.clone();
    c.colors.button_primary_active = c.colors.primary_active.clone();
    c.colors.button_primary_foreground = c.colors.primary_foreground.clone();
    c.radius = Some(6);
    c.radius_lg = Some(10);
    Rc::new(c)
}

/// Put the palette into gpui-kit's light and dark themes (before `Theme::change`).
pub fn register(cx: &mut App) {
    let t = Theme::global_mut(cx);
    t.light_theme = apply(&t.light_theme, false);
    t.dark_theme = apply(&t.dark_theme, true);
}

fn hex(v: u32) -> Hsla {
    rgb(v).into()
}

/// Colors outside gpui-kit's theme: folder icons, account dots, the mail card.
pub struct Extra {
    pub inbox: Hsla,
    pub drafts: Hsla,
    pub sent: Hsla,
    pub archive: Hsla,
    pub junk: Hsla,
    pub trash: Hsla,
    pub folder: Hsla,
    pub flagged: Hsla,
    /// Background of the card the mail body sits on.
    pub card: Hsla,
    /// Text on a selected (blue) list row.
    pub on_selected: Hsla,
    pub on_selected_muted: Hsla,
}

pub fn extra(cx: &App) -> Extra {
    let dark = cx.theme().mode.is_dark();
    if dark {
        Extra {
            inbox: hex(0x4f8ff7),
            drafts: hex(0xffb340),
            sent: hex(0x64d2ff),
            archive: hex(0x34c759),
            junk: hex(0x98989d),
            trash: hex(0xff5a4f),
            folder: hex(0x8e9bb3),
            flagged: hex(0xff7a45),
            card: hex(0x18181a),
            on_selected: hex(0xffffff),
            on_selected_muted: hsla(0., 0., 1., 0.72),
        }
    } else {
        Extra {
            inbox: hex(0x2f6fe4),
            drafts: hex(0xe08600),
            sent: hex(0x1a9fd6),
            archive: hex(0x28a745),
            junk: hex(0x8e8e93),
            trash: hex(0xe5372b),
            folder: hex(0x6b7a96),
            flagged: hex(0xf05a1a),
            card: hex(0xfbfbfc),
            on_selected: hex(0x1d1d1f),
            on_selected_muted: hex(0x4a4f5c),
        }
    }
}

/// A distinct color per account (sidebar dot, tag in the flagged list).
pub fn account_color(ix: usize) -> Hsla {
    const COLORS: [u32; 6] = [0x3b82f6, 0xbf5af2, 0x30d158, 0xff9f0a, 0xff375f, 0x64d2ff];
    hex(COLORS[ix % COLORS.len()])
}
