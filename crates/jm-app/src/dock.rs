//! The number of unread mails on the Dock icon, like Mail and Outlook show it.

/// Show `count` on the Dock icon; 0 shows nothing. Main thread only (elsewhere it does nothing).
#[cfg(target_os = "macos")]
pub fn set_badge(count: u32) {
    use objc2_app_kit::NSApplication;
    use objc2_foundation::{MainThreadMarker, NSString};

    let Some(mtm) = MainThreadMarker::new() else { return };
    let label = (count > 0).then(|| NSString::from_str(&count.to_string()));
    NSApplication::sharedApplication(mtm).dockTile().setBadgeLabel(label.as_deref());
}

#[cfg(not(target_os = "macos"))]
pub fn set_badge(_count: u32) {}
