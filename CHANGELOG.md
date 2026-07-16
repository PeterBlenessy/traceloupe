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
| `0.7.0` | **Native-first complete** — native TikTok contacts (the last iLEAPP-only artifact); default import fully native (~35s, no engine subprocess); `Photos.sqlite` metadata (people/date/GPS/favorite); native Safari bookmarks, reading list & tabs. |
| `0.8.0` | **Native TikTok DMs + UI overhaul** — TikTok direct messages (validated) with a message content-kind filter, friendlier voice-memo titles, and message media in the gallery; a shared `PanelHeader`, badge filters, a UI density setting, and persisted per-view state. |
| `0.9.0` | **Data-coverage pass** — a field-level audit against a real backup, then filling the high-value gaps: Calls FaceTime/location, Photos EXIF/hidden/subtypes, Contacts detail (birthday/note/addresses), Messages receipts/reactions/replies, Safari deleted-history. |
| `0.10.0` | **Untapped stores surfaced** — five new views (Device, Calendar, Reminders, Health, Interactions), Messages `attributedBody` decode + edited flag, Notes rich-content indicators, and the app-chat attachment-media framework. |
| `0.11.0` | **Locked-note decryption** — password-protected Apple Notes unlock on demand (PBKDF2 → RFC-3394 key-unwrap → AES-128-GCM), the note password entered in-app and nothing decrypted at rest. Closes the last coverage-audit gap. |
| `0.12.0`+ | **Native-first, continued** — remaining apps (need heavier machinery), then make iLEAPP an optional on-demand engine. See "Planned" below. |

> The single source of truth for the version is `package.json`; keep the
> workspace `Cargo.toml` and `src-tauri/tauri.conf.json` in step when it changes.

## [Unreleased]

_Nothing yet._

## [0.11.3] — 2026-07-16

A broad frontend + UI/UX review. Fixes real interaction bugs and tightens
consistency across the newer views.

### Fixed

- **Calendar / Reminders / Interactions were unscrollable and un-virtualized** —
  they wrapped the virtual list in a plain block, so it had no bounded height:
  the list couldn't scroll and every row mounted at once (rows past the fold were
  clipped and unreachable). They now use the shared `VirtualListView`, which also
  gives them a loading skeleton, an error state, and the same row width as every
  other list.
- **Health** gains loading and error states.
- **Device** shows an error state instead of a blank panel when its query fails.
- **Re-import didn't refresh some counts** — the Messages Timeline total (a
  query-key typo) and the Photos time-chip counts stayed stale after a re-import;
  both now invalidate correctly.
- **Contacts weren't requested before a backup was open** — the shared contact
  resolver now gates on an active backup.

### Changed

- Calendar/Reminders list-name pills use the shadcn `Badge`; the Reminders header
  count is now the total (matching every other view); Photos grid thumbnails have
  an accessible label and a keyboard focus ring.

## [0.11.2] — 2026-07-16

A broad whole-crate review of `traceloupe-core`. The security-critical surface —
keybag/AES decryption, the Manifest path guards, and all dynamic SQL — verified
clean (no reachable panics from adversarial keybag/plist/typedstream/postbox
bytes, no SQL injection, no path traversal). This releases the data-integrity
hardening it surfaced.

### Fixed

- **Timestamp overflow across 13 parsers** — converting a Core Data date did
  `d as i64 + MAC_EPOCH`, which saturates the float→int cast and then overflows
  the integer add on a corrupt/absurd date (~1e19): a panic in debug builds, a
  wrapped-negative time in release. Now the epoch is added in floating point
  before the cast, so it saturates cleanly. (safari, calls, address book, photos,
  reminders, health, interactions, calendar, and the WhatsApp/Viber/Kik/Threema
  chat parsers.)
- **Safari bookmarks: one bad row no longer wipes the whole import** — a NULL
  `type` or `id` was read strictly and aborted the entire bookmarks/reading-list/
  tabs load; such rows are now tolerated (NULL type → not a folder) or skipped.
- **WhatsApp / Facebook Messenger: a mistyped cell no longer drops all messages**
  — message body/timestamp now go through the same tolerant column readers the
  other app-chat parsers use.
- **Recordings re-import keeps the folder name** — a recordings-only re-import
  hardcoded the Voice-Memos folder to NULL; it now matches the full import.

## [0.11.1] — 2026-07-16

A code-review pass over the 0.9.0→0.11.0 work. The reviewed surface (Notes
decryption/crypto ladder, the five new parsers/views, the import/IPC/frontend
wiring) verified correct; this releases the handful of real fixes it found.

### Fixed

- **Messages import no longer aborts on a NULL-dated row** — `message.date` was
  read as a required integer, so one NULL date (the column is `INTEGER DEFAULT 0`,
  not `NOT NULL`) would fail the entire Messages parse. Now read optionally.
- **Attachment-only messages no longer dropped on a stale flag** — a message with
  no text was skipped whenever the denormalized `cache_has_attachments` flag was
  stale (0 despite real `message_attachment_join` rows). Selection and the
  has-attachment flag now consult the actual join table.
- **Health workouts pick their activity deterministically** — a multi-activity
  (multi-sport / all-NULL-primary) workout previously showed an arbitrary
  activity's type/duration; now it deterministically prefers the explicit primary,
  else the longest, and aggregates sample dates for the true span.
- **Locked notes are unlockable even without an iteration count** — `note_crypto`
  no longer requires `ZCRYPTOITERATIONCOUNT` (decryption already defaults 0/absent
  to 20000), so a schema that omits it still gets a password prompt.
- Hardening: `aes_ecb_decrypt_block` is panic-safe in isolation; corrected stale
  doc/comments (Notes ciphertext column, import step count).

## [0.11.0] — 2026-07-16

Closes the last gap from the 0.9.0/0.10.0 coverage audit: **password-protected
(locked) Apple Notes can now be unlocked**. The note password is entered in the
app and never leaves it; nothing is decrypted at rest — only the crypto
parameters are cached, and the plaintext is derived on demand and discarded.

### Added

- **Locked-note decryption** — unlocking a protected note runs Apple's crypto
  ladder: `PBKDF2-HMAC-SHA256(password, salt)` → AES key-unwrap (RFC 3394) of the
  per-note key → `AES-128-GCM` over the note body (IV/tag/ciphertext from
  `ZICNOTEDATA`) → gunzip → protobuf → text. Salt/iterations/wrapped-key are read
  from the note object, matching Apple's real table layout.

### Fixed

- **Locked-note decryption was broken** — the parser read the ciphertext from a
  nonexistent `ZENCRYPTEDDATA` column, took the GCM IV/tag from the wrong table,
  and ignored `ZCRYPTOWRAPPEDKEY` (skipping the key-unwrap step), so `unlockNote`
  always failed. All three are corrected. The decryptor is also resilient to an
  anomalous on-device variant (iteration count `0` → 20000 default; a 16-byte
  wrapped key) by trying multiple key candidates and letting the GCM tag select
  the right one.

### Internal

- Cache schema **v23 → v24** (adds `notes.crypto_wrapped_key`).

## [0.10.0] — 2026-07-16

Follows the 0.9.0 coverage audit by **surfacing the untapped stores** it flagged —
five new views plus deeper decoding of Messages and Notes. See
[`docs/app-data-coverage.md`](docs/app-data-coverage.md) for the field-level
inventory.

### Added

- **Device view** — the active backup's device/backup metadata (name, model
  mapped to a marketing name, iOS version, serial, last-backup date, encryption).
- **Calendar view** — events from `Calendar.sqlitedb` (title · when · location ·
  notes · calendar).
- **Reminders view** — from the reminders store (title/notes · completion · flag ·
  list · due date).
- **Health view** — a workout log (activity, date, duration, distance) plus a
  sample-count + date-range summary, without materializing the raw samples.
- **Interactions view** — CoreDuet's pre-aggregated cross-app communication graph:
  who you've talked to, incoming/outgoing counts, and the span, most-contacted
  first.
- **Messages: `attributedBody` decoded** — recovers the body of modern text-less
  messages (streamtyped NSString extractor, validated 3000/3000 against the `text`
  column), and flags **edited** messages (`date_edited`) with an "Edited" tag.
- **Notes: rich-content indicators** — checklist badge (`ZHASCHECKLIST`) and
  per-note embedded image / attachment counts.
- **App-chat attachment media framework** — the shared inserter now resolves an
  `AppMessage`'s attachments to backup files (`attachments` table + gallery
  mirror), closing the audit's cross-cutting gap. Per-app emission lands when a
  backup with app media is available to validate against.

### Notes

- **Locked-note decryption** remains unfixed and is a **known defect** — iLEAPP
  doesn't decode encrypted notes and the on-disk crypto is ambiguous, so a correct
  fix needs a validated reference/known-answer vector.

## [0.9.0] — 2026-07-15

A **data-coverage pass**: a field-level audit of the real backup (parser →
cache → query → UI) followed by filling every high-value, tractable gap it
found. Each item below is verified end-to-end. See
[`docs/app-data-coverage.md`](docs/app-data-coverage.md) for the full inventory
and the remaining (large-feature / password-blocked) gaps.

### Added

- **Calls: FaceTime audio vs video + call location.** `ZCALLTYPE` distinguishes
  FaceTime Audio from Video (only video gets the video icon); `ZLOCATION`
  (carrier/geo) shows in the call row.
- **Photos: EXIF, dimensions, file size, video duration.** Camera make/model,
  lens, and a compact "ISO · ƒ · shutter · mm" exposure summary in the lightbox,
  plus pixel dimensions, original file size, and video length.
- **Photos: hidden-album flag** — hidden assets are badged (eye-off), shown and
  flagged rather than silently mixed in (forensic stance).
- **Photos: screenshot / panorama subtype badges.**
- **Contacts detail** — birthday, note, job title, department, nickname, middle
  name, and structured postal addresses.
- **Voice Memos folder** — recordings show their containing folder.
- **Messages: read/delivered receipts** ("Read <time>" / "Delivered" under sent
  bubbles), **tapbacks/reactions** (add/remove folded into a "❤️×2 👍" badge,
  incl. custom emoji), and **inline replies** (a quoted preview above the reply).
- **Safari: deleted-history tombstones** — cleared URLs surface in the History
  list flagged deleted (trash icon + strikethrough).

### Fixed

- **Voice-memo titles** — read `ZENCRYPTEDTITLE` (plaintext locally, on every
  row) so all memos show their real name, not just the ~276 with a composition
  manifest.
- **Notes creation dates** — COALESCE the suffixed Core Data date columns so a
  present-but-NULL `ZCREATIONDATE1` no longer shadows the populated
  `ZCREATIONDATE3` (was NULL on every note).
- **`safari_bookmarks.rs`** items-after-test-module and a `manual_is_multiple_of`
  lint (pre-existing, blocked `clippy -D warnings`).

### Known

- **Locked-note decryption is broken** and unfixed: the ciphertext is read from a
  nonexistent column and the AES-key-unwrap step is missing. A correct fix needs
  validation with a real note password.

## [0.8.0] — 2026-07-15

### Added

- **Native TikTok DM messages**, validated on a real backup (263k messages). Parsed
  from `ChatFiles/<uid>/db.sqlite` (`TIMMessageORM`) — a *separate* DB from the
  `AwemeIM.db` social graph — with sender names resolved from `AwemeContacts*`. The
  `-wal` sidecar is extracted alongside each DB so unflushed rows are replayed.
- **Typed markers for non-text TikTok messages.** Shared videos, stickers, nudges
  and profile cards (whose payloads live only on TikTok's servers) surface as
  labelled markers instead of blank bubbles, and each carries a content `kind`.
- **Message content-kind filter** in the open conversation — clickable badges
  (text / link / media / shared / sticker / system) showing only the kinds actually
  present. Threaded through SQL, the Tauri commands and the cache (schema v11 adds
  `messages.kind`).
- **Friendlier voice-memo titles** — read from each recording's
  `.composition/manifest.plist` (`RCSavedRecordingTitle`), falling back to the DB
  label then the filename, instead of the cryptic folder name.
- **Message image/video attachments now appear in the Photos gallery** (mirrored
  into `media_items` with source `Messages`).
- **UI density setting** (Comfortable / Cozy / Compact) — "True Density": fonts and
  icons keep their size, only spacing tightens. A rows-icon toggle in the top bar
  cycles the levels; list rows, the Timeline and chat bubbles all respond.
- **Time-range + search filters** extended to Contacts, Calls and Recordings, matching
  Photos / Safari / Notes.

### Changed

- **Shared `PanelHeader` header** across every list view (title · count · filter
  badges / search / toolbar). Master-detail views (Contacts, Recordings, Notes,
  Messages) now put the full-width header across the top with the list+detail split
  below it, instead of a header trapped in the narrow master column.
- **All filter chips are now `BadgeFilter` badges** and **never wrap** — they scroll
  horizontally when the window is narrow, so filters can't push the header taller.
  The time-range period chips got the same no-wrap treatment.
- **Import progress** now separates the *Indexing* phase from import, restarts at 0%,
  shows a right-aligned `step n/N`, and uses Title-Case entity labels.
- **Appearance toggle** in the top bar is a single button that cycles
  System → Light → Dark (lucide `sun-moon` for system); also surfaced in Settings.
- **Settings dialog** redesigned to a fixed-size, macOS-System-Settings-style layout
  with a vertical tab rail.
- Selection, active filter and sort order now **persist across navigation and restarts**
  for every view (`usePersistedState`).
- Removed the redundant single-field "Time" sort picker in Messages (a direction
  toggle replaces it).

### Fixed

- **Stale persisted filters can no longer strand a view empty.** Photos' source and
  Notes' folder/lock filters are clamped to what the *current* backup actually has,
  so a choice carried over from another backup falls back to "all" instead of leaving
  an unrecoverable empty grid.
- A `?service=` deep-link into Messages now applies **once** per value instead of
  snapping the filter back on every refetch.
- Recordings show a distinct "no matches" message when a search/time filter excludes
  every recording (vs. "no recordings in this backup").
- TikTok message parsing reads `content`/`chat_key` tolerantly, so a single odd row
  (BLOB content, numeric group id) no longer aborts the whole account.

## [0.7.0] — 2026-07-15

### Fixed

- **Opening an encrypted backup no longer needs a second "Open" click.** After the
  password step the backup is now marked active optimistically, so the target
  view no longer reads a stale "no backup open" state and bounce back to the
  picker (queries use `staleTime: Infinity`).
- **Photos source filter no longer breaks on a narrow window** — the pills scroll
  horizontally within the title row instead of wrapping out of it. The long
  "iTunes Backup - Installed Applications" source is shortened to "iTunes Backup"
  (and its numbered variants collapse into one).
- Filter/header **item counts are now smaller and dimmer** across all views, so
  they read as secondary to the labels they annotate.

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

- **Native TikTok contacts / social graph** (`AwemeIM.db`), the last artifact
  that had needed iLEAPP. A default import is now **fully native** — it launches
  no iLEAPP subprocess and doesn't require the engine installed, cutting a full
  import from minutes to ~35s. iLEAPP is kept only as a development reference for
  schemas we can't inspect in our own backup; the engine code path stays dormant.
- **Photo metadata from `Photos.sqlite`** — a native parser enriches each
  camera-roll photo with the **people** in it (face recognition), a precise
  **capture date**, **GPS location**, and its **favorite** flag. Photo search
  matches person names; the lightbox shows who's in a photo, its coordinates, and
  a favorite heart; tagged/favorited thumbnails carry small badges.
- **Search in Notes** — a full-width search row over title / snippet / folder,
  alongside the folder, lock, time, and sort filters.
- **Search rows for Photos, Messages, and Safari.** Photos gets a full-width
  filename search; the Messages timeline gets a full-width search over body /
  sender / conversation; Safari's search moved to its own full-width row. All
  compose with the existing time filter and sort. (Photos person/face tags aren't
  parsed yet — a future `Photos.sqlite` parser — so photo search is filename-only
  for now.)
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

## [0.6.3] — 2026-07-15

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
