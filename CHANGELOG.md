# Changelog

All notable changes to **TraceLoupe** are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project uses
[Semantic Versioning](https://semver.org/).

While pre-1.0, the **minor** version tracks major milestones:

| Version | Milestone |
|---------|-----------|
| `0.1.0` | **MVP** — iLEAPP-powered import into a local cache, with a native browsing UI. |
| `0.2.0` | **Native lazy-decode core, wired in** — Manifest Index + on-demand decryption + native Messages/Notes/Recordings/Camera-roll parsers, running *alongside* iLEAPP (which still supplies Calls, Safari, Apps, third-party chats). |
| `0.3.0` | **Native-first, Batch 1** — all first-party views native (Calls, Safari, Apps, Contacts); a pluggable native app-chat framework with WhatsApp, Facebook Messenger, Instagram & TikTok. iLEAPP still runs for Telegram, TikTok contacts, and the long tail. |
| `0.4.0` | **More native chat apps** — Telegram (binary postbox), Kik, imo, Threema, via the app-chat framework. iLEAPP still runs for the long tail. |
| `0.5.0` | **More native chat apps** — Viber, Microsoft Teams, via the app-chat framework. |
| `0.6.0` | **LinkedIn** native chat. |
| `0.7.0`+ | **Native-first, continued** — remaining apps (need heavier machinery), then make iLEAPP an optional on-demand engine. See "Planned" below. |

> The single source of truth for the version is `package.json`; keep the
> workspace `Cargo.toml` and `src-tauri/tauri.conf.json` in step when it changes.

## [Unreleased]

_Nothing yet._

## [0.7.0]

### Changed

- **Timeline rows redesigned** to a single flat line — avatar · direction · message ·
  app icon · time — with the message free to wrap over multiple lines. The
  always-the-owner conversation phone number is gone, and the source app is now
  just its brand icon (no "iMessage"/etc. text), pinned to the left of the time.
- **Timeline rows now show the conversation partner and direction.** The avatar is
  always the other party (so every row makes clear which chat it belongs to, even
  your own outgoing messages), and a direction arrow marks sent (→, tinted) vs
  received (←). Backed by a new `threadHandle` on each timeline row.
- **All large counts now use a thousands separator** (`450 897`, non-breaking) so
  they read clearly and never wrap mid-number.

### Added

- **Native Safari bookmarks, reading list, and open tabs.** New parsers read
  `Bookmarks.db` (bookmarks + reading-list items, with their added/viewed dates
  and preview text) and `SafariTabs.db` (open tabs, grouped by tab group). The
  Safari view gains a **type filter** on the title row — History · Bookmarks ·
  Reading List · Tabs — with the same search + time filter + sort across all of
  them.
- **Back button** in a conversation opened from the Timeline view, returning you
  to that overview.
- **Timeline time filters.** Merged the separate Periods view into Timeline: the
  toolbar now carries quick-filter chips (All · 24h · 7d · 30d · year, each with
  its message count) plus a custom from–to date range, left-aligned beside the
  sort control. Selecting a chip or range filters the stream (rather than the old
  jump-to-bucket behaviour).

- **Time filters on Photos, Notes, and Safari.** The same preset chips + custom
  from–to range as the Timeline now filter Photos (by capture date, server-side),
  Notes (by modified date), and Safari history (by visit date, server-side). On
  Photos the app/source filter moved up beside the title; on Notes the time chips
  replace the old year dropdown.
- **Notes layout** rebuilt into full-width rows: title + folder + lock state (now
  with lock/unlock icons) on the first row, time filters + sort on the second.
- **Brand icons on the Photos source filter** (same treatment as the message
  filter chips).

### Removed

- The standalone **Periods** view (folded into Timeline's filters, above).
- The Notes **year dropdown** (superseded by the time-filter chips + range).

## [0.6.3]

### Fixed

- **Message views no longer stick while scrolling.** The lazy virtual list was
  measuring not-yet-loaded placeholder rows as their true height, collapsing the
  total size and then re-expanding each row as its window resolved — the jump
  that made Timeline/Periods/conversation scrolling feel frozen. Unloaded rows
  now reserve their estimated height and are never measured; only real content
  is. Also disables the browser's own scroll-anchoring so it can't fight the
  virtualizer.
- **Timeline & Periods now show which conversation each message belongs to.**
  Rows led with the sender only; the conversation is now the primary label
  (making clear who a 1:1 was with / which group), with the sender shown as a
  prefix on the snippet for your own and group messages.

### Added

- **Sort messages by time direction** (oldest-first ↔ newest-first) in the
  Timeline, Periods, and conversation views — previously only the conversation
  *list* could be sorted. Newest-first pins the newest message to the top;
  oldest-first keeps the chat-like newest-at-bottom layout.

## [0.6.0] — 2026-07-15

### Added
- **LinkedIn** (`Documents/msg_database.sqlite`) — messages grouped by
  `conversationUrn`; sender, direction (`distance == "SELF"`), and body decoded
  from the `serializedMessage` JSON; the chat name from the non-owner participant
  in `serializedConversation`. Unvalidated against a real backup; behind the
  iLEAPP fallback.

## [0.5.0] — 2026-07-15

Two more native third-party chat apps via the app-chat framework, each
code-reviewed and hardened. Both unvalidated against a real backup; behind the
iLEAPP fallback.

### Added
- **Viber** (`com.viber/database/Contacts.data`) — messages, conversation
  grouping, per-author group attribution, direction, attachment flag. Uses
  `ZSTATEDATE` (creation) for the timestamp and infers direction robustly
  (including failed sends).
- **Microsoft Teams** (`SkypeSpacesDogfood/*/Skype*.sqlite`) — messages with
  per-author group attribution; HTML content reduced to plain text (recovering
  emoji `alt` text); `ZTHREADTOPIC` group titles.

## [0.4.0] — 2026-07-14

Four more native third-party chat apps via the app-chat framework. All are
unvalidated against a real backup and sit behind the automatic iLEAPP fallback.

### Added
- **Telegram** — a native reader for its binary "postbox" store
  (`postbox/db/db_sqlite`): a bounds-checked byte reader, the `t7` message parse
  (text/author/timestamp/direction), and a minimal `PostboxDecoder` for peer
  names from `t2`. Media payloads aren't decoded.
- **Kik** (`kik.sqlite`) — messages, direction (`ZTYPE`), and group detection via
  the group `ZJID`. Group per-author isn't in this schema, so a group is titled
  but its messages carry no author (as with iLEAPP).
- **imo** (`IMODb2.sqlite`) — messages with correct **per-author group
  attribution** via `ZALIAS`; nanosecond timestamps.
- **Threema** (`ThreemaData.sqlite`) — messages with per-member group attribution
  via `ZSENDER` (named and unnamed groups); system messages excluded.

### Fixed
- Each app was code-reviewed and hardened before release: group chats are no
  longer mislabeled as 1:1 or mis-attributed (Kik/imo/Threema), a new shared
  `col_i64` reads large integer timestamps without f64 precision loss, and
  storage-class-tolerant column reads prevent one odd row from aborting a parse.

### Notes & caveats
- Telegram/Kik/imo/Threema native output is unvalidated against real backups;
  all fall back to iLEAPP on any parse miss.
- iLEAPP remains required for the long tail (Viber, Discord, Slack, Teams, etc.).

## [0.3.0] — 2026-07-14

Native-first, Batch 1: every built-in view now materializes without iLEAPP, and
third-party chats gain native parsers behind a pluggable app-module framework.
iLEAPP still runs for what isn't native yet (Telegram, TikTok's contact graph,
and the long tail), so this is the first batch of the migration, not its end.

### Added
- **Native Calls, Safari & Contacts (no iLEAPP).** Call history
  (`CallHistory.storedata`), Safari history (`History.db`), and Contacts
  (`AddressBook.sqlitedb`, self-extracted) now materialize via native parsers
  through the ManifestIndex, with iLEAPP kept as automatic fallback. Calls and
  Safari also gained sidebar re-import actions. **All first-party views are now
  native.** (Apps was already native from `Info.plist`.)
- **Native third-party chat framework** (`parsers/apps/`). Each app is a small
  module — locate its DB, parse it into a shared message stream — and one shared
  inserter builds the same threads/messages the Messages view renders. Adding an
  app is one module file plus a registry entry.
  - **WhatsApp** (`ChatStorage.sqlite`) and **Facebook Messenger**
    (`lightspeed-userDatabases/*.db`) — native, validated by synthetic fixtures.
  - **Instagram** (`DirectSQLiteDatabase/*.db`) and **TikTok** (`AwemeIM.db`) —
    native but **not yet validated against a real backup**, so they stay behind
    the automatic iLEAPP fallback.
- **NSKeyedArchiver decoder** (`crate::nska`) — resolves Apple keyed-archive
  blobs (used by Instagram DMs); a reusable, standalone iOS-forensics primitive.
- **Living coverage docs** — `docs/app-support.md` (native vs iLEAPP per app) and
  `docs/app-data-coverage.md` (field-level: what each DB holds vs. what we
  surface). Includes research notes on Snapchat / X / Facebook local stores.

### Fixed
- Hardening from a multi-agent code review of the native work: the
  NSKeyedArchiver decoder no longer hangs or panics on a crafted/cyclic archive
  (memoized graph resolution, guarded date conversion); 1:1 Messenger/Instagram
  chats are no longer mislabeled "Group chat"; per-app import counts are folded in
  only after commit; a schema-drifted third-party DB falls back to iLEAPP instead
  of silently dropping messages; several column reads are storage-class-tolerant.

### Notes & caveats
- Instagram & TikTok native output is unvalidated against a real backup; both
  degrade to iLEAPP on any parse miss. TikTok's contact social-graph still comes
  from iLEAPP.
- iLEAPP remains required (Telegram, TikTok contacts, long-tail apps). Making it
  optional is a later batch.

## [0.2.0] — 2026-07-14

The native lazy-decode core, wired into the import — plus password-protected and
pinned Notes, richer Notes browsing, and a reworked re-import UX. iLEAPP still
runs (it supplies Calls, Safari, Apps, and third-party chats); replacing it is
the batched 0.3.0+ migration under "Planned" below.

### Added
- **Native Messages, Notes, Recordings & Camera roll.** The import materializes
  these natively from the backup via a reusable `ManifestIndex` (decrypt-on-
  demand: resolve `domain/relativePath` → file + key, read one file). Messages
  come from `sms.db`; Notes from `NoteStore.sqlite` (body gzip-inflated from
  `ZICNOTEDATA.ZDATA`, text walked out of the `NoteStoreProto` wire format,
  Core Data columns schema-introspected); Recordings from `CloudRecordings.db`
  with `.m4a` streamed over a `traceloupe-audio://` scheme (Range-seekable,
  decrypted at play time). iLEAPP stays the automatic fallback when a source DB
  is absent or a native parse fails.
- **Locked (password-protected) Notes.** Detected via `ZISPASSWORDPROTECTED` /
  `ZENCRYPTEDDATA`; shown with a lock icon and unlocked on demand with the note
  password (PBKDF2 → AES-128-GCM), the plaintext held only in session, never at
  rest.
- **Notes filters & date grouping.** Filter by folder, year, and locked state;
  the list groups into Pinned + recency sections (Today, Yesterday, Previous 7/30
  Days, months, years), matching the Notes app. Parses `ZISPINNED`.
- **Re-import moved to the sidebar.** Per-data-type re-import is now an action on
  each nav item, with a spinner that survives navigation (state lifted above the
  routes); a cancelled re-import no longer destroys the previous import (atomic
  temp-cache swap).
- **Touch ID (opt-in) + signing detection.** An encrypted backup's Keychain
  password can be gated behind Touch ID; the app detects whether it's stably
  signed and enables the toggle accordingly (see `docs/signing.md`).

### Notes & caveats
- Native Messages/Notes/Recordings/Camera-roll run *in addition to* iLEAPP's
  passes, so import time isn't reduced yet — that lands with the 0.3.0
  first-party migration.
- Locked-note AES-GCM decryption and `ZISPINNED` parsing are unit-tested but
  pending validation against a real backup that contains such notes.

## Planned — 0.3.0+ (native-first migration, in batches)
- **Batch 1 (0.3.0) — first-party parity + first native third-party wave.**
  - *Apple apps, no iLEAPP:* native parsers for Calls (`CallHistory.storedata`),
    Safari (`History.db`), Apps (app-state plist), and self-extracted Contacts
    (`AddressBook.sqlitedb`) via the Manifest Index. Every built-in view then
    materializes natively, and the redundant iLEAPP sms/notes passes are dropped
    so import time actually falls.
  - *Third-party, native:* TikTok (moved off iLEAPP), plus Instagram, Facebook,
    Facebook Messenger, X/Twitter, and Snapchat. Snapchat stores little locally
    (ephemeral by design), so its native parser surfaces only what persists.
    WhatsApp and Telegram are deferred to 0.4.0 — they already read via iLEAPP, so
    there's no urgency to convert them first.
- **Batch 2 (0.3.x) — iLEAPP optional.** Default install fully offline (no first-
  import download, no bundled ~222 MB engine); iLEAPP fetched on demand only when
  the user opts into deeper third-party coverage.
- **Batch 3+ (0.4.0+) — remaining native third-party modules** per the app-tier
  roadmap, replacing iLEAPP coverage incrementally — starting with WhatsApp and
  Telegram (deferred from Batch 1, still read via iLEAPP until then). Per-app
  status and the version each gains native support are tracked in
  `docs/app-support.md`.

## [0.1.0] — 2026-07-13

Initial baseline. Open, decrypt, and browse iPhone backups entirely on-device.

### Added
- Discover and open encrypted or unencrypted iPhone backups; first-time import
  via a bundled, checksum-pinned iLEAPP engine, then instant re-open from cache.
- Native, hardware-accelerated backup decryption (keybag → class keys → AES-CBC);
  camera roll read natively with on-demand full-image decryption and cache-once.
- Views: Messages (conversations, cross-conversation timeline, per-year periods),
  Photos (virtualized gallery + full-viewport lightbox with keyboard nav),
  Contacts, Calls, Safari, Notes, and installed Apps.
- Third-party chats surfaced in Messages (TikTok, WhatsApp, Telegram).
- Per-list sorting (field + direction), a 24-hour clock option, resizable and
  icon-rail sidebar, always-visible scrollbars.
- Security: key zeroization, a "forget backup" flow, `backup_id` validation, and
  hardened media serving.
