//! English or German, decided once: `JUST_MAIL_LANG=en|de` overrides, otherwise the system
//! locale decides (German when it starts with "de").

use std::sync::OnceLock;

static GERMAN: OnceLock<bool> = OnceLock::new();

pub fn german() -> bool {
    *GERMAN.get_or_init(|| match std::env::var("JUST_MAIL_LANG").as_deref() {
        Ok("de") => true,
        Ok("en") => false,
        _ => sys_locale::get_locale().is_some_and(|l| l.to_lowercase().starts_with("de")),
    })
}

/// Pick the English or German text. For `format!` texts use `i18n::german()` directly.
#[macro_export]
macro_rules! tr {
    ($en:literal, $de:literal) => {
        if $crate::i18n::german() { $de } else { $en }
    };
}
