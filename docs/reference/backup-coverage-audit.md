# Backup coverage audit

**What an iOS backup can hold, what TraceLoupe reads today, and what is left.**
This is the source-level companion to
[`app-data-coverage.md`](app-data-coverage.md) (field-level, within a source we
already parse) and [`app-support.md`](app-support.md) (per-app native status).

The work this audit sizes is charted on
[Map: iLEAPP-parity coverage of everything a backup contains](https://github.com/PeterBlenessy/traceloupe/issues/189).

> **Method.** Numbers come from `tools/classify-ileapp-artifacts.py`, run against
> the pinned iLEAPP checkout (`pnpm setup:engine`). Re-run it rather than
> trusting the tables below — iLEAPP gains artifacts continuously, and a frozen
> table is wrong within weeks.

---

## The rule: Apple's, not ours

**iOS ships the answer on the device.** `Domains.plist` is what `backupd` reads
to decide what goes into a backup, and `tools/data/ios-backup-domains.json` is
its contents. Each domain declares a `RootPath` and several path lists; for a
**local** (iTunes/Finder) backup these are the ones that matter:

| Key | Effect |
|---|---|
| `RelativePathsToBackupAndRestore` | included |
| `RelativePathsToBackupToDriveAndStandardAccount` | included — *local backups specifically* |
| `RelativePathsToBackupIgnoringProtectionClass` | included |
| `RelativePathsToOnlyBackupEncrypted` | included **only in an encrypted backup** |
| `RelativePathsNotToBackup` | excluded |
| `RelativePathsNotToBackupToDrive` | excluded from local backups |

Keys naming *Service* or *MegaBackup* are iCloud concerns; `*Restore*` keys
describe the restore side, not what lands in the backup.

Three things this made visible that the earlier heuristic got wrong:

1. **`HomeDomain` is an allowlist.** `/var/mobile` is not backed up wholesale —
   only its listed subpaths are. That is why `Library/Biome` and
   `Library/CoreDuet/Knowledge` appear in *no* exclusion list: they are absent
   by not being included. No denylist could have told us that, and the previous
   version of this document reasoned from a denylist.
2. **A whole class of stores is in local backups but not iCloud.**
   `Library/Safari/History.db`, `Library/Safari/BrowserState.db` and
   `Library/CallHistoryDB` are not in the base include list at all — they are in
   `RelativePathsToBackupToDriveAndStandardAccount`. We parse all three today.
3. **Several sources we ship are encrypted-backup-only** — see below.

### Current split

| | Count | Meaning |
|---|---:|---|
| **Backup-reachable** | 363 | A backup contains it. The addressable universe. |
| **Encrypted-only** | 28 | Only in an encrypted backup. Reachable, conditionally. |
| **Excluded** | 135 | Under a domain root but on no include list, or explicitly excluded. |
| **Unclassified** | 72 | Neither the glob nor the device path-lists place it. |

> **These numbers have moved twice.** A hand-written heuristic gave 355/84/159;
> Apple's domain rules gave 311/26/42/219; resolving rootless globs against real
> device paths gives the figures above. Re-run the tool rather than quoting any
> of them from memory.

### Resolving rootless globs

iLEAPP writes `**/interactionC.db*` because it searches a filesystem, whereas a
backup is addressed by `(domain, relativePath)` — so a bare filename cannot be
placed by the domain rules alone. iLEAPP also ships **file-path lists extracted
from real devices** (`admin/data/filepath-lists/*.csv.zip`, ~243k distinct
basenames), and matching a glob against those supplies the missing directory.
**148 of the 219 unknowns resolve this way.**

This does *not* use a full-filesystem image to claim backup membership. The
image says only **where a file lives**; `Domains.plist` still decides whether
that location is backed up. The distinction matters — an FFS image contains
everything, including precisely what backups exclude.

The remaining 72 are mostly apps absent from those particular test devices.

### Encrypted-backup-only — including things we already ship

`RelativePathsToOnlyBackupEncrypted` is not an edge case. It contains sources
TraceLoupe surfaces today:

| Path | What it is |
|---|---|
| `Library/Safari/SafariTabs.db` | Safari open tabs (iCloud tabs) |
| `Library/CoreDuet/People/interactionC.db` | Security Check's contact-identifier scan (the Interactions view was removed in #222) |
| `Health`, `MedicalID` | the whole Health view |
| `Library/com.apple.siri.remembers` | Siri Remembers |
| `Library/locationd/user.plist` | location |
| `Library/DoNotDisturb/DB/*` | Focus modes |

**On an unencrypted backup those views are empty, and today the app does not say
why.** That is precisely the failure
[#197](https://github.com/PeterBlenessy/traceloupe/issues/197) ruled against —
"not in this kind of backup" reading as "nothing to see" — and it is not
hypothetical or future work; it is current behaviour.

### Out of scope, permanently

The 135 excluded are out of reach whatever we decide: Biome (24), knowledgeC,
unified logs, sysdiagnose, `/var/db`, `/var/log`, `Library/Caches`, and
app-container `Library/Caches`/`tmp`. The count grew from 42 because resolving
rootless globs let many artifacts be judged that previously could not be —
`Library/Caches` cases especially, including **app snapshots**, which this
document previously listed as a gap worth building.

Recorded so they are not re-investigated. **KnowledgeC is worth naming twice** —
the artifact most often assumed present, and the clearest case of a
`mobile/Library` path a backup does not carry.

---

## What TraceLoupe reads today

All native Rust, no iLEAPP at runtime (see `app-support.md` for per-source
"native since" versions).

| Source | Store |
|---|---|
| Messages (iMessage/SMS) incl. groups, tapbacks, replies, edits, deletions | `sms.db` |
| Call history | `CallHistory.storedata` |
| Contacts incl. groups, relations, photos | `AddressBook.sqlitedb` |
| Safari history, bookmarks, reading list, iCloud + local tabs | `History.db`, `Bookmarks.db`, `SafariTabs.db`, `BrowserState.db` |
| Notes incl. locked-note decryption, embedded images | `NoteStore.sqlite` |
| Voice recordings | `CloudRecordings.db` + `.m4a` |
| Camera roll + metadata (EXIF, people, albums, GPS, Live/burst, trashed, hidden) | `Photos.sqlite`, DCIM |
| Health — workouts, daily activity, sleep, GPS routes, rings, timezones | `healthdb_secure.sqlite` |
| Calendar · Reminders | `Calendar.sqlitedb` · `Reminders` store |
| CoreDuet interactions + per-app channels | `interactionC.db` |
| Device / backup metadata | `Info.plist`, `Manifest.plist` |
| Installed apps | `Info.plist` |
| 12 third-party chat apps | see `app-support.md` |

---

## Apple first-party gaps

Backup-reachable, iLEAPP parses them, we do not. This is the map's work list.

| Cluster | n | What is in it |
|---|---:|---|
| **Identifiers** | 10 | Device name, IMEI/IMSI, AirDrop ID, backup settings, Find My settings, Location Services config |
| **Files App** | 7 | iCloud Drive files, shared files, favourites, tags, iCloud app list |
| **Keychain** | 5 | Wi-Fi credentials, web passwords, mail accounts, paired Bluetooth |
| **Locations** | 5 | Apple Maps search history, Maps groups, last-activity camera |
| **Clock** | 4 | Alarms, timers, stopwatch, world clocks |
| **iCloud Shared Albums** | 4 | Album data, owners, people, emails |
| **Siri Remembers** | 3 | Calls, media, messages |
| **User Activity** | 3 | Keyboard usage stats, **dynamic lexicon** (learned typed words) |
| **Location** | 3 | `locationd`, `routined` (significant locations), Weather locations |
| **Fitness** | 2 | Workout location data + analysis |
| **Device Usage** | 1 | Last-used dates. ~~App snapshots~~ — **not reachable**: they live in an app container's `Library/Caches/Snapshots`, which backups exclude |
| **Accounts · Core Accessories · IOS Build** | 6 | Account data, accessory pairings, system version |
| **Singles** | 12 | Apple Mail, Wallet transactions, Find My devices, notifications, TCC app permissions, data usage, known Wi-Fi networks, Control Center config, OS migrations, Spotlight index, Identity Lookup, Mobile Backup plist |

### `Photos.sqlite` — 78 artifacts in 14 families

The single largest cluster, and mostly **not new stores**: they are different
queries over the one store we already parse. iLEAPP slices it into
`Ph1..Ph9` (basic asset data, trashed, hidden, locations, viewed, favourite,
adjustments, burst), `Ph10..Ph17` (embedded files, captions, people/faces,
GenAI-detected), `Ph20..Ph26` (albums, Shared-with-You conversation albums,
syndication), `Ph30..Ph35` (iCloud share methods, shared library, shared links),
`Ph50/Ph51` (internal resources, optimized assets), `Ph70` (user-adjusted
date/timezone/location), `Ph80..Ph86` (Photos preference plists) and
`Ph94..Ph98` (per-iOS-version reference tables).

What "parity" means for these is
[#196](https://github.com/PeterBlenessy/traceloupe/issues/196) — it is a real
decision, because reproducing 78 overlapping artifact lists and enriching one
Gallery are very different products.

---

## Third-party

**In scope, on the same pipeline.** The destination is everything a backup
contains that iLEAPP can parse — whose app wrote it is not a boundary. The
tiers in [`product-overview.md` §13.1](../product-overview.md) and the per-app
table in [`app-support.md`](app-support.md) order the work; they do not limit
it.

The backup-reachable third-party artifacts iLEAPP has that we do not are led by
**Booking.com (11) · Home Depot (10) · Uber (10) · ~~Waze (9)~~ · Slack (8) · Oura
Ring (7) · Withings (7) · Dahua/DMSS (7) · ChatGPT (6) · BeReal (5) · Box (5)**.
Note how few are chat apps — which matches the finding already recorded in
`app-support.md`, and is the reason the declarative module matters here as much
as for first-party: most of these are flat record stores, not conversations.

---

## The device corpus, and why disk is the constraint

Every parser and module in this repo is validated against **one** device — Josh
Hickman's iOS 17.3 iPhone 11. That is one device's apps, one iOS version, one set
of schema shapes. It is why `tcc.toml`'s third SQL alternative and
`data_usage.toml`'s WWAN-only fallback have never run against a real device, and
why 72 of iLEAPP's artifacts stay unclassifiable: they declare rootless globs that
only a device with the app installed can resolve.

So the corpus is the constraint on coverage, and **disk is the constraint on the
corpus**. One image is a ~22 GB archive that unpacks to ~34 GB of full filesystem
plus a ~2 GB backup — 56 GB per device, of which we need the 2 GB.

The policy, encoded in `scripts/fetch-test-image.sh` rather than left to memory:

```bash
scripts/fetch-test-image.sh --list     # what exists, what is here, what is reclaimable
scripts/fetch-test-image.sh ios16      # fetch → extract the backup → delete the archive
scripts/fetch-test-image.sh --prune    # drop archives and FFS trees
```

- **Keep** unpacked backups. They are the artifact, and re-fetching one costs 22 GB
  to get 2 GB back.
- **Delete** archives once the backup is out, and the full-filesystem trees. An FFS
  image is evidence of a file's shape and location — never of backup membership,
  which `Domains.plist` decides — so it answers no question this project asks.
- **Check space before downloading**, not 20 GB in.

`tools/data/dfir-images.json` is the catalogue of what exists, so a fresh session
does not re-research it. It deliberately records no "have this one" flag: what is
on disk is what is on disk, and a committed flag goes stale the moment someone
prunes. Ask the script.

With no arguments, `validate_against_real_backup` runs every module against
**every** backup in the corpus. A validator that must be pointed at one device at a
time becomes a validator that is only ever run against the newest one.

---

## A trap worth naming: iLEAPP's declared `paths` is not where its data is

`waze.py` declares `com.waze.iphone.plist` as its path for eight artifacts. That
plist is used only to find the app's **container id**; the search history,
recent destinations and favourites are read from `Documents/user.db`, a plain
SQLite database. Dumping the declared path and finding nothing but Firebase
configuration led to Waze being recorded as "protobuf, out of reach for a
declarative module" — which was wrong, and cost a round trip.

**Read the artifact's code, not its manifest.** The `paths` entry is what iLEAPP
globs for to decide the artifact applies; the data can come from anywhere the
seeker can reach.

A second trap in the same family: **iLEAPP's `sample_data` row counts are
measured against full-filesystem images, not backups.** They are a good signal
that data EXISTS on a device, and no signal at all that a backup carries it.
Both were nearly used here to justify fetching a 22 GB image for data that was
already on disk.

---

## Implemented is not verified

Two different claims, and this document used to make only one of them.

A module can be written from iLEAPP's definition, load, run, and pass its fixture
while reading the wrong key — the fixture was written from the same reading of
the store, so it **agrees with the module by construction**. Only a real device
settles it.

Both states ship. Waiting for a backup before writing a module means the module
never gets written, and iLEAPP's own definitions plus its per-image sample counts
are a good enough basis to build from. What must not happen is the two being
confused, so they are tracked apart:

| | means |
|---|---|
| **Implemented** | exists, loads, runs, produced rows from a fixture |
| **Verified** | its output was checked against a **named real backup** |

The verified note lives in each module's own `verified` field, so the table below
is read from the modules and cannot drift from them. Regenerate it with:

```bash
python3 tools/module-status.py            # this table
python3 tools/module-status.py --summary  # just the counts
```

### Backups used for verification

| Corpus key | Device | What it is |
|---|---|---|
| `iphone11_ios17` | iPhone 11, iOS 17.3 | Josh Hickman's public research image — the encrypted backup, not the full-filesystem tree |
| `iphone11_ios16` | iPhone 11, iOS 16.1.2 | **The same physical phone** (serial F4GZ987AN72N) one OS earlier, which is what makes it a drift check rather than a second sample |
| `iphone_se_ios13` | iPhone SE, iOS 13.3.1 | A **different** device (serial DX3T126VH2XV), four years older — the lineage that exercises every older-schema SQL alternative the modules carry |
| `iphone_se_ios13_4` | iPhone SE, iOS 13.4.1 | The same SE one point release later. Weaker than a major-version pair for drift, kept because 420 MB is cheaper than deciding whether to want it |

The iPhone 11 pair decrypt with the same password; **the iPhone SE does not** — its
password is lowercase `mypassword123`, documented only in the image-creation PDF.
Its `image_info.txt` says "Password Protected: false" while the Manifest says
`IsEncrypted: true`, so the obvious file is the misleading one.

**Fetching it cost 396 MB, not 8.9 GB.** The archive is one zip; the iTunes backup
inside it is a single member. `tools/fetch-zip-member.py` range-fetches just that
member — read the End of Central Directory, find the entry, pull its bytes. Two of
the four catalogue entries were also wrong and are corrected: `ios15` does not exist
on Digital Corpora at all, and `ios13`'s URL 404'd. The catalogue recorded `ios16` as "not yet
fetched", with no password and the wrong device — so every module "failed"
against it in corpus mode, unauthenticated. Corrected from the fetched backup's
own `Info.plist`.

**A second lineage paid for itself immediately.** `healthdb.sqlite` has **no
`device_context` table at all** on iOS 16.1.2; the module's only query failed
with "no such table" and took the artifact down. It now has an iOS 16
alternative reading `source_devices`, with the local device identified the way
the schema itself does (`sources.local_device = 1`) rather than by guessing at
`model = 'iPhone'`, which would be wrong on an iPad.

**Absence is a fact about a device, not a defect.** That older phone has no
`ACXRemoteAppList.plist` and no `com.apple.MobileBackup.plist`; the same phone at
17.3 has both. The corpus validator used to count those as failures, which made
the report get *worse* every time an older device was added — and a validator
that always fails is one nobody runs. It now fails only for a module absent from
**every** backup, which is a wrong path, and notes the rest.

### Auditing what we already ship, against what iLEAPP actually reads

Two rounds of this found real gaps, and both were invisible from the coverage
numbers because the category already counted as covered:

- **`home_screen` was shipping UUIDs.** A widget stack is an icon whose
  `displayIdentifier` is a UUID; the names are one level down in `elements`.
  "iOS Screens" had looked covered since `home_screen` and `dock` shipped — it
  was covered for apps and blind to widgets.
- **`imei_imsi` read only the nested `PersonalWallet` subtree.** The same
  plist's TOP-LEVEL keys are the device's last NETWORK state — MCC, MNC, carrier
  bundle, `LastKnownICCID` — and nothing was reading them.

The method: for each shipped module, find the iLEAPP script reading the same
store and diff what it extracts against our columns. A category counting as
"touched" says nothing about whether the module reads all of the store, and the
coverage tool is explicit that it never claims otherwise — this is how to check.

### Property lists hidden in database columns

Apple stores structured data in BLOB columns constantly, and until now nothing
here could see inside one: `plist` reads files, and `sql` renders a blob as
`<12502 bytes>`. `explore_real_backup blob` decodes it with the same
NSKeyedArchiver resolver the module runner uses.

The case that forced it, and the key paths it settled, so nobody re-derives
them — `NoteStore.sqlite`, `ZICCLOUDSYNCINGOBJECT.ZSERVERSHAREDATA`, a 12 KB
archive holding the CloudKit share for a note:

| What | Path inside the archive |
|---|---|
| E-mail of each participant | `LastFetchedParticipants / N / UserIdentity / LookupInfo / EmailAddress` |
| Their name | `… / UserIdentity / NameComponents / NS.nameComponentsPrivate / NS.givenName` + `NS.familyName` |
| Whether they accepted | `LastFetchedParticipants / N / AcceptanceStatus` |
| What they may do | `LastFetchedParticipants / N / Permission` |

On the validation device that resolves to **This Is DFIR
&lt;thisisdfir@gmail.com&gt;** on "iOS 16 Note". `ZICNOTEPARTICIPANT` gives the
count and a `__defaultOwner__` marker for the note's own owner; the blob gives
the identities.

**Shipped.** `parse_notes` decodes the archive and writes
`notes.shared_with_json` (schema v56); Notes marks a shared note in the list and
names every participant in the detail pane. The owner is among them — a CloudKit
share always names them — so the wording is "shared with" and lists everyone,
rather than counting strangers and getting it off by one.

A note whose share blob will NOT decode is left unshared-looking rather than
written as `[]`: "we could not read it" and "it is not shared" are different, and
the parser skips instead of asserting the second.

### One table, three different things

`ZCLOUDSHAREDCOMMENT` in `Photos.sqlite` holds **likes, captions and free-form
comments together**, and its row count says nothing about which. On the
validation device its 18 rows are **15 likes and 3 captions, with no comments at
all** — so "18 comments" would have been wrong three ways over.

The store flags them itself (`ZISLIKE`, `ZISCAPTION`, `ZISMYCOMMENT`), so
nothing is inferred from whether the text happens to be NULL. Shipped as
`media_items.shared_caption` / `shared_likes` (schema v57), shown in the Photos
lightbox: a like is counted because fifteen rows saying "someone liked this"
*are* a number, and a caption is kept verbatim because it is text a person
wrote.

`ZISMYCOMMENT` is deliberately **not** filtered out — a caption the owner wrote
on a shared album is still evidence they shared it, and dropping it would leave
an album nobody appears to have touched.

### Same-store artifacts split by who reads the store

`coverage-gap.py --present` reports 16 artifacts that read a file we **already
parse** — "no new parsing, analysis we do not do on data already in the cache".
That framing is right about the data and wrong about the cost, because it
depends on WHICH layer reads the store:

- **A store read by a MODULE** (Podcasts' `MTLibrary.sqlite`, AllTrails) takes
  another module. `check-artifact-overlap.mjs` compares domain + path + *what is
  read within*, so two modules over one file are fine.
- **A store read NATIVELY** (`NoteStore.sqlite`, `Photos.sqlite`,
  `interactionC.db`, `IconState.plist`) cannot take a module at all — the
  overlap guard rejects any module over a natively-parsed store, deliberately.
  Those artifacts need the native parser, the cache schema and the view to
  change together.

So of the 16, the cheap ones are the module-read stores and the rest are
parser work. `ZICNOTEPARTICIPANT` — **who a note was shared with**, 4 rows on
the validation device — is the most valuable of the blocked ones and is the
argument for doing that parser work.

### Extensionless stores were invisible for weeks

`Login Data`, `Web Data` and `Top Sites` are SQLite databases with **no file
extension**. Every audit this project ran enumerated `%.db`, `%.sqlite`,
`%.storedata` and `%.sqlite3` — so all three were invisible, in all three
Chromium browsers, on every device in the corpus.

They surfaced only when `coverage-gap.py --present` was pointed at a real
backup's full path list, which is the third time a *method* change found more
than a search did (after "read the code, not the manifest" and "enumerate by
store type, not by filename"). **Enumerate by what the manifest actually holds.**

A related correction: this document previously recorded that no third-party
browser ships anything readable. `History` is indeed absent on every device —
but `Top Sites`, `Login Data` and `Web Data` are present in Chrome, Edge and
Brave alike.

### What the third lineage caught

`sleep_schedule` failed outright on iOS 13.3.1: `MTSleepAlarms` does not exist
there, because **Sleep Schedule arrived in iOS 14**. Strict path-walking took the
whole artifact down over a device that simply predates the feature — the same
shape as the `MTStopwatches` absence, and the same fix (`optional = true`).

That is three sessions running where the OLDER device found something the newer
ones could not. The pattern is worth stating plainly: **a corpus of one device is
a corpus of one iOS version**, and every schema fallback a module carries is
untested until something old enough to need it turns up.

### What verifying actually caught

Three defects that every fixture-based test agreed with, because the fixtures
were written from the same reading of the store as the modules:

- `timers` declared `MTTimerLastModifiedDate`, which **iLEAPP also reads**. The
  real timer dict has eight keys and that is not one of them, so the column
  could only ever have been empty.
- `MTTimerFireTime` is **polymorphic** — `$MTTimerDate` for a running timer,
  `$MTTimerTimeInterval` for a stored one, with `MTTimerFireTimerClass` naming
  which. The fixture had only the date shape; the device has only the interval
  one.
- `stopwatch` failed outright: `com.apple.mobiletimerd.plist` has **no
  `MTStopwatches` key at all** when the stopwatch has never been run. Strict
  path-walking took the whole artifact down over the ordinary state of a device,
  which is what `[plist] optional` now exists for.

None of these were reachable by reasoning. That is the argument for the
implemented/verified split, and for keeping the corpus.

`tools/data/dfir-images.json` catalogues what else exists and
`scripts/fetch-test-image.sh` fetches it. Backups are **kept**; archives and
full-filesystem trees are **pruned** — an FFS image shows where a file lives,
never whether a backup carries it, which is the only question this document
asks. Check free space before fetching, not 20 GB in.

| Module | Category | Implemented | Verified against |
|---|---|:-:|---|
| Accounts | Device | ✅ | iphone11_ios17 — 19 accounts across 12 services |
| AirDrop | Device | ✅ | iphone11_ios17 — AirDrop ID 5de9c0ec2f83; DiscoverableMode is not written on this device, so it reads as unrecorded |
| Alarms | Device | ✅ | iphone11_ios17 — one alarm at 10:41, switched off, last changed 2024-07-28 |
| AllTrails recordings | Locations | ✅ | iphone11_ios17 — 6 recordings between November 2021 and July 2024 |
| Backup history | Device | ✅ | iphone11_ios17 — computer backup 2024-01-23 (EST), iCloud backup 2024-07-22 (GMT-4), iCloud on: the iCloud copy is the MORE RECENT of the two |
| Backup size by domain | Device | ✅ | iphone11_ios17 — 42 domains sized |
| Bluetooth devices | Device | ✅ | iphone11_ios17 — 5 devices, including two sets of AirPods named after different people |
| Nearby Bluetooth | Device | ✅ | iphone11_ios17 — 1056 sightings, 5 named |
| Bluetooth pairings | Device | ✅ | iphone11_ios17 — 3 paired devices — a Garmin vívoactive 4, a Fitbit Versa 3 and an Apple Watch |
| CarPlay apps | Device | ✅ | iphone11_ios17 — 7 apps, including com.waze.iphone, com.spotify.client and com.google.Maps, last used between 2024-01-16 and 2024-07-27 |
| CarPlay connection | Device | ✅ | iphone11_ios17 — last session ended 2024-07-27T16:21:39Z at 84% battery, thermal level 'None' |
| Cellular network | Network | ✅ | iphone11_ios17 — MCC 310 / MNC 260 (United States), carrier bundle 310260_GID1-4276, ICCID 8901260971148676693 which MATCHES the one sim_cards reads from a different store, number 19195794674, last OS 17.3 |
| Saved logins (Chromium browsers) | Security | ✅ | iphone11_ios17 — the store is present in ALL THREE browsers (Chrome, Edge, Brave) and holds 0 rows in each: installed, never used to save a login. The schema is read off the device; the output against a populated store is unproven |
| Most-visited sites (Chromium browsers) | Network | ✅ | iphone11_ios17 — 1 row, nhl.com at rank 0 in Chrome, with the vendor profile captured as 'Google/Chrome' by the ** glob; the same site service_workers found in Safari from a different store |
| Data usage | Network | ✅ | iphone11_ios17 — 1959 usage rows collapse to 671 per-app totals, topped by the App Store at 2.77 GB and TikTok at 300 MB |
| Language and region | Device | ✅ | iphone11_ios17 — en-US, en_US, 24-hour time on |
| Dock | Device | ✅ | iphone11_ios17 — 4 apps, Phone first |
| Find My | Device | ✅ | iphone11_ios17 + iphone11_ios16 — DSID 17193901029 on both, enabled 2023-07-01; 'Send last location' was OFF at 16.1.2 and ON at 17.3, so the setting is read, not defaulted |
| Health device | Device | ✅ | iphone11_ios17 — iPhone12,1 running iOS 17.3, recorded 2024-08-02; iphone11_ios16 — same device at 16.1.2 via the source_devices fallback, since device_context does not exist there |
| Home screen | Device | ✅ | iphone11_ios17 — 5 pages, 18 icons on the first |
| Home screen widgets | Device | ✅ | iphone11_ios17 + iphone11_ios16 — 6 widgets (Weather, Maps and others) with their page/slot positions, resolving the two anonymous 'custom' UUIDs the layout shows on page 0; iphone_se_ios13 + iphone_se_ios13_4 — 0 rows, because home screen widgets did not exist before iOS 14 |
| iCloud containers | Files | ✅ | iphone11_ios17 — 43 containers, com.apple.CloudDocs at 25 items / 20 documents / 22 MB; iphone11_ios16 — 43 again but 24 items / 12 documents / 5.1 MB, so the counts track real growth rather than being static |
| iCloud devices | Files | ✅ | iphone11_ios17 + iphone11_ios16 — 3 devices on both, one of them a Mac; two of the three are not the phone being examined |
| iCloud Drive files | Files | ✅ | iphone11_ios17 — 27 files resolving to real folders (Desktop/, Documents/, Downloads/, Dictionaries/…) with sizes and all three dates; iphone11_ios16 — 26 on the same phone one OS earlier, and the one file's Shared flag differs between them, so the flags are read not defaulted |
| Cellular identity | Device | ✅ | iphone11_ios17 + iphone11_ios16 — 1 SIM, IMEI 353985100845978, IMSI 310260974867669, +1 919 579 4674 on PLMN 310260; identical across both OS versions, which is what a handset identifier should be |
| Life360 location history | Location | ✅ | iphone11_ios17 — 1,635 rows from 48 logs across all three directories. That number was checked against the files rather than against iLEAPP. Dumping all 48 logs and counting the marker directly gives 1,635… |
| Location access | Security | ✅ | iphone11_ios17 — 189 clients including TikTok, Gmail and Apple Maps |
| Location Services | Device | ✅ | iphone11_ios17 — Location Services on, last written by iPhone OS17.3/21D50 |
| MEGA files | Files | ✅ | iphone11_ios17 — 966 files across 332 folders, resolving to real paths such as 'Cloud Drive/My chat files/IMG_4552.jpg'; the _status_ and _transfers_ sibling stores the path glob also matches are skipped, which is what the globbed-skip rule exists for |
| Message retention | Device | ✅ | iphone11_ios17 — 'Forever' via the value map, from the iOS 17+ key; the iOS 16 key is absent as expected on this lineage |
| OS build history | Device | ✅ | iphone11_ios17 — 2 boots, 20B110 (2023-07-01) then 21D50 (2024-01-25); iphone11_ios16 — the SAME phone shows only the first, because the upgrade had not happened yet, so the artifact corroborates the corpus |
| Podcast episodes | Media | ✅ | iphone11_ios17 — 35 of 1,774 cached episodes, i.e. the ~2% someone acted on; iphone11_ios16 — 35; iphone_se_ios13 — 18 and iphone_se_ios13_4 — 24, the SAME device three days apart, so the count tracks real listening rather than the feed |
| Podcasts | Media | ✅ | iphone11_ios17 — 6 subscriptions, one with a 2021 last-played date |
| Sites with a service worker | Network | ✅ | iphone11_ios17 — 2 registrations, nhl.com in Safari and one in DuckDuckGo, with the worker's own script URL; absent on both iPhone SE lineages, where service workers were barely in use in 2020 |
| SIM cards | Device | ✅ | iphone11_ios17 — 1 SIM in slot 1, with its ICCID, its number, and a July 2024 update |
| Siri | Device | ✅ | iphone11_ios17 — voice 'nora', en-US, cloud sync on |
| Sleep schedule | Device | ✅ | iphone11_ios17 — bedtime 22:45, wake 06:00, switched off, tracking off; iphone_se_ios13 — 0 rows, because MTSleepAlarms does not exist before iOS 14 |
| Stopwatch | Device | ✅ | iphone11_ios17 — 0 rows: the MTStopwatches key is absent entirely, which is what taught this module it must be optional |
| Permissions | Security | ✅ | iphone11_ios17 — 289 rows, which is exactly the count iLEAPP records for the same image — two independent parsers agreeing. The distribution also justified passing unknowns through rather than guessing: alongside… |
| Timers | Device | ✅ | iphone11_ios17 — 1 timer, the stored 'CURRENT_TIMER' placeholder: 15 min, fire-time class MTTimerTimeInterval, so 'Due' is correctly empty |
| Apple Watch apps | Device | ✅ | iphone11_ios17 — 47 apps on one paired watch |
| Waze favourites | Locations | ✅ | iphone11_ios17 — 1 favourite, slot 2, named 'Work' at 605 Bridge St, Fuquay-Varina NC |
| Waze places | Locations | ✅ | iphone11_ios17 — 7 places with street addresses and decimal coordinates (Burke VA, Fuquay-Varina NC), matching the 7 rows iLEAPP records for this image |
| Waze destinations | Locations | ✅ | iphone11_ios17 — 6 recent destinations, matching iLEAPP's 6 for this image; last-used and first-added differ on some rows, so both are read |
| Web domains loaded in apps | Network | ✅ | iphone11_ios17 — 352 domains across 42 app containers, from Edge/Chrome/DuckDuckGo/Brave to Signal, Discord, Reddit and Zoom; this WebKit has no firstSeen/isPrevalentResource so the second SQL alternative is the one that runs, and both report NULL rather than a value |
| Wi-Fi networks | Network | ✅ | iphone11_ios17 — 17 known networks, with join dates from July 2023 to January 2024 |
| Private Wi-Fi addresses | Network | ✅ | iphone11_ios17 — 17 networks with their private addresses, join times and rotation timestamps |
| World Clock | Device | ✅ | iphone11_ios17 — 4 cities (Cupertino, New York, UTC, …) with coordinates; matches the 4 rows iLEAPP records for this same image |

**48 implemented · 48 verified · 0 awaiting a real backup.**

---

## What is left, as a number that goes down

`classify-ileapp-artifacts.py` says which of iLEAPP's artifacts a backup can
contain. `coverage-gap.py` says which of those we read:

```bash
python3 tools/classify-ileapp-artifacts.py --json /tmp/ileapp.json
python3 tools/coverage-gap.py            # summary + the biggest gaps
python3 tools/coverage-gap.py --all      # every untouched category
```

`--present` splits the remaining gap by what a REAL backup contains, which is the
difference between a list and a worklist:

```bash
cargo run -p traceloupe-core --example explore_real_backup -- <dir> <pw> list '%' \
    | tail -n +3 > /tmp/allpaths.txt
python3 tools/coverage-gap.py --present /tmp/allpaths.txt
```

Three outcomes, and they are different kinds of work:

| | meaning |
|---|---|
| **same store** | the artifact reads a file we already parse — no new parsing, just analysis we do not do on data already in the cache |
| **unread store** | the file is in this backup and nothing reads it — real work, provably worth doing on this device |
| **not here** | the file is not in this backup at all — this device simply lacks the app, so it can be neither built nor ruled out without another device |

On the iOS 17 image that splits 187 untouched artifacts into **12 / 21 / 154**.
The last number is the one that matters: most of what is left cannot be settled
by working harder on the device we have. It is the corpus argument again, with a
figure attached.

It reads our coverage **from the source** — `APP_CHAT_MODULES`, the module TOMLs,
the parser files, the app catalog — so a parser added tomorrow counts without
anyone editing the tool.

It reports a category as **touched**, never as finished. iLEAPP groups artifacts
by product, and a category is rarely all-or-nothing: "Clock" is Alarms, Stopwatch,
Timers and WorldClock and we read one of four. Saying "we support Clock" is
exactly the kind of claim that stops someone looking, so the tool refuses to make
it — the alias table records what each claim rests on, including what it excludes.

---

## Settling a candidate against a real backup

This audit says what Apple's rules imply. Before writing a module, check the
implication against a real device with `explore_real_backup`, which decrypts a
backup with the app's own decryptor and reads its Manifest:

```bash
# The SHAPE of a property list: key paths, types, sample values. The plist
# equivalent of `schema`, and the thing to read before writing a `[plist]` block.
cargo run -p traceloupe-core --example explore_real_backup -- <dir> <password> \
    plist SystemPreferencesDomain com.apple.wifi.known-networks.plist

# Is this store actually in a backup? (SQL LIKE, % wildcards)
cargo run -p traceloupe-core --example explore_real_backup -- <dir> <password> \
    list '%voicemail%'

# What is its real schema, and does anything populate it?
cargo run -p traceloupe-core --example explore_real_backup -- <dir> <password> \
    schema HomeDomain Library/Accounts/Accounts3.sqlite

# Does a candidate query return the rows you expect?
cargo run -p traceloupe-core --example explore_real_backup -- <dir> <password> \
    sql HomeDomain Library/Accounts/Accounts3.sqlite 'SELECT ... LIMIT 5'
```

Use Josh Hickman's public image (`scripts/fetch-test-image.sh`) — **never the
owner's own backup** (AGENTS.md). It pays for itself immediately: the row counts
are what killed voicemail as a candidate (present but empty on the validation
image, so nothing would have been proven) and what redirected the accounts module
off the Apps surface (every owning bundle id is a system daemon, so the join
attached nothing).

---

## Known weaknesses of this audit

Stated so nobody mistakes it for more than it is:

1. **The 219 unclassified artifacts** are unresolved, so the destination's size
   is a range, not a number ([#192](https://github.com/PeterBlenessy/traceloupe/issues/192)).
2. **The domain rules are transcribed, and from iOS 16.4.**
   `ios-backup-domains.json` comes from
   [a third-party transcription](https://gist.github.com/leminlimez/c602c067349140fe979410ef69d39c28)
   of an iPhone SE 3, not a file we extracted. Apple **moved** it in iOS 17.0
   (to `MobileBackup.framework/Domains.plist`) and may have changed it.
   Authoritative for 16.4, strongly indicative for later. To check it against a
   real iOS 17 copy: `scripts/fetch-test-image.sh`, then
   `scripts/fetch-test-image.sh --list 'Domains.plist'`.
   One symptom is already visible: iLEAPP's `safariTabs` module targets
   `CloudTabs.db`, which is on no 16.4 list, while `SafariTabs.db` is — the
   filename changed and the module did not follow.
3. **"iLEAPP has a module" is not "the data is there."** A module proves the
   artifact exists on *some* device. Whether a given backup contains rows is a
   separate question — `app-data-coverage.md` already records several stores
   (Maps, Podcasts, Journal, Wallet) that are present but empty on a real
   device.
4. **iLEAPP is not the whole universe either.** It is the best open catalogue of
   iOS artifacts, but an artifact with no iLEAPP module is not thereby absent —
   `app-support.md` records several such apps found by research alone.
5. **Category names are iLEAPP's**, and a few are misleading: the
   `Photos.sqlite-*` families are Apple first-party despite sorting oddly, and
   `Health & Fitness` holds third-party (AllTrails) rather than Apple Health.
