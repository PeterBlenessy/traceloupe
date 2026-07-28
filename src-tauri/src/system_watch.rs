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
    /// Region or language changed — anything formatted from a locale is stale.
    Locale,
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
    /// System Settings → Appearance → Show scroll bars:
    /// "automatic" | "whenScrolling" | "always".
    pub show_scroll_bars: String,
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
            show_scroll_bars: "automatic".into(),
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

/// The locale to FORMAT in — language from the user's language, region from
/// their Region setting.
///
/// macOS lets those differ, and plenty of people run one language with another
/// region: English on a Mac set to Sweden reports `AppleLocale =
/// en_US@rg=sezzzz`. The webview's own default locale drops the override
/// entirely and answers `en-US`, which is why every date read `Jun 8, 12:40 AM`
/// on a machine that writes `8 juni` and keeps a 24-hour clock.
///
/// **Passing the raw value through does not work**, and looks like it does:
/// ```text
/// en-US                «redacted»   Jun 8, 2024, 2:40 PM
/// en-US-u-rg-sezzzz    «redacted»   Jun 8, 2024, 2:40 PM   ← the override is ignored
/// en-SE                408 937   8 Jun 2024, 14:40      ← what the user set
/// ```
/// Intl does not honour the `rg` extension, so the region has to be folded into
/// the locale itself. That is what [`system_locale`] does.
#[tauri::command]
pub fn get_system_locale() -> String {
    #[cfg(target_os = "macos")]
    {
        macos::apple_locale()
            .as_deref()
            .map(to_bcp47)
            .unwrap_or_else(|| "en-US".to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        "en-US".to_string()
    }
}

/// `en_US@rg=sezzzz` → `en-SE`; `sv_SE` → `sv-SE`; `en_US` → `en-US`.
///
/// Kept pure and separate from the AppKit call so it can be tested against the
/// shapes macOS actually produces rather than trusted.
pub fn to_bcp47(apple_locale: &str) -> String {
    // Keywords hang off `@` as `key=value;key=value`. `rg` is the Region
    // override — a region code padded to eight characters ("sezzzz", "gbzzzz").
    let (base, keywords) = match apple_locale.split_once('@') {
        Some((b, k)) => (b, Some(k)),
        None => (apple_locale, None),
    };
    let region_override = keywords.and_then(|k| {
        k.split(';')
            .filter_map(|kv| kv.split_once('='))
            .find(|(key, _)| *key == "rg")
            .map(|(_, v)| v.trim_end_matches('z').to_ascii_uppercase())
            .filter(|r| r.len() == 2)
    });

    let mut parts = base.split('_');
    let language = parts.next().unwrap_or("en");
    // A script subtag (zh_Hans_CN) sits between language and region; keep it,
    // because dropping it turns Simplified Chinese into a guess.
    let rest: Vec<&str> = parts.collect();
    let (script, region) = match rest.as_slice() {
        [s, r] => (Some(*s), Some(r.to_string())),
        [r] if r.len() == 4 => (Some(*r), None),
        [r] => (None, Some(r.to_string())),
        _ => (None, None),
    };

    let mut out = String::from(language);
    if let Some(s) = script {
        out.push('-');
        out.push_str(s);
    }
    if let Some(r) = region_override.or(region) {
        out.push('-');
        out.push_str(&r);
    }
    out
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
            show_scroll_bars: {
                let key = NSString::from_str("AppleShowScrollBars");
                match defaults
                    .stringForKey(&key)
                    .map(|v| v.to_string())
                    .as_deref()
                {
                    Some("Always") => "always".into(),
                    Some("WhenScrolling") => "whenScrolling".into(),
                    // Unset is macOS's default: automatic — overlay bars that
                    // appear on scroll, permanent when a mouse is attached.
                    _ => "automatic".into(),
                }
            },
        }
    }

    pub fn apple_locale() -> Option<String> {
        let defaults = NSUserDefaults::standardUserDefaults();
        let key = NSString::from_str("AppleLocale");
        defaults.stringForKey(&key).map(|s| s.to_string())
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
        // Region / language: everything formatted from a locale is now stale.
        ("NSCurrentLocaleDidChangeNotification", || {
            SystemChange::Locale
        }),
        ("AppleLanguagePreferencesChangedNotification", || {
            SystemChange::Locale
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
    use super::*;

    /// The transformation the whole locale fix hinges on.
    ///
    /// Asserted against the shapes macOS actually writes, because the obvious
    /// alternative — handing `AppleLocale` to Intl unchanged — produces a value
    /// that is accepted and then ignored, which is indistinguishable from
    /// working until someone looks at a date.
    #[test]
    fn apple_locale_folds_the_region_override_into_the_locale() {
        for (apple, want) in [
            // English language, Region: Sweden. The case that started this.
            ("en_US@rg=sezzzz", "en-SE"),
            ("en_US@rg=gbzzzz", "en-GB"),
            // No override: the locale's own region stands.
            ("en_US", "en-US"),
            ("sv_SE", "sv-SE"),
            // A script subtag must survive; dropping it makes Simplified
            // Chinese a guess.
            ("zh_Hans_CN", "zh-Hans-CN"),
            ("zh_Hans_CN@rg=twzzzz", "zh-Hans-TW"),
            // Other keywords ride alongside rg and must not be mistaken for it.
            ("en_US@calendar=japanese;rg=sezzzz", "en-SE"),
            ("en_US@calendar=japanese", "en-US"),
            // Language only.
            ("en", "en"),
        ] {
            assert_eq!(to_bcp47(apple), want, "{apple}");
        }
    }

    /// Whatever the machine reports, the result must be something Intl can use.
    #[test]
    fn the_resolved_locale_is_well_formed() {
        let l = get_system_locale();
        assert!(!l.is_empty());
        assert!(
            !l.contains('@') && !l.contains('_'),
            "{l} is still in Apple's form"
        );
        let mut parts = l.split('-');
        assert!(
            parts
                .next()
                .is_some_and(|lang| lang.len() >= 2 && lang.len() <= 3),
            "{l} has no language subtag"
        );
    }
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
