# App support tracker

A **living table** of what TraceLoupe reads, and the version each source gains
**native** support — i.e. an in-house Rust parser reading the backup directly,
with no dependency on the iLEAPP sidecar. Update a row the moment its status
changes; this file is the single source of truth for coverage.

Companion to `../CHANGELOG.md` (milestones) and
`../product-architecture-description.md` §13 (roadmap rationale).

**Last updated:** 2026-07-14 · current release **0.2.0**

## Status legend

| Mark | Meaning |
|------|---------|
| ✅ **Native** | Own Rust parser, no iLEAPP. The "Native since" column gives the version. |
| 🟡 **Via iLEAPP** | Surfaced today, but through the iLEAPP engine — not yet native. |
| ⬜ **Planned** | Not surfaced in any path yet; a target version is listed when scheduled. |
| ⚪ **Little local data** | The app deliberately stores little/nothing recoverable locally (encrypted or server-side), so there may be nothing to parse regardless of engine. |

> "Native since" is filled only when the **native** parser ships. An app can be
> 🟡 (readable via iLEAPP) for several releases before its ✅ native row lands.

## First-party (device data)

| Data | Source file | Status | Native since |
|------|-------------|--------|--------------|
| Messages (iMessage/SMS) | `sms.db` | ✅ Native | 0.2.0 |
| Group-chat names/members | `sms.db` (`chat.db` schema) | ✅ Native | 0.2.0 |
| Notes (incl. locked & pinned) | `NoteStore.sqlite` | ✅ Native | 0.2.0 |
| Voice recordings | `CloudRecordings.db` + `.m4a` | ✅ Native | 0.2.0 |
| Camera roll (photos/videos) | `Photos.sqlite` / DCIM | ✅ Native | 0.1.0 |
| Call history | `CallHistory.storedata` | ✅ Native | 0.3.0 |
| Safari history | `History.db` | ✅ Native | 0.3.0 |
| Contacts | `AddressBook.sqlitedb` | ✅ Native | 0.3.0 |
| Installed apps | app-state plist | 🟡 Via iLEAPP | 0.3.0 (planned) |

## Third-party apps

Tiers mirror the roadmap (`product-architecture-description.md` §13.1), ordered by
prevalence × local-data richness × parse feasibility. Native rollout is by
**scheduled batch, not strict tier order** — an app can be pulled forward.

**Batch 1 (0.3.0) — first native third-party wave.** Alongside the Apple-app
parity work, these get native parsers in 0.3.0 — prioritizing apps not yet
surfaced at all:

> TikTok (already surfaced via iLEAPP) · Instagram · Facebook ·
> Facebook Messenger · X/Twitter · Snapchat

**WhatsApp and Telegram are deferred to 0.4.0.** They already read through iLEAPP,
so there's no urgency to make them native first; they stay 🟡 until then.

Everything else stays ⬜ Planned until scheduled into a later batch.

### Tier 1 — Top 10

| App | What's stored locally | Status | Native since |
|-----|-----------------------|--------|--------------|
| WhatsApp | Messages (rich local SQLite) | 🟡 Via iLEAPP — messages | 0.4.0 (deferred) |
| TikTok | Messages + social-graph contacts | 🟡 Via iLEAPP — messages, contacts | 0.3.0 (Batch 1) |
| Telegram | Messages (cloud-synced; local cache varies) | 🟡 Via iLEAPP — messages | 0.4.0 (deferred) |
| Instagram | DMs, media cache | ⬜ Planned | 0.3.0 (Batch 1) |
| Facebook | Feed cache, messages, media | ⬜ Planned | 0.3.0 (Batch 1) |
| Facebook Messenger | Messages, media | ⬜ Planned | 0.3.0 (Batch 1) |
| Snapchat | Minimal — ephemeral by design (⚠ thin local store) | ⬜ Planned | 0.3.0 (Batch 1) |
| YouTube | Watch/search history, cache | ⬜ Planned | TBD |
| Gmail | Cached mail/metadata | ⬜ Planned | TBD |
| WeChat | Messages, media | ⬜ Planned | TBD |
| Signal | Minimal — encrypted local store | ⚪ Little local data | — |

### Tier 2 — Top 25 (adds)

| App | Status | Native since |
|-----|--------|--------------|
| X / Twitter | ⬜ Planned | 0.3.0 (Batch 1) |
| Discord | ⬜ Planned | TBD |
| Reddit | ⬜ Planned | TBD |
| Spotify | ⬜ Planned | TBD |
| LinkedIn | ⬜ Planned | TBD |
| Pinterest | ⬜ Planned | TBD |
| Threads | ⬜ Planned | TBD |
| Viber | ⬜ Planned | TBD |
| LINE | ⬜ Planned | TBD |
| Google Maps | ⬜ Planned | TBD |

### Tier 3 — Top 50 (adds)

| App | Status | Native since |
|-----|--------|--------------|
| Slack | ⬜ Planned | TBD |
| Microsoft Teams | ⬜ Planned | TBD |
| Zoom | ⬜ Planned | TBD |
| Twitch | ⬜ Planned | TBD |
| Tinder / Bumble / Hinge | ⬜ Planned | TBD |
| PayPal / Venmo / Cash App | ⬜ Planned | TBD |
| Uber | ⬜ Planned | TBD |
| Amazon | ⬜ Planned | TBD |
| Strava | ⬜ Planned | TBD |
| Notion | ⬜ Planned | TBD |

### Tier 4 — Top 100 (the long tail)

Regional messengers, banking/fintech, travel, fitness, productivity apps, and
popular games. Enumerated per-app as they're scheduled; add rows here when a
specific app is picked up.

## How to update this file

1. When an app/source gains **native** support, flip its status to ✅ and set
   **Native since** to the shipping version.
2. When an app is first surfaced via iLEAPP, set it 🟡 and note which artifacts.
3. Keep the **Last updated** line and `CHANGELOG.md` in step.
4. Apps that store nothing usefully local stay ⚪ — record *why* so we don't keep
   re-investigating them.
