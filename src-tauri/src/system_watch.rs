//! Adopt system settings while the app is running, not when it next gets focus.
//!
//! Changing the accent colour in System Settings updated every other app
//! immediately and ours only once it was clicked back into. That was not a
//! rendering quirk — nothing was listening. The accent was read at startup and
//! re-read on window activation, and the frontend's own comment said macOS
//! "doesn't push accent changes into a running process", which is simply not
//! true: it posts distributed notifications that any process can observe.
//!
//! Three settings are watched, all through the same mechanism:
//!
//! - **Accent** — `AppleColorPreferencesChangedNotification`.
//! - **Appearance** (light/dark) — `AppleInterfaceThemeChangedNotification`.
//! - **Text size** — the accessibility text-size category, whose notification
//!   name differs across releases, so several are observed. Harmless if one
//!   never fires; missing the change is not.
//!
//! The notification only says *what* changed. The frontend re-reads the value
//! itself, so there is one path for "read the accent" whether it runs at startup
//! or after a change — a push carrying the new value would be a second one, free
//! to disagree.

use tauri::ipc::Channel;

use crate::stream::ProgressStream;

/// What changed. The payload is deliberately just an identifier.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum SystemChange {
    Accent,
    Appearance,
    TextSize,
    KeyboardAccess,
    /// Reduce motion / reduce transparency / increase contrast / differentiate
    /// without colour, and the sidebar icon size.
    Accessibility,
}

static SYSTEM_CHANGES: ProgressStream<SystemChange> = ProgressStream::new();

/// Subscribe to system-setting changes. Registers the OS observers on first
/// call; later calls just re-point the channel, which is what a webview reload
/// does.
#[tauri::command]
pub fn subscribe_system_changes(channel: Channel<SystemChange>) {
    SYSTEM_CHANGES.subscribe(channel);
    #[cfg(target_os = "macos")]
    macos::start_observing();
}

/// The display preferences a UI is expected to respect, read together because
/// they change together and the UI applies them together.
///
/// WebKit exposes matching media queries (`prefers-reduced-motion`,
/// `prefers-contrast`, `prefers-reduced-transparency`) but a WKWebView never
/// resolves them to the system values — measured: all report false on a machine
/// where the settings are readable through AppKit. So the queries are free
/// syntax and not free behaviour, and the values have to come across the bridge.
#[derive(Clone, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AccessibilityPrefs {
    pub reduce_motion: bool,
    pub reduce_transparency: bool,
    pub increase_contrast: bool,
    pub differentiate_without_color: bool,
    /// System Settings → Appearance → Sidebar icon size: 1 small, 2 medium,
    /// 3 large. Defaults to medium when unset.
    pub sidebar_icon_size: i64,
}

#[tauri::command]
pub fn get_accessibility_prefs() -> AccessibilityPrefs {
    #[cfg(target_os = "macos")]
    {
        macos::accessibility_prefs()
    }
    #[cfg(not(target_os = "macos"))]
    {
        AccessibilityPrefs {
            sidebar_icon_size: 2,
            ..Default::default()
        }
    }
}

/// Whether macOS **Full Keyboard Access** is on (System Settings → Keyboard →
/// "Keyboard navigation").
///
/// This is the system-level expression of "I navigate with the keyboard", and it
/// changes what Tab does natively: with it OFF, Tab moves only between text
/// fields and lists — buttons, checkboxes and rows are skipped. Our app makes
/// everything tabbable regardless, which is why keyboard focus feels noisy on a
/// machine where the setting is off (measured: 46 tab stops in Messages, 58 in
/// Safety Scan, against roughly six a native app would offer).
///
/// `AppleKeyboardUIMode` is a bit field; bit 1 (value 2) means full keyboard
/// access. Returns false on non-macOS and when the preference is unset.
#[tauri::command]
pub fn get_full_keyboard_access() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::full_keyboard_access()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// The accessibility text-size category as a multiplier for our type ramp.
///
/// macOS's Text Size setting (System Settings → Accessibility → Display) does
/// NOT reach AppKit metrics or WebKit's `-apple-system-*` fonts — measured with
/// `scripts/font-probe.swift` at category XL, where every text style still
/// reported its default size. It only reaches apps using
/// `UIPreferredContentSizeCategory`. So an app like ours has to read the
/// category and apply it, or ignore the setting entirely — and ignoring it means
/// a user who enlarged system text gets nothing from us.
///
/// Returns 1.0 when there is no preference, on failure, and on non-macOS.
#[tauri::command]
pub fn get_system_text_scale() -> f32 {
    #[cfg(target_os = "macos")]
    {
        macos::text_scale()
    }
    #[cfg(not(target_os = "macos"))]
    {
        1.0
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::sync::atomic::{AtomicBool, Ordering};

    use block2::RcBlock;
    use objc2::AnyThread;
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{
        NSDistributedNotificationCenter, NSNotification, NSNotificationCenter, NSString,
        NSUserDefaults,
    };

    use super::{AccessibilityPrefs, SystemChange, SYSTEM_CHANGES};

    /// Category → multiplier, keyed by the SUFFIX.
    ///
    /// macOS stores the short name ("XL"), while the constants inside the
    /// accessibility framework are the long `UICTContentSizeCategoryXL` form.
    /// Matching only the long form silently returned 1.0 on a machine that was
    /// actually set to XL — which looks exactly like "the feature does not
    /// work". Both forms are accepted, and the prefix is stripped first.
    ///
    /// The multipliers are ours: macOS exposes a category, not a factor. They
    /// track the same shape as iOS's Dynamic Type steps.
    const CATEGORIES: &[(&str, f32)] = &[
        ("XS", 0.82),
        ("S", 0.88),
        ("M", 0.94),
        ("L", 1.0),
        ("XL", 1.12),
        ("XXL", 1.24),
        ("XXXL", 1.35),
        ("AccessibilityM", 1.6),
        ("AccessibilityL", 1.9),
        ("AccessibilityXL", 2.2),
        ("AccessibilityXXL", 2.6),
        ("AccessibilityXXXL", 3.0),
    ];

    pub fn text_scale() -> f32 {
        let Some(defaults) = NSUserDefaults::initWithSuiteName(
            NSUserDefaults::alloc(),
            Some(&NSString::from_str("com.apple.universalaccess")),
        ) else {
            return 1.0;
        };
        let key = NSString::from_str("FontSizeCategory");
        let Some(dict) = defaults.dictionaryForKey(&key) else {
            return 1.0;
        };
        let global = NSString::from_str("global");
        let Some(value) = dict.objectForKey(&global) else {
            return 1.0;
        };
        // The value is an NSString category name; DEFAULT means "not set".
        let name = value.downcast_ref::<NSString>().map(|s| s.to_string());
        match name.as_deref() {
            None | Some("DEFAULT") | Some("UseGlobal") => 1.0,
            Some(n) => {
                let suffix = n.strip_prefix("UICTContentSizeCategory").unwrap_or(n);
                CATEGORIES
                    .iter()
                    .find(|(c, _)| *c == suffix)
                    .map(|(_, m)| *m)
                    .unwrap_or(1.0)
            }
        }
    }

    pub fn accessibility_prefs() -> AccessibilityPrefs {
        let workspace = NSWorkspace::sharedWorkspace();
        let defaults = NSUserDefaults::standardUserDefaults();
        let size_key = NSString::from_str("NSTableViewDefaultSizeMode");
        let size = defaults.integerForKey(&size_key);
        AccessibilityPrefs {
            reduce_motion: workspace.accessibilityDisplayShouldReduceMotion(),
            reduce_transparency: workspace.accessibilityDisplayShouldReduceTransparency(),
            increase_contrast: workspace.accessibilityDisplayShouldIncreaseContrast(),
            differentiate_without_color: workspace
                .accessibilityDisplayShouldDifferentiateWithoutColor(),
            // 0 means unset; macOS's own default is medium.
            sidebar_icon_size: if (1..=3).contains(&size) {
                size as i64
            } else {
                2
            },
        }
    }

    pub fn full_keyboard_access() -> bool {
        let defaults = NSUserDefaults::standardUserDefaults();
        let key = NSString::from_str("AppleKeyboardUIMode");
        // 0 = text fields and lists only; 2 (bit 1) = every control.
        defaults.integerForKey(&key) & 2 != 0
    }

    static STARTED: AtomicBool = AtomicBool::new(false);

    /// Notification names, and what each one means to us. Several are observed
    /// for the text-size category because the name has moved between releases —
    /// an extra observer that never fires costs nothing; a missed change means
    /// the app silently ignores an accessibility setting.
    /// A notification name and what it means to us.
    type Watch = (&'static str, fn() -> SystemChange);

    const WATCHED: &[Watch] = &[
        ("AppleColorPreferencesChangedNotification", || {
            SystemChange::Accent
        }),
        ("AppleInterfaceThemeChangedNotification", || {
            SystemChange::Appearance
        }),
        ("AppleAquaColorVariantChanged", || SystemChange::Accent),
        // Sidebar icon size is an Appearance preference.
        ("AppleNoRedisplayAppearancePreferenceChanged", || {
            SystemChange::Accessibility
        }),
        (
            "ApplePreferredContentSizeCategoryChangedNotification",
            || SystemChange::TextSize,
        ),
        ("UAPFontSizeCategoryDidChange", || SystemChange::TextSize),
        (
            "com.apple.universalaccess.FontSizeCategoryDidChange",
            || SystemChange::TextSize,
        ),
        // Keyboard navigation is toggled in System Settings → Keyboard.
        ("AppleKeyboardUIModeChangedNotification", || {
            SystemChange::KeyboardAccess
        }),
    ];

    pub fn start_observing() {
        if STARTED.swap(true, Ordering::SeqCst) {
            return;
        }
        // The display options post on the WORKSPACE's notification centre, not
        // the distributed one — observing only the distributed centre would miss
        // every one of them.
        {
            let center = NSWorkspace::sharedWorkspace().notificationCenter();
            let block = RcBlock::new(move |_note: core::ptr::NonNull<NSNotification>| {
                SYSTEM_CHANGES.send(SystemChange::Accessibility);
            });
            unsafe {
                center.addObserverForName_object_queue_usingBlock(
                    Some(&NSString::from_str(
                        "NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification",
                    )),
                    None,
                    None,
                    &block,
                );
            }
        }
        let center = NSDistributedNotificationCenter::defaultCenter();
        // The block-based observer lives on NSNotificationCenter, the superclass;
        // the distributed centre only re-exports the selector-based variants.
        let center: &NSNotificationCenter = &center;
        for (name, kind) in WATCHED {
            let kind = *kind;
            // Deliver on the main queue's default mode. The block only sends on
            // a channel, so it does no work worth moving off the caller.
            let block = RcBlock::new(move |_note: core::ptr::NonNull<NSNotification>| {
                SYSTEM_CHANGES.send(kind());
            });
            unsafe {
                center.addObserverForName_object_queue_usingBlock(
                    Some(&NSString::from_str(name)),
                    None,
                    None,
                    &block,
                );
            }
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    /// Reads the real machine's setting — the point is that the plumbing works
    /// against the actual preference domain, which is where the earlier guesses
    /// about this feature went wrong. The assertion is a range, not a value, so
    /// it holds whatever the machine is set to.
    /// Reads the machine's real display preferences. Asserts shape rather than
    /// values, so it holds however the machine is configured — the point is that
    /// the bridge reaches the actual settings.
    #[test]
    fn accessibility_prefs_read_the_real_settings() {
        let p = super::get_accessibility_prefs();
        println!(
            "reduceMotion={} reduceTransparency={} increaseContrast={} differentiate={} sidebarIconSize={}",
            p.reduce_motion,
            p.reduce_transparency,
            p.increase_contrast,
            p.differentiate_without_color,
            p.sidebar_icon_size,
        );
        assert!(
            (1..=3).contains(&p.sidebar_icon_size),
            "sidebar icon size {} is outside macOS's 1..3",
            p.sidebar_icon_size,
        );
    }

    #[test]
    fn text_scale_reads_the_system_category() {
        let scale = super::get_system_text_scale();
        println!("system text scale on this machine: {scale}");
        assert!(
            (0.8..=3.0).contains(&scale),
            "implausible scale {scale} — the category lookup is wrong",
        );
    }
}
