# App data coverage

For each app/source we parse, this is the **living inventory** of what its
backed-up database actually contains, and whether TraceLoupe surfaces it. Tick a
row (⬜ → ✅) the moment the field shows up in the UI. New apps get a section here
when their parser lands.

Companion to [`app-support.md`](app-support.md) (which tracks *native* vs iLEAPP
per app); this file tracks *field-level* coverage within each app.

**Legend:** ✅ surfaced · ⬜ present in the backup, not surfaced yet · — not in the
backup. "In backup" reflects the known schema and can vary by iOS version.

---

## Messages (iMessage / SMS) — `sms.db`

| Data | In backup | Surfaced | Notes |
|------|:---------:|:--------:|-------|
| Message text | ✅ | ✅ | |
| Timestamp (sent/received) | ✅ | ✅ | |
| Direction (is-from-me) + sender handle | ✅ | ✅ | |
| Attachments (image/video/file) | ✅ | ✅ | decrypt-on-demand |
| Thread / conversation | ✅ | ✅ | |
| Group name + participants | ✅ | ✅ | via `chat.db` schema |
| Service (iMessage/SMS) | ✅ | ✅ | |
| Read / delivered receipts | ✅ | ⬜ | `date_read`, `date_delivered` |
| Tapbacks / reactions | ✅ | ⬜ | `associated_message_*` |
| Edited / unsent message history | ✅ | ⬜ | iOS 16+ `message_summary_info` |
| Replies (inline threads) | ✅ | ⬜ | `thread_originator_guid` |
| Message effects / expressive send | ✅ | ⬜ | `expressive_send_style_id` |

## Notes — `NoteStore.sqlite`

| Data | In backup | Surfaced | Notes |
|------|:---------:|:--------:|-------|
| Title | ✅ | ✅ | |
| Body text | ✅ | ✅ | gzip-protobuf decoded |
| Folder | ✅ | ✅ | incl. "Recently Deleted" |
| Created / modified dates | ✅ | ✅ | |
| Pinned | ✅ | ✅ | |
| Locked + unlock | ✅ | ✅ | on-demand decrypt |
| Password hint | ✅ | ✅ | |
| Embedded images / scans / drawings | ✅ | ⬜ | `ZICATTACHMENT` / media |
| Checklists (structured) | ✅ | ⬜ | rendered as text only now |
| Tables | ✅ | ⬜ | |
| Hashtags / tags | ✅ | ⬜ | inline attribute runs |
| Shared / collaboration info | ✅ | ⬜ | |

## Calls — `CallHistory.storedata`

| Data | In backup | Surfaced | Notes |
|------|:---------:|:--------:|-------|
| Address (number / handle) | ✅ | ✅ | |
| Timestamp | ✅ | ✅ | |
| Duration | ✅ | ✅ | |
| Direction (in/out) | ✅ | ✅ | `ZORIGINATED` |
| Answered | ✅ | ✅ | |
| Service (phone/FaceTime) | ✅ | ✅ | coarse |
| FaceTime video vs audio | ✅ | ⬜ | `ZCALLTYPE` |
| Contact name (stored on call) | ✅ | ⬜ | `ZNAME`; we resolve via Contacts instead |
| Location / country code | ✅ | ⬜ | `ZLOCATION`, `ZISO_COUNTRY_CODE` |
| Blocked / read flags | ✅ | ⬜ | |

## Safari history — `History.db`

| Data | In backup | Surfaced | Notes |
|------|:---------:|:--------:|-------|
| URL | ✅ | ✅ | |
| Page title | ✅ | ✅ | per visit |
| Visit time | ✅ | ✅ | |
| Total visit count | ✅ | ✅ | |
| Redirect chains | ✅ | ⬜ | `history_visits.redirect_*` |
| Bookmarks | ✅ | ⬜ | separate `Bookmarks.db` |
| Open tabs | ✅ | ⬜ | separate `BrowserState.db` |
| Frequently visited / load status | ✅ | ⬜ | |

## Contacts — `AddressBook.sqlitedb`

| Data | In backup | Surfaced | Notes |
|------|:---------:|:--------:|-------|
| First / last name | ✅ | ✅ | |
| Organization | ✅ | ✅ | |
| Phone numbers (+ labels) | ✅ | ✅ | |
| Emails (+ labels) | ✅ | ✅ | |
| Photo | ✅ | ✅ | `AddressBookImages.sqlitedb` |
| Middle / prefix / suffix / nickname | ✅ | ⬜ | |
| Postal addresses | ✅ | ⬜ | `ABMultiValue` property 5 |
| Birthday / dates | ✅ | ⬜ | |
| Contact note | ✅ | ⬜ | |
| URLs / social / IM handles | ✅ | ⬜ | |
| Related names, groups | ✅ | ⬜ | |

## Voice recordings — `CloudRecordings.db`

| Data | In backup | Surfaced | Notes |
|------|:---------:|:--------:|-------|
| Title | ✅ | ✅ | filename fallback |
| Recorded-at date | ✅ | ✅ | |
| Duration | ✅ | ✅ | |
| Audio playback (`.m4a`) | ✅ | ✅ | Range-seekable, decrypt-on-demand |
| Folder | ✅ | ⬜ | |
| Transcript | ✅ | ⬜ | iOS 18+, if present |

## Camera roll — `Photos.sqlite`

| Data | In backup | Surfaced | Notes |
|------|:---------:|:--------:|-------|
| Photo / video file | ✅ | ✅ | full-res decrypt-on-demand |
| Thumbnail | ✅ | ✅ | |
| Taken-at date | ✅ | ✅ | |
| Kind (photo/video) | ✅ | ✅ | |
| GPS / location | ✅ | ⬜ | |
| EXIF (camera, dimensions, etc.) | ✅ | ⬜ | |
| Albums | ✅ | ⬜ | |
| Favorite / hidden / recently-deleted | ✅ | ⬜ | |
| Faces / people | ✅ | ⬜ | |
| Live Photo pairing, bursts | ✅ | ⬜ | |

---

## Third-party apps

Each app is a native module under `parsers/apps/` (see `app-support.md`). Sections
below are filled in as each parser lands; apps still on iLEAPP show as message
threads only until then.

### WhatsApp — `ChatStorage.sqlite` (native)

Schema facts from iLEAPP `whatsApp.py`; provenance reference (§10).

| Data | In backup | Surfaced | Notes |
|------|:---------:|:--------:|-------|
| Message text | ✅ | ✅ | `ZWAMESSAGE.ZTEXT` |
| Timestamp | ✅ | ✅ | `ZMESSAGEDATE` (Core Data time) |
| Direction (from-me) | ✅ | ✅ | `ZISFROMME` |
| Conversation grouping | ✅ | ✅ | `ZWACHATSESSION.ZCONTACTJID` |
| Chat / contact name | ✅ | ✅ | `ZPARTNERNAME` |
| Has attachment (flag) | ✅ | ✅ | `ZWAMEDIAITEM.ZMEDIALOCALPATH` |
| Attachment media (view/play) | ✅ | ⬜ | media path known; not yet served |
| Group sender (per-message) | ✅ | ⬜ | `ZWAGROUPMEMBER` join |
| Starred messages | ✅ | ⬜ | `ZSTARRED` |
| Location messages (lat/long) | ✅ | ⬜ | `ZLONGITUDE` / `ZLATITUDE` |
| Call history | ✅ | ⬜ | separate `CallHistory.sqlite` |
| Contacts (registered) | ✅ | ⬜ | `ContactsV2.sqlite` |

### Facebook Messenger — `lightspeed-userDatabases/*.db` (native)

Schema facts from iLEAPP `facebookMessenger.py`; provenance reference (§10).

| Data | In backup | Surfaced | Notes |
|------|:---------:|:--------:|-------|
| Message text | ✅ | ✅ | `thread_messages.text` |
| Timestamp | ✅ | ✅ | `timestamp_ms` (Unix ms) |
| Direction (from-me) | ✅ | ✅ | sender vs `_user_info.facebook_user_id` |
| Conversation grouping | ✅ | ✅ | `thread_messages.thread_key` |
| Sender name | ✅ | ✅ | `contacts.name` |
| Has attachment (flag) | ✅ | ✅ | `has_attachment` |
| Attachment media | ✅ | ⬜ | `attachments` / `attachment_items` |
| Calls | ✅ | ⬜ | attachment rows tagged "call" |
| Secret / client conversations | ✅ | ⬜ | `secure_messages`, `client_messages` |

### Instagram — `DirectSQLiteDatabase/*.db` (native, unvalidated)

DMs are NSKeyedArchiver blobs in `MESSAGES.ARCHIVE`, resolved by the native
`crate::nska` decoder. Schema facts from iLEAPP `instagramThreads.py`; provenance
reference (§10). **Unvalidated against a real backup — key paths may need tuning.**

| Data | In backup | Surfaced | Notes |
|------|:---------:|:--------:|-------|
| Message text | ✅ | ✅ | `content["NSString*string"]` |
| Timestamp | ✅ | ✅ | `metadata["NSDate*serverTimestamp"]` |
| Direction (from-me) | ✅ | ✅ | senderPk vs `THREADS.VIEWER_ID` |
| Conversation grouping | ✅ | ✅ | `THREAD_ID` |
| Sender name | ✅ | ✅ | from `THREADS.METADATA` user list |
| Reactions (emoji) | ✅ | ⬜ | `reactions[].emojiUnicode` |
| Shared media | ✅ | ⬜ | `content` media object |
| Video-chat announcements | ✅ | ⬜ | `threadActivity` |

### TikTok — `AwemeIM.db` (native, unvalidated)

Messages from `TIMMessageORM`; `content` is JSON (`$.text`). Sender names join the
dynamically-named `AwemeContacts*` tables; direction uses `account_id` = the DB's
parent directory name. Schema facts from iLEAPP `tikTok.py` (reference, §10).
**Unvalidated against a real backup.**

| Data | In backup | Surfaced | Notes |
|------|:---------:|:--------:|-------|
| Message text | ✅ | ✅ | `content` JSON `$.text` |
| Timestamp | ✅ | ✅ | `localcreatedat` (Unix s/ms) |
| Direction (from-me) | ✅ | ✅ | sender vs path `account_id` |
| Conversation grouping | ✅ | ✅ | `belongingConversationIdentifier` |
| Sender nickname | ✅ | ✅ | `AwemeContacts*` join |
| Contacts (social graph) | ✅ | 🟡 iLEAPP | still via iLEAPP, tagged in Contacts |
| GIF / link / media | ✅ | ⬜ | `content` `$.url`, `$.display_name` |

### Telegram (0.4.0) · X/Twitter · Facebook (main) · Snapchat

Telegram is deferred to 0.4.0. X/Twitter, Facebook (main app), and Snapchat keep
no recoverable local chat store (no iLEAPP module exists for them), so there's
nothing to surface — see `app-support.md`.
