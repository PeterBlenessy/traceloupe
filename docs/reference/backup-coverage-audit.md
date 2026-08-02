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
**Booking.com (11) · Home Depot (10) · Uber (10) · Waze (9) · Slack (8) · Oura
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

`tools/data/dfir-images.json` catalogues what else exists and
`scripts/fetch-test-image.sh` fetches it. Backups are **kept**; archives and
full-filesystem trees are **pruned** — an FFS image shows where a file lives,
never whether a backup carries it, which is the only question this document
asks. Check free space before fetching, not 20 GB in.

| Module | Category | Implemented | Verified against |
|---|---|:-:|---|
| Accounts | Device | ✅ | iphone11_ios17 — 19 accounts across 12 services |
| AirDrop | Device | ✅ | — not yet — |
| Alarms | Device | ✅ | iphone11_ios17 — one alarm at 10:41, switched off, last changed 2024-07-28 |
| AllTrails recordings | Locations | ✅ | iphone11_ios17 — 6 recordings between November 2021 and July 2024 |
| Backup history | Device | ✅ | — not yet — |
| Backup size by domain | Device | ✅ | iphone11_ios17 — 42 domains sized |
| Bluetooth devices | Device | ✅ | iphone11_ios17 — 5 devices, including two sets of AirPods named after different people |
| Nearby Bluetooth | Device | ✅ | iphone11_ios17 — 1056 sightings, 5 named |
| Bluetooth pairings | Device | ✅ | iphone11_ios17 — 3 paired devices — a Garmin vívoactive 4, a Fitbit Versa 3 and an Apple Watch |
| CarPlay apps | Device | ✅ | iphone11_ios17 — 7 apps, including com.waze.iphone, com.spotify.client and com.google.Maps, last used between 2024-01-16 and 2024-07-27 |
| CarPlay connection | Device | ✅ | iphone11_ios17 — last session ended 2024-07-27T16:21:39Z at 84% battery, thermal level 'None' |
| Data usage | Network | ✅ | iphone11_ios17 — 1959 usage rows collapse to 671 per-app totals, topped by the App Store at 2.77 GB and TikTok at 300 MB |
| Language and region | Device | ✅ | iphone11_ios17 — en-US, en_US, 24-hour time on |
| Dock | Device | ✅ | iphone11_ios17 — 4 apps, Phone first |
| Health device | Device | ✅ | iphone11_ios17 — iPhone12,1 running iOS 17.3, recorded 2024-08-02 |
| Home screen | Device | ✅ | iphone11_ios17 — 5 pages, 18 icons on the first |
| Life360 location history | Location | ✅ | iphone11_ios17 — «redacted» rows from 48 logs across all three directories. That number was checked against the files rather than against iLEAPP. Dumping all 48 logs and counting the marker directly gives «redacted»… |
| Location access | Security | ✅ | iphone11_ios17 — 189 clients including TikTok, Gmail and Apple Maps |
| Location Services | Device | ✅ | — not yet — |
| Message retention | Device | ✅ | — not yet — |
| Podcasts | Media | ✅ | iphone11_ios17 — 6 subscriptions, one with a 2021 last-played date |
| SIM cards | Device | ✅ | iphone11_ios17 — 1 SIM in slot 1, with its ICCID, its number, and a July 2024 update |
| Siri | Device | ✅ | iphone11_ios17 — voice 'nora', en-US, cloud sync on |
| Sleep schedule | Device | ✅ | iphone11_ios17 — bedtime 22:45, wake 06:00, switched off, tracking off |
| Stopwatch | Device | ✅ | — not yet — |
| Permissions | Security | ✅ | iphone11_ios17 — 289 rows, which is exactly the count iLEAPP records for the same image — two independent parsers agreeing. The distribution also justified passing unknowns through rather than guessing: alongside… |
| Timers | Device | ✅ | — not yet — |
| Apple Watch apps | Device | ✅ | iphone11_ios17 — 47 apps on one paired watch |
| Wi-Fi networks | Network | ✅ | iphone11_ios17 — 17 known networks, with join dates from July 2023 to January 2024 |
| Private Wi-Fi addresses | Network | ✅ | iphone11_ios17 — 17 networks with their private addresses, join times and rotation timestamps |
| World Clock | Device | ✅ | — not yet — |

**31 implemented · 24 verified · 7 awaiting a real backup.**

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
