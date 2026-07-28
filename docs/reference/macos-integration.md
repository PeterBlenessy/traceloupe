# macOS integration — following the system instead of guessing

TraceLoupe follows eleven macOS settings and adopts each one **while running**,
not at the next window focus. This is the reference for how that works, what it
cost to find out, and how to add the twelfth.

The short version: **the values come over a Rust bridge, not from CSS media
queries.** WebKit understands the queries; a WKWebView never resolves them to the
system's values. Every "we can get this for free" instinct here turned out to be
wrong, and each was expensive enough to be worth writing down.

## What is adopted

| Setting | Read from | Reaches the UI as | Changes |
|---|---|---|---|
| Accent colour | `NSColor.controlAccentColor` | `--accent-system-value` | every accented surface |
| Selection colour | `NSColor.selectedContentBackgroundColor` | `--selected-system-value` | selected rows |
| Appearance (light/dark) | `AppleInterfaceStyle` | `color-scheme` + theme class | the whole palette |
| Text size | `com.apple.universalaccess` → `FontSizeCategory` | `--system-text-scale` | the type ramp, **frame included** |
| Keyboard navigation | `AppleKeyboardUIMode` | `data-full-keyboard-access` | what Tab reaches |
| Reduce motion | `NSWorkspace.accessibilityDisplayShouldReduceMotion` | `data-reduce-motion` | transitions and animations |
| Reduce transparency | `…ShouldReduceTransparency` | `data-reduce-transparency` | the frosted title bar |
| Increase contrast | `…ShouldIncreaseContrast` | `data-increase-contrast` | borders, secondary text tiers |
| Differentiate without colour | `…ShouldDifferentiateWithoutColor` | `data-differentiate-without-color` | severity gains a glyph |
| Sidebar icon size | `NSTableViewDefaultSizeMode` | `data-sidebar-icon-size` | 16 / 20 / 24 px icons |
| Show scroll bars | `AppleShowScrollBars` | `data-scroll-bars` | gutter and thumb |

Plus **locale**, which is not a value but an invalidation: a region change
re-keys the view subtree so dates, times and numbers re-format.

## How it flows

1. **`src-tauri/src/system_watch.rs`** registers OS observers once and exposes
   the readers as commands (`get_accessibility_prefs`, `get_system_text_scale`,
   `get_full_keyboard_access`); `theme.rs` reads the two colours.
2. When the OS announces a change, the watcher sends a `SystemChange` over a
   **Channel** saying only *what* changed.
3. **`src/lib/use-system-accent.ts`** re-reads that value through the same
   command it used at startup and stamps `<html>`.
4. CSS in **`src/index.css`** keys off the attributes; `use-keyboard-nav.ts`
   keys off `data-full-keyboard-access`.

The notification deliberately carries no payload. One code path reads a setting
whether it runs at startup or after a change — a push carrying the new value
would be a second path, free to disagree with the first.

## What we learned the hard way

**WebKit's `prefers-*` media queries are free syntax, not free behaviour.**
`prefers-reduced-motion`, `prefers-contrast` and `prefers-reduced-transparency`
are all *supported* in a WKWebView and all report `false` on a machine where
AppKit reports them correctly. Anything that needs a system value needs the
bridge.

**macOS's Text Size never reaches us.** Measured at category XL with
`scripts/font-probe.swift`: `NSFont.systemFontSize` stayed 13, every text style
kept its default, and WebKit's `-apple-system-body` stayed 13px. The setting only
reaches apps using `UIPreferredContentSizeCategory` — SwiftUI/UIKit text styles.
An AppKit + WebKit app either reads the category itself or ignores the user.

**The category is stored short.** `FontSizeCategory` holds `"XL"`, not
`"UICTContentSizeCategoryXL"` — the long constants exist in the accessibility
framework but not in the preference. Matching only the long form returns 1.0 on a
machine that is set to XL, which is indistinguishable from "the feature does not
work".

**The accessibility display options post on the *workspace* notification
centre**, not the distributed one. Everything else here is distributed, so
observing only that centre silently misses all four of them.

**The block-based observer lives on `NSNotificationCenter`**, the superclass —
`NSDistributedNotificationCenter` re-exports only the selector-based variants —
and it needs objc2-foundation's `block2` feature.

**`com.apple.universalaccess` is TCC-protected.** It reads fine and refuses
writes (`defaults write` exits with "Could not write domain"), so tests read the
real machine and never mutate it. Changing a system setting to test something is
the user's action, not ours.

**Two scales, and they are not interchangeable.** `--system-text-scale` is the
macOS setting and multiplies the ramp *everywhere including the frame*, because
someone who enlarged system text needs the toolbar legible too.
`--text-scale` is the in-app A+/− reading preference and scales content only.

## The engine divergence that keeps biting

`scripts/shot.mjs` and `scripts/check-design.mjs` render in **Chromium**; the app
ships **WKWebView**. They disagree in ways that make a screenshot a poor witness:

- `-apple-system-label` / `-apple-system-body` resolve in WebKit and are unknown
  to Chromium. A custom property holding an unknown keyword is invalid *at use
  time*, so the two-declaration fallback trick does not work — hence the
  `@supports (color: -apple-system-label)` guard, which is load-bearing rather
  than decorative.
- `:focus-visible` differs for programmatic focus. Radix focusing a dialog's
  first control drew a ring in WebKit and none in Chromium, which is why the
  Settings "General" ring was invisible to the harness and obvious on screen.

When a UI question involves platform CSS or focus, ask WKWebView — a few lines of
Swift (see `scripts/font-probe.swift`) answers it in seconds.

## Adding another setting

1. Read it in `system_watch.rs` (or `theme.rs` for a colour) and add it to the
   returned struct.
2. Observe the notification that announces it — check whether it is distributed
   or workspace; guessing wrong fails silently.
3. Add a `SystemChange` variant, or reuse `Accessibility` if it travels with the
   display options.
4. Stamp it in `use-system-accent.ts` and consume it from `index.css`.
5. **Measure it on and off.** The sidebar icon rule shipped as a no-op in review
   because "large" was set to 1.25rem and the existing icon was already 20px.
   A setting that appears to work and does nothing is the default failure here.

Tests in `system_watch.rs` read the machine's actual settings and assert shape
rather than values, so they hold however the machine is configured — the point is
that the bridge reaches the real preference domain, which is precisely where the
guesses went wrong.

## Language and Region are separate, and Intl only honours one of them

macOS lets you pick a language and a Region independently. A Mac with English
and Region: Sweden reports:

```
$ defaults read -g AppleLocale
en_US@rg=sezzzz
```

The webview's default locale **drops the region override** and answers `en-US`.
So `new Intl.DateTimeFormat(undefined, …)` — the obvious call, and what this app
used everywhere — formatted every date for the United States on a machine set to
Sweden: `Jun 8, 12:40 AM` where the user expects `8 Jun, 0:40`.

**The obvious fix does not work, and looks like it does.** Passing `AppleLocale`
through verbatim is accepted and then ignored, because Intl does not implement
the `rg` extension:

| locale | number | date |
|---|---|---|
| `en-US` | «redacted» | Jun 8, 2024, 2:40 PM |
| `en-US-u-rg-sezzzz` | «redacted» | Jun 8, 2024, 2:40 PM |
| `en-SE` | 408 937 | 8 Jun 2024, 14:40 |

So the region has to be folded into the locale itself: `system_watch::to_bcp47`
turns `en_US@rg=sezzzz` into `en-SE`, and `get_system_locale` hands that to the
frontend. `src/lib/format.ts` holds it and every formatter in the app takes it.

Two things make this stay fixed rather than drift back:

- the `locale` rule in `scripts/check-design.mjs` fails on any `toLocaleString()`
  or `Intl.*(undefined, …)` outside `format.ts`;
- `to_bcp47` is tested against the shapes macOS actually writes — including a
  script subtag (`zh_Hans_CN@rg=twzzzz` → `zh-Hans-TW`) and other keywords
  riding alongside `rg`.

`formatCount` deserves its own mention: it used to insert a space with a regex
and consult no locale at all, which made it correct for Sweden by accident and
wrong for every US reader.
