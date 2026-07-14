# App support tracker

A **living table** of what TraceLoupe reads, and the version each source gains
**native** support — i.e. an in-house Rust parser reading the backup directly,
with no dependency on the iLEAPP sidecar. Update a row the moment its status
changes; this file is the single source of truth for coverage.

Companion to `../CHANGELOG.md` (milestones) and
`../product-architecture-description.md` §13 (roadmap rationale). For *field-level*
coverage within each app — everything in its DB and what we surface — see
[`app-data-coverage.md`](app-data-coverage.md).

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
| Installed apps | `Info.plist` (Installed Applications) | ✅ Native | 0.1.0 |

**All first-party views are now native** — iLEAPP is no longer required for any
built-in view. It's still invoked only for the third-party chats below (until
those go native), after which it becomes optional (Batch 2).

## Third-party apps

Tiers mirror the roadmap (`product-architecture-description.md` §13.1), ordered by
prevalence × local-data richness × parse feasibility. Native rollout is by
**scheduled batch, not strict tier order** — an app can be pulled forward.

**Native app-module framework** (`parsers/apps/`): each app is a small module that
locates its own DB and parses it into a shared message stream; one shared inserter
writes the threads/messages. Adding an app is one module file + a registry entry.
**WhatsApp is the first module** (cleanest schema — used to validate the framework).

**Batch 1 (0.3.0) — native third-party wave.** Which apps are feasible is set by
what's actually in the backup (confirmed by whether iLEAPP even has a module):

> ✅ done: **WhatsApp**, **Facebook Messenger** (both clean SQLite).
> Remaining, harder: **TikTok** (protobuf message bodies), **Instagram** (messages
> stored as archived plists) — framework-ready, need a blob decoder.
> Investigate (data exists but not open-source-documented — see Research notes):
> **Snapchat**, **X/Twitter**, **Facebook** (main app).

**Telegram is deferred to 0.4.0** (already reads through iLEAPP; no urgency).

**Research notes — apps with no iLEAPP module** (web research, July 2026; "no
iLEAPP module" ≠ "no data"). These need a real backup to pin exact schemas before
a parser is safe to write:
- **Snapchat** (`com.toyopagroup.picaboo`) — iOS *does* persist chats, contacts,
  best-friends, and memories in the app's `Data/Application/<uuid>/` container
  (SQLite under a `databases/`-style folder; message receipts too). Records
  survive "Clear Conversation" in WAL/freelist. **Caveat:** the store is often
  **encrypted** (Cellebrite/others decrypt it), so it may need key material — the
  reason iLEAPP has no module. Upgraded from "no data" to *investigate*.
- **X/Twitter** (`com.atebits.tweetie2`) — no documented clean message DB; DMs
  aren't in a well-known SQLite. Cached API responses live in the **generic
  `Cache.db`** (unencrypted CFURL cache: `cfurl_cache_response` +
  `cfurl_cache_blob_data`). Best-effort carving, not a clean store.
- **Facebook main app** (`com.facebook.Facebook`) — chats route through Messenger
  (already native); the rest is feed/media in `Cache.db`.
- **Generic `Cache.db` opportunity:** nearly every app has
  `Library/Caches/<bundle>/Cache.db` holding cached network content. One generic
  native module (iLEAPP does this via `fsCachedData`/`cachev0`/`parsecdCache`)
  could surface cached data across many apps at once — a strong future addition.

Sources: forensafe (iOS Snapchat/Messenger), Cellebrite & xperylab (Snapchat
decryption), TrustedSec (iOS `Cache.db`), SANS ISC / AboutDFIR (iOS app artifacts).

### Tier 1 — Top 10

| App | What's stored locally | Status | Native since |
|-----|-----------------------|--------|--------------|
| WhatsApp | Messages (rich local SQLite) | ✅ Native — messages | 0.3.0 |
| Facebook Messenger | Messages (`lightspeed` SQLite) | ✅ Native — messages | 0.3.0 |
| TikTok | Messages (protobuf) + social-graph contacts | 🟡 Via iLEAPP — messages, contacts | 0.3.0 (needs protobuf) |
| Instagram | DMs as archived plists | ⬜ Planned | 0.3.0 (needs plist decode) |
| Telegram | Messages (cloud-synced; local cache varies) | 🟡 Via iLEAPP — messages | 0.4.0 (deferred) |
| Facebook (main app) | Chats via Messenger (done); feed/media in generic `Cache.db` | ⬜ Investigate (Cache.db) | TBD |
| Snapchat | Chats/contacts DO persist on iOS (often encrypted) | ⬜ Investigate (real backup) | TBD |
| YouTube | Watch/search history, cache | ⬜ Planned | TBD |
| Gmail | Cached mail/metadata | ⬜ Planned | TBD |
| WeChat | Messages, media | ⬜ Planned | TBD |
| Signal | Encrypted local store | ⚪ Little local data | — |

### Tier 2 — Top 25 (adds)

| App | Status | Native since |
|-----|--------|--------------|
| X / Twitter | ⬜ Investigate — DMs not in a clean DB; cached in generic `Cache.db` | TBD |
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
