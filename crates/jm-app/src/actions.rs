//! Keyboard actions and their bindings.

use gpui_kit::*;

gpui_kit::actions!(
    just_mail,
    [
        NewMessage,
        Reply,
        ReplyAll,
        Forward,
        ToggleFlag,
        ToggleRead,
        ArchiveMessage,
        TrashMessage,
        Refresh,
        FocusSearch,
        SelectNext,
        SelectPrev,
        SendDraft,
        SaveDraft,
        CopyId,
    ]
);

/// Context of the message list: single-key shortcuts only work there (not while typing).
pub const LIST: &str = "MailList";
/// Context of the draft editor.
pub const COMPOSER: &str = "Composer";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-n", NewMessage, None),
        KeyBinding::new("cmd-r", Reply, None),
        KeyBinding::new("cmd-shift-r", ReplyAll, None),
        KeyBinding::new("cmd-shift-f", Forward, None),
        KeyBinding::new("cmd-shift-l", ToggleFlag, None),
        KeyBinding::new("cmd-shift-u", ToggleRead, None),
        KeyBinding::new("ctrl-cmd-a", ArchiveMessage, None),
        KeyBinding::new("cmd-backspace", TrashMessage, Some(LIST)),
        KeyBinding::new("cmd-shift-n", Refresh, None),
        KeyBinding::new("cmd-f", FocusSearch, None),
        KeyBinding::new("down", SelectNext, Some(LIST)),
        KeyBinding::new("j", SelectNext, Some(LIST)),
        KeyBinding::new("up", SelectPrev, Some(LIST)),
        KeyBinding::new("k", SelectPrev, Some(LIST)),
        KeyBinding::new("e", ArchiveMessage, Some(LIST)),
        KeyBinding::new("backspace", TrashMessage, Some(LIST)),
        KeyBinding::new("s", ToggleFlag, Some(LIST)),
        KeyBinding::new("u", ToggleRead, Some(LIST)),
        KeyBinding::new("r", Reply, Some(LIST)),
        KeyBinding::new("a", ReplyAll, Some(LIST)),
        KeyBinding::new("f", Forward, Some(LIST)),
        KeyBinding::new("c", NewMessage, Some(LIST)),
        KeyBinding::new("y", CopyId, Some(LIST)),
        KeyBinding::new("/", FocusSearch, Some(LIST)),
        KeyBinding::new("cmd-enter", SendDraft, Some(COMPOSER)),
        KeyBinding::new("cmd-s", SaveDraft, Some(COMPOSER)),
    ]);
}
