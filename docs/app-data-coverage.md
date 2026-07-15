# App data coverage

For each source we parse, this is the **living inventory** of what the backed-up
database actually contains and whether TraceLoupe surfaces it. Tick a row (⬜ →
✅) the moment a field shows up in the UI.

Companion to [`app-support.md`](app-support.md) (native vs iLEAPP per app); this
file tracks *field-level* coverage within each source.

**Legend:** ✅ surfaced · ◑ partial · ⬜ present in the backup, not surfaced · —
not present / N/A in this backup.

> **Verified against a real backup (2026-07-15).** The counts below come from
> auditing the decrypted mirror of one real device: **143,088** messages ·
> **95,334** camera-roll assets · **71** contacts · **3,101** calls · **691**
> Safari history items (2,046 visits) · **3,842** notes · **606** voice
> recordings · TikTok the only third-party chat installed. "In backup" reflects
> the observed schema and can vary by iOS version. Two parser defects were found
> and fixed in this pass; one (locked-note decryption) remains — see
> [Known parser defects](#known-parser-defects).

---

## Messages (iMessage / SMS) — `sms.db`

The parser is deliberately minimal — it selects only `text, is_from_me, date,
handle_id, cache_has_attachments` — so the entire rich-interaction layer of
iMessage is unsurfaced.

| Data | In backup | Surfaced | Notes |
|------|:---------:|:--------:|-------|
| Message text (`text`) | ✅ 133k | ✅ | Plaintext only |
| Rich text (`attributedBody`) | ✅ 142k | ⬜ | **Not decoded** — messages whose text lives only here render blank; styling/mentions/inline-link text lost |
| Timestamp (sent) | ✅ | ✅ | `date` |
| Read / delivered receipts | ✅ 100k / 63k | ✅ | `date_read`/`date_delivered` → a "Read <time>" / "Delivered" line under sent bubbles (the `is_read/error` flags remain unused) |
| Edited-message history | ✅ 897 edits | ⬜ | `date_edited` + `message_summary_info` blob (on 138k rows) never decoded |
| Unsent / retracted | — 0 | — | Empty in this backup |
| Direction + sender handle | ✅ | ✅ | `is_from_me`, `handle.id` (contact-resolved) |
| Receiving line (`destination_caller_id`) | ✅ 143k | ⬜ | Which SIM/account received it — dropped |
| Service (iMessage/SMS) | ✅ 140k / 3.4k | ✅ | Per-thread; service filter + brand icon |
| Attachments (image/video/file) | ✅ 8,558 | ✅ | filename, mime, on-demand decrypt/serve |
| Attachment size / dimensions / `transfer_state` / `is_sticker` | ✅ | ⬜ | Not surfaced — can't flag stickers (641) or not-downloaded attachments |
| Thread / conversation | ✅ | ✅ | |
| Group name + participants | ✅ 84/85 | ✅ | `display_name`, `chat_handle_join` |
| Group actions (rename/join/leave) | ✅ 544 | ⬜ | **Dropped** — parser requires text or attachment, so action rows are skipped |
| Tapbacks / reactions | ✅ 7,600 | ⬜ | `associated_message_*`; reaction rows with no text are discarded entirely |
| Replies (inline threads) | ✅ 6,560 | ⬜ | `thread_originator_guid` — shown flat |
| Expressive effects | ✅ 217 | ⬜ | `expressive_send_style_id` |
| App/bubble messages (Apple Cash, polls…) | ✅ 589 | ⬜ | `balloon_bundle_id` / `payload_data` not decoded |
| Filtered (unknown sender) / archived | ✅ 11 | ⬜ | `chat.is_filtered` — no Unknown/Filtered separation |
| Recently-deleted / recoverable | ✅ (tables) | ⬜ | `chat_recoverable_message_join` not read |
| Content kind (media/text/link/sticker) | derived | ✅ | `messages.kind` → content-filter pills |

## Notes — `NoteStore.sqlite`

Everything lives in one Core Data table (`ZICCLOUDSYNCINGOBJECT`); only the
plain-text protobuf body layer is decoded.

| Data | In backup | Surfaced | Notes |
|------|:---------:|:--------:|-------|
| Title | ✅ 3,837 | ✅ | `ZTITLE1` |
| Body text | ✅ | ✅ | gzip-protobuf decoded; **plain text only** — bold/lists/links/attachment runs dropped |
| Snippet | ✅ | ✅ | `ZSNIPPET`, first-line fallback |
| Folder | ✅ 97 | ✅ | incl. "Recently Deleted" |
| Created date | ✅ 3,837 | ◑ | **Fixed this pass** — was mapped to all-NULL `ZCREATIONDATE1`; now COALESCEs to `ZCREATIONDATE3`. Stored in cache; not yet shown in UI |
| Modified date | ✅ | ✅ | Drives all recency grouping/sort/time-filter |
| Pinned | ✅ 349 | ✅ | |
| Locked (flag + withhold body) | ✅ 9 | ✅ | Lock icon, filter, password prompt |
| **Locked-note unlock (decrypt body)** | ✅ | ⬜ | **Broken** — ciphertext read from a nonexistent column; unlock always fails. See [defects](#known-parser-defects) |
| Password hint | ✅ | ✅ | none present on the 9 locked notes here |
| Embedded images / scans / drawings | ✅ 505 notes | ⬜ | `ICAttachment`/`ICMedia` never walked |
| Checklists (structured) | ✅ 46 | ⬜ | Flag + checked-state unread |
| Tables | ✅ 18 notes | ⬜ | Cells not decoded |
| Hashtags / mentions | ✅ 273 | ⬜ | Inline attribute runs |
| Shared / collaboration | ✅ 24 shared / 70 participants | ⬜ | Share state + participants dropped |
| Account / source | ✅ (1 iCloud) | ⬜ | |

## Calls — `CallHistory.storedata`

Parser extracts 6 of ~45 `ZCALLRECORD` columns.

| Data | In backup | Surfaced | Notes |
|------|:---------:|:--------:|-------|
| Address (number / handle) | ✅ 3,096 | ✅ | |
| Timestamp / duration / direction / answered | ✅ | ✅ | `ZDATE`/`ZDURATION`/`ZORIGINATED`; drives "missed" |
| Service (phone/FaceTime) | ✅ | ✅ | coarse (`ZSERVICE_PROVIDER`) |
| FaceTime video vs audio | ✅ (315 audio / 710 video) | ✅ | `ZCALLTYPE` → "FaceTime Video/Audio" label; only video gets the video icon |
| Location | ✅ 2,848/3,101 | ✅ | `ZLOCATION` → shown in the call row subtitle |
| Country code | ✅ 2,082 | ⬜ | `ZISO_COUNTRY_CODE` |
| Read / new-missed flag | ✅ | ⬜ | `ZREAD` — no unseen-missed badge |
| Withheld / unavailable number | ✅ 5 | ⬜ | `ZNUMBER_AVAILABILITY` |
| Disconnect cause / filtered reason | ✅ | ⬜ | declined/blocked/junk not distinguished |
| Contact name on call (`ZNAME`) | — 0 | — | Empty here; UI resolves via Contacts |
| Unique id (`ZUNIQUE_ID`) | ✅ | ⬜ | No dedupe key — re-import clears + reinserts |
| Group-call / participant UUIDs | ✅ | ⬜ | FaceTime-group linkage unused |

> UI caveat: the "Name" sort actually sorts by raw `address`, not the resolved
> contact name shown in the row.

## Safari — `History.db` / `Bookmarks.db` / `SafariTabs.db`

| Data | In backup | Surfaced | Notes |
|------|:---------:|:--------:|-------|
| History URL / per-visit title / visit time | ✅ 691 items / 2,046 visits | ✅ | searchable, time-filtered, sortable |
| Total visit count | ✅ | ✅ | sort-by-visits |
| Redirect chains | ✅ 92 links | ⬜ | `redirect_source/destination` — navigation graph not reconstructed |
| Deleted-history tombstones | ✅ 2,477 | ⬜ | `history_tombstones` never read — sizeable forensic signal |
| Load status / HTTP method / origin / score | ✅ | ⬜ | Not parsed |
| Daily/weekly visit-count blobs | ✅ | ⬜ | Visit-time histogram unused |
| Bookmarks (title/url) | ✅ 8 | ✅ | + open external |
| Bookmark folder hierarchy | ✅ | ◑ | Only immediate parent shown; no tree/breadcrumb |
| Reading list (title/url/added/preview) | ✅ 1 | ✅ | |
| Reading-list last-viewed | ✅ | ⬜ | Parsed to cache (`date_viewed`) but never rendered |
| Reading-list unread/fetched flags | ✅ | ⬜ | No read/unread indicator |
| Open tabs (title/url) | ✅ 41 | ✅ | + tab group name |
| Tab windows / active-tab / private vs local | ✅ | ⬜ | `windows*` tables unread — flat list, no open-vs-recently-closed split |

## Contacts — `AddressBook.sqlitedb`

Parser reads only First/Last/Organization + phone/email multivalues (hardcoded to
properties 3 & 4).

| Data | In backup | Surfaced | Notes |
|------|:---------:|:--------:|-------|
| First / last name | ✅ | ✅ | drives sort |
| Organization | — 0 here | ✅ (capable) | parsed + shown; none populated |
| Middle name / nickname | — 0 here | ✅ | parsed + shown in detail; none populated in this backup |
| Prefix / suffix / phonetic | — 0 here | ⬜ | not parsed |
| Job title / department | — 0 here | ✅ | parsed + shown ("Work" section); none populated here |
| Phone numbers (+ labels) | ✅ 77 | ✅ | tel: links; also feeds message matching |
| Emails (+ labels) | ✅ 11 | ✅ | mailto: |
| Postal addresses | ✅ 6 | ✅ | `ABMultiValueEntry` (Street/City/State/ZIP/Country) → one-line address, shown with its label |
| Social / IM handles | ✅ 1 | ⬜ | property 46 |
| Related names (relationship graph) | ✅ 24 | ⬜ | Mother/Father/custom — fully dropped |
| Birthday | ✅ 11 | ✅ | `Birthday` Core Data timestamp → shown in detail |
| Contact note | ✅ 22 | ✅ | shown in the detail "Note" section |
| Groups + membership | ✅ 3 / 40 | ⬜ | `ABGroup`/`ABGroupMembers` untouched |
| Photo | ✅ 54 | ✅ | thumbnail w/ full-size fallback |
| Memoji / avatar recipe | ✅ | ⬜ | |
| Creation / modification dates | ✅ 71/71 | ⬜ | present on all; unused |
| Identity (`guid`/`ExternalUUID`, `PersonLink`, account) | ✅ | ⬜ | linked/unified contacts not modeled — may show as duplicates |

## Voice recordings — `CloudRecordings.db`

| Data | In backup | Surfaced | Notes |
|------|:---------:|:--------:|-------|
| Title | ✅ 606 | ✅ | **Fixed this pass** — now reads `ZENCRYPTEDTITLE` (plaintext locally, all 606 rows) instead of falling back to the timestamp label for the ~330 memos without a composition manifest |
| Composition-manifest title | ✅ 276 | ✅ | `.composition/manifest.plist` `RCSavedRecordingTitle` (preferred when present) |
| Recorded-at date / duration | ✅ | ✅ | |
| Audio playback (`.m4a`) | ✅ | ✅ | Range-seekable, decrypt-on-demand |
| Folder | ✅ 245/606, 8 folders | ✅ | `ZFOLDER.ZENCRYPTEDNAME` joined via the recording's `ZFOLDER` FK; shown in the row subtitle + detail |
| Recently-deleted (`ZEVICTIONDATE`) | ✅ (0 set) | ⬜ | A trashed memo would show as normal |
| Playback position / studio-mix flags | ✅ (0 set) | ⬜ | minor |
| Transcript / favorite / geo | — | — | Not present in this DB (source limitation, not a parser gap) |

## Camera roll — `Photos.sqlite`

`camera_roll.rs` enumerates DCIM files + capture date; `photos_meta.rs` enriches
people/GPS/favorite/moment/albums onto `media_items`.

| Data | In backup | Surfaced | Notes |
|------|:---------:|:--------:|-------|
| Photo / video file + thumbnail | ✅ 88k / 7.1k | ✅ | full-res + thumb, decrypt-on-demand |
| Capture date | ✅ | ✅ | primary sort + time filter |
| Added / modified dates | ✅ | ⬜ | `ZADDEDDATE`/`ZMODIFICATIONDATE` unread |
| GPS lat/long | ✅ 24k | ✅ | lightbox Maps link (no map/grid pin) |
| Reverse-geocoded place | ◑ | ◑ | moment/event title only; per-asset reverse-geocode blob ignored |
| **EXIF** (camera, lens, ISO, exposure, focal length) | ✅ 22–25k | ✅ | `ZEXTENDEDATTRIBUTES` → camera + lens + "ISO · ƒ · shutter · mm" in the lightbox |
| Dimensions / file size | ✅ 95k | ✅ | `ZWIDTH`/`ZHEIGHT` + `ZORIGINALFILESIZE`; shown in the lightbox |
| Orientation | ⬜ 95k | ⬜ | `ZORIENTATION` unread |
| Albums — user | ✅ 482 | ✅ | lightbox chip + search |
| Albums — smart/system | ⬜ 235 | ⬜ | excluded (`ZKIND=2` only) |
| Favorite | ✅ 17k | ✅ | heart badge + search |
| Hidden | ✅ 46k | ✅ | `ZHIDDEN` → an eye-off badge on the grid tile + lightbox (shown, not excluded — forensic) |
| Recently-deleted / trashed | ✅ 48 | ◑ | excluded from grid; not shown as a category |
| Faces / people (named) | ✅ 69 named / 72k faces | ✅ | badge + lightbox + search (named only) |
| Live Photo / burst | 381 / 53 | ⬜ | not paired/grouped |
| Subtype (screenshot ~62k, HDR ~18k, portrait, pano, slo-mo) | ⬜ | ⬜ | no badge/filter |
| Video duration | ✅ 7.1k | ✅ | `ZDURATION` → `media_items.duration_s` |
| Description | ⬜ 1,253 | ⬜ | `ZASSETDESCRIPTION` unread |
| Edited-vs-original / import session / cloud state | ⬜ 17k edited | ⬜ | provenance + edit state unread |

---

## Third-party app chats

Native modules under `parsers/apps/` feed a shared pipeline. **In this backup only
TikTok is installed**; the other ten rows reflect the parser's SELECT + the
iLEAPP-derived schema in code comments (unvalidated).

**The headline cross-cutting gap:** `insert_app_conversation` writes only the
`messages.has_attachments` flag and **never inserts into the `attachments`
table** — attachment rows come only from the iMessage/SMS path. So no app-chat
media (path or bytes) ever reaches the cache or the Messages view, even when the
parser knows the local file (WhatsApp `ZMEDIALOCALPATH`, Kik `ZKIKATTACHMENT`,
Threema `ZFILENAME`, TikTok `TIMFileORM.localRelativePath`/`remoteURL`).

| App | DB present here? | Surfaced | Notable available-but-unsurfaced | Media |
|-----|:---:|------|------|:---:|
| WhatsApp | — | text, ts, direction, chat name, has-attachment | media path; **group per-msg author** (inbound mis-attributed to partner); calls | ⬜ |
| Messenger | — | text, ts, direction, sender, chat key | media, reactions | ⬜ |
| Instagram | — | text, ts, direction, sender | all media (`has_attachment` hardcoded false), reactions | — |
| **TikTok** | ✅ | text, ts, direction, sender name/handle, **kind** (text/shared/sticker/system) | `TIMFileORM` (317 rows: local path + remoteURL + mime + md5); group members/name; read receipts | ⬜ (typed marker only) |
| Telegram | — | text, ts, direction, chat/author name, has-attachment | media, reactions, reply/forward | ⬜ |
| Kik | — | text, ts, direction, chat name, has-attachment | media; group sender left blank | ⬜ |
| imo | — | text, ts, direction, sender (per-author) | `ZIMDATA` payload, phone | ⬜ |
| Threema | — | text/caption, ts, direction, sender (per-author) | media files incl. `ZFILENAME` | ⬜ |
| Viber | — | text, ts, direction, sender (per-author) | attachments, calls, location, time-bomb | ⬜ |
| Teams | — | text (HTML→plain), ts, direction, sender (per-author) | attachments (`has_attachment` hardcoded false) | — |
| LinkedIn | — | text, ts, direction, chat/sender name | attachments, InMail/reaction metadata | — |

Other cross-cutting gaps: **reactions/starred/edited** aren't modeled anywhere
(no cache column exists); **group per-message sender** is missing for WhatsApp &
Kik; **call history** from WhatsApp/Viber/Teams is unparsed (the `calls` table is
iMessage/phone-only); **contacts/social graph** is imported only for TikTok
(`source='TikTok'`). Only WhatsApp and TikTok are marked validated in code.

---

## Untouched data (present in backup, not surfaced)

TraceLoupe surfaces Messages, Photos, Contacts, Calls, Safari, Notes, Recordings,
third-party chats, and an installed-apps list. Everything below exists in this
backup but has no parser. Ranked by value × feasibility.

| Domain | Present here? | Rough scale | Value | Notes |
|--------|:---:|-------|:---:|-------|
| **Health** | ✅ real | `healthdb_secure.sqlite` 87 MB; 344,063 quantity samples, 13 workouts | ★★★ | steps/HR/sleep/workouts + GPS routes (`location_series`). Richest untouched store |
| **CoreDuet interactions** | ✅ real | `interactionC.db` 15,055 interactions, 66 contacts | ★★★ | cross-app who-talked-to-whom graph w/ direction + timestamps |
| **Device / backup metadata** | ✅ parsed, not shown | name, model, iOS version, serial, last-backup, encryption | ★★★ | `read_backup_info` already extracts it — only flashed in the import banner, then discarded. A "Device Info" view is nearly free |
| **Calendar** | ✅ real | `Calendar.sqlitedb` 217 items, 15 calendars | ★★ | events/invitees/recurrence; plain SQLite |
| **Reminders** | ✅ real | 124 reminders | ★★ | to-dos + due dates |
| **Apps view metadata** | ✅ available | Info.plist `Applications` → version, install/purchase date, seller | ★★ | cache stores only `bundle_id`; no version/date/size/name shown |
| **Keychain** | ✅ (sensitive) | passwords / Wi-Fi / certs | ★★ | surface presence + counts only, never values |
| **Instagram time-in-app** | ✅ partial | 7 usage DBs | ★ | usage telemetry only, no messages |
| **Maps** | store empty | favorites/history/visits all 0 | ★ | ideal schema, nothing synced locally here |
| **Podcasts / Journal / Wallet** | empty/absent | 0 rows | ★ | apps present but unused in this backup |
| **Freeform** | present, corrupt | `boards.db` "disk image malformed" | ★ | WAL-only in backup; not usable as-is |
| **Mail** | no store here | only prefs + Gmail autocomplete contacts | ★ | no Envelope Index in this backup |
| **Screen Time (knowledgeC)** | absent | — | — | not in this backup (only CoreDuet) |

**Highest-value additions:** Health, the CoreDuet interaction graph, and
Calendar/Reminders — all hold substantial real data, are plain SQLite with
documented schemas, and need no new decryption path. A **Device Info** view is
almost free (the data is already extracted and thrown away). Keychain should be
presence-and-counts only. Maps/Podcasts/Journal/Wallet are worth a parser for the
schema but render empty on this particular device.

---

## Known parser defects

Found while auditing against the real backup (2026-07-15):

1. **Notes — locked-note decryption is broken** ⬜ *(open)*. The parser reads the
   note ciphertext from a nonexistent `ZENCRYPTEDDATA` column (real ciphertext is
   `ZICNOTEDATA.ZDATA`) and takes the AES-GCM IV/tag from the object row instead
   of `ZICNOTEDATA`; it also ignores `ZCRYPTOWRAPPEDKEY`, so the decrypt ladder is
   missing the AES-key-unwrap step. Net: all locked notes are un-decryptable and
   `unlockNote` always fails. Fixing it needs corrected columns + a wrapped-key
   unwrap + a cache-schema change, then validation with a real note password.
2. **Notes — creation date lost** ✅ *(fixed this pass)*. `col_or_null` picked the
   first *existing* date column; on a modern NoteStore `ZCREATIONDATE1` exists but
   is all-NULL while the value is in `ZCREATIONDATE3`. Now COALESCEs across all
   that exist so a populated sibling wins.
3. **Recordings — real titles ignored** ✅ *(fixed this pass)*. The parser used the
   timestamp-only `ZCUSTOMLABEL`, so the ~330 memos without a `.composition`
   manifest showed as bare timestamps. Now prefers `ZENCRYPTEDTITLE` (plaintext
   locally, populated on all 606 rows).
4. **Calls — "Name" sort** sorts by raw `address`, not the resolved contact name
   shown in each row (cosmetic ordering mismatch).
