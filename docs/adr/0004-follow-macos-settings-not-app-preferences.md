# Follow macOS settings rather than adding app preferences

**Status:** accepted (2026-07-27)

## Decision

When macOS already has a setting for something, TraceLoupe **follows that
setting** instead of adding its own preference for it. New app preferences are
for choices the system has no opinion about.

## Context

Keyboard focus in the app felt noisy: tabbing through a view stopped at every
button and every row. The proposal on the table was an in-app toggle to turn the
focus behaviour off.

Measuring first changed the question. macOS has **Keyboard navigation**
(`AppleKeyboardUIMode`), which decides whether Tab visits every control or only
text fields and lists — and it was **off** on the machine in question. Native
apps follow it; we ignored it, so Tab had 46 stops in Messages and 58 in Safety
Scan where a native app would offer a handful. The app was behaving as though
full keyboard access had been requested when it explicitly had not.

The same pattern held everywhere we looked: eleven settings the OS exposes and
we ignored — accent, selection colour, text size, reduce motion, reduce
transparency, increase contrast, differentiate without colour, sidebar icon
size, scroll bars, appearance, locale.

## Why not an app toggle

- **It duplicates a setting the user has already made.** They set it once, for
  every app; ours would be a second place to say the same thing, and the only
  people who would find it are the ones who already knew to look.
- **It frames a defect as a preference.** The complaint was rings in the wrong
  places; the cause was tab stops in the wrong places. A toggle would hide the
  symptom and leave keyboard navigation just as unusable for anyone who needs it.
- **It is a bad switch to have within reach.** This is a tool people may use in
  genuinely bad situations. A control that quietly removes keyboard access is not
  something to offer casually.

## Consequences

- Honouring a system setting must be *complete*, or it reads as broken. Removing
  buttons from the tab order without giving lists arrow-key navigation would have
  been a regression wearing the costume of a fix.
- Some settings do not reach us at all and have to be read directly — macOS's
  Text Size never reaches AppKit metrics or WebKit fonts, so we read the category
  and apply it ourselves. See `docs/reference/macos-integration.md`.
- Dialogs deliberately stay fully tabbable even when the setting is off: a modal
  must be completable from the keyboard, and unlike a toolbar there is nowhere
  else its buttons can be reached from.
- Where the platform's own choice is below a standard we would otherwise hold —
  macOS's secondary label tiers fail WCAG AA — we follow the platform and record
  the measurement with a floor in `check-design.mjs`, rather than silently
  diverging or silently accepting.

## Alternatives considered

**An in-app toggle** — rejected above.

**Ignoring the settings** (the status quo) — this is what produced the
complaint. An app that ignores accessibility settings is not neutral about them;
it overrides them.
