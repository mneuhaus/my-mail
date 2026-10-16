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

/// The Dock shows an app's badge only if the app may badge its icon (System Settings,
/// Notifications); without asking, the badge is set and stays invisible. Asks for that and nothing
/// else: no banners, no sounds. macOS asks the user once and remembers the answer.
///
/// A bare binary (`cargo run`, the demo) has no bundle to ask for, and the Dock shows its badge
/// anyway; asking there would end the process.
#[cfg(target_os = "macos")]
pub fn allow_badge() {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSBundle, NSError};
    use objc2_user_notifications::{UNAuthorizationOptions, UNUserNotificationCenter};

    if NSBundle::mainBundle().bundleIdentifier().is_none() {
        return;
    }
    let done = RcBlock::new(|granted: Bool, error: *mut NSError| {
        // SAFETY: the error is either null or valid for the duration of the call
        if let Some(error) = unsafe { error.as_ref() } {
            eprintln!("no badge on the Dock icon: {}", error.localizedDescription());
        } else if !granted.as_bool() {
            eprintln!("no badge on the Dock icon: not allowed in System Settings, Notifications");
        }
    });
    UNUserNotificationCenter::currentNotificationCenter()
        .requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Badge, &done);
}

#[cfg(not(target_os = "macos"))]
pub fn allow_badge() {}
