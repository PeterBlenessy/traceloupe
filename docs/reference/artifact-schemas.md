# Artifact schemas and their traps

Layout facts about the databases TraceLoupe parses, and the mistakes each one has
already caused here. This is **Apple's schema and public DFIR knowledge** — it
contains no data from any real backup, and must stay that way (see "Never touch
the user's real backup data" in AGENTS.md). Record *shapes*, never counts,
contents, or anything measured from a real device.

Everything below was verified against an authoritative source, not inferred.
When you add to it, cite where the fact came from.

## Where to verify a fact

Never ship a guess about a schema. Wrong labels on forensic or health data are a
real harm, not a cosmetic bug. In order:

1. **[iLEAPP](https://github.com/abrignoni/iLEAPP)** — the reference implementation
   for iOS artifact parsing. Not a runtime dependency (see `app-support.md`), but
   still the first place to look. Its `admin/test/cases/data/<Artifact>/` holds
   per-artifact fixtures across many apps and iOS versions — single extracted
   databases, not backups, so they suit native parser tests rather than
   normalizer tests.
2. **Apple's own enums** — `HKQuantityTypeIdentifier`, `HKCategoryValue*`,
   `PHAssetPlaybackStyle`. Apple's JS-rendered docs are awkward to read;
   the Microsoft Learn mirror of the HealthKit enums renders plainly.
3. **The domain expert for that artifact**, then replicate their approach —
   `christophhagen/HealthDB` (`HKCategoryTypeIdentifier+SampleType.swift`) is
   authoritative for HealthKit type→identity mapping; `kacos2000` for Photos.

If none of them answer it, the honest move is to leave the value raw. iLEAPP
itself leaves call disconnect-cause codes unmapped beyond two values; guessing
the rest would be inventing forensic conclusions.

## Timestamps

Apple stores time in at least three ways in the artifacts we read.

- **Cocoa/Mac epoch** — seconds since 2001-01-01. Read as Unix time it lands in
  1970, which is why the analytics time axis is clamped to `TIMELINE_START`
  (2007-01-01) rather than trusting the minimum value in the data.
- **Nanoseconds since 2001** in newer `sms.db` columns. `mac_to_unix` in
  `messages.rs` divides by 1e9 when the value exceeds 1e12 — the same column
  holds both forms depending on iOS version.
- **REAL Unix epoch** in Safari, and **INTEGER Unix epoch** in third-party chat
  databases.

## Messages (`sms.db`)

**`threads.identifier` is not a phone number.** It is `chat.ROWID`. The handle —
phone or email — is `chat.chat_identifier`, which we store in `display_name`.
Resolve contacts against `display_name`; matching on `identifier` once made the
Messages view show raw numbers.

**Phone matching is suffix-based.** Handles arrive fully international
(`+46701234567`) while contacts are stored nationally (`070-123 45 67`). Match on
the **last 8 digits** (`phoneOrEmailKey` in `src/lib/use-contact-resolver.ts`),
which ignores country code and trunk zero. Emails match lowercased.

**Group chats** need the raw database. `chat.display_name` is the group name and
`chat_handle_join → handle.id` the members; a thread is a group when it has more
than one participant.

**Recently-deleted messages live in a second join table.** `chat_recoverable_message_join`
does not overlap `chat_message_join`, so a parser reading only the latter never
sees them. Fold it in with a table-existence guard.

**Stickers must be classified from attachment metadata, not body text.** A
plausible-looking guard — "it is a sticker if it has no text" — classifies none
of them, because sticker attachments decode to text in `attributedBody` too. The
attachment metadata also only loads when the attachment resolver runs, so the
sticker set has to be loaded by an independent join.

## Contacts (`AddressBook.sqlitedb`, `AddressBookImages.sqlitedb`)

Photos come from `ABThumbnailImage.data WHERE record_id = ABPerson.ROWID AND
format = 0` — **the `format = 0` filter matters** — with `ABFullSizeImage.data` as
a fallback. The bytes are directly renderable. Photos populate only at import, so
existing caches need a re-import to gain them.

Multi-value properties are keyed by number: 23 = related names, 46 = social/IM
profiles.

## Safari (`Bookmarks.db`, `SafariTabs.db`, `BrowserState.db`)

Both `Bookmarks.db` and `SafariTabs.db` use a single `bookmarks` table
(`id, special_id, parent, type, title, url, order_index, added, last_modified`
REAL epoch, `deleted`, `extra_attributes` BLOB). `type` is 0 for a leaf, 1 for a
folder.

Special folders by `special_id`: 0 = root, 1 = bookmarks bar,
3 = `com.apple.ReadingList`, 4 = `com.apple.WebFilterWhiteList`. **The web-filter
subtree is a parental-control allowlist, not user bookmarks** — exclude it, or
sites the user never saved appear as theirs.

- **Bookmarks** = type-0 leaves outside the reading-list and web-filter subtrees.
- **Reading list** = type-0 leaves under `special_id = 3`; its dates and preview
  text live in the `extra_attributes` bplist under `com.apple.ReadingList`.
- **Tabs** = type-0 leaves with a URL, parent folder being the tab group;
  `windows_tab_groups` maps groups to windows.

## Photos (`Photos.sqlite`)

People: join `ZASSET ← ZDETECTEDFACE.ZASSETFORFACE`, then
`ZDETECTEDFACE.ZPERSONFORFACE → ZPERSON`; named people have a non-empty
`ZFULLNAME`/`ZDISPLAYNAME`. The asset path is
`Media/<ZASSET.ZDIRECTORY>/<ZASSET.ZFILENAME>`, matched against
`media_items.relative_path` by suffix.

**Asset subtype codes are easy to invert**, and did get inverted here — a panorama
mislabel shipped because `ZKINDSUBTYPE == 2` is a Live Photo *still frame*;
the panorama is `== 1`. A Live Photo is `ZPLAYBACKSTYLE == 3`, and a burst is
identified by `ZAVALANCHEUUID`. Verified against `kacos2000`'s decode and Apple's
`PHAssetPlaybackStyle`.

The camera roll pairs each DCIM asset with iOS's pre-rendered thumbnail at
`Media/PhotoData/Thumbnails/V2/DCIM/<album>/<file>/<size>.JPG`. Serving those
directly is why import does no image decoding and the grid is instant.

## Health (`healthdb_secure.sqlite`)

Type codes are numeric and **must be looked up, never guessed**. Confirmed
against `christophhagen/HealthDB`:

| Quantity | | Category |
| --- | --- | --- |
| 5 heart rate · 7 steps · 8 distance | | 63 sleep analysis · 91 cervical mucus · 92 ovulation |
| 9 basal energy · 10 active energy · 12 flights | | 95 menstrual flow · 96 intermenstrual bleeding |
| 173 headphone audio · 182 double support | | 97 sexual activity · 99 mindful session |
| 187 walking speed · 188 step length · 194 asymmetry | | 157–171 symptoms · 178 audio event |

Heart rate is canonically stored as count/second — multiply by 60 for bpm.
Distance is metres, energy kcal.

Two traps: **95 is not "stand hours"** (an earlier guess here, wrong — its values
are 1–5, a flow level); and per-day aggregates must dedupe by
`data_provenances.source_id`, or a day with both a watch and a phone
double-counts every cumulative metric.

## Third-party chat

**TikTok spans two databases**, which is why it has a dedicated parser rather
than using the generic single-file app-chat path:

- messages in `…/Library/Application Support/ChatFiles/<account_id>/db.sqlite`,
  table `TIMMessageORM` — and the `<account_id>` folder name *is* the local
  user's uid, which is how direction is determined. Bring the `-wal` file.
- sender names in `AwemeIM.db` (`AwemeContacts*`, `TTKIMContactBaseUser*`).

Classify messages by the shape of the `content` JSON, not by the `type` code:
7 = text, 1 = system notice (text is in `$.tips`), 40 = shared video
(`$.aweme_id`), 8 = shared profile, 5 = sticker or nudge, 1805/1809 = empty
control rows to skip.

**Shared media is not in the backup.** For shared videos and stickers `content`
is a placeholder, `contentPb` is NULL, and `ext` holds only routing metadata —
only the `aweme_id` survives. iLEAPP hits the same wall. Resolving those would
need TikTok's API at view time, which is an online call in an otherwise fully
offline app, so it must be explicitly opt-in.

WhatsApp and Telegram specs are **structural, derived from module source and
unvalidated against real data**. Treat them as unverified until a backup with
those apps confirms them.

Two apps that look like gaps but are not: Instagram DMs are server-side, and
Snapchat's chat store is excluded from backups by design. Neither is a parser
bug, and neither is fixable.

## When data is missing, prove where it went

An empty view has three very different causes, and guessing between them wastes
days. Check which one applies before touching a parser:

1. **The file is not in the backup.** Safari history and call history are absent
   from some backups entirely. Check the backup manifest first.
2. **The row exists and the blob does not** — the asset was offloaded to iCloud.
   Note that `transfer_state` is **not** an offload flag: it is a transfer
   completion latch, not rewritten when a file is offloaded. The reliable model
   is structural — row present with no blob means offloaded, row absent means
   deleted. See `docs/adr/0003-icloud-offloaded-media-two-tier.md`.
3. **The parser skipped it.** Only conclude this after ruling out 1 and 2.
