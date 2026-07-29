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
| **Backup-reachable** | 311 | A backup contains it. The addressable universe. |
| **Encrypted-only** | 26 | Only in an encrypted backup. Reachable, conditionally. |
| **Excluded** | 42 | Under a domain root but on no include list, or explicitly excluded. |
| **Unclassified** | 219 | The iLEAPP glob is rootless (`*/NoteStore.sqlite*`) — no directory context to resolve. |

> **These numbers replace an earlier 355 / 84 / 159**, which came from a
> hand-written heuristic before Apple's rules were available. The heuristic
> over-counted reachable artifacts and mis-stated the reason for the exclusions.

The 219 unclassified are rootless globs, not unknown territory: iLEAPP writes
`**/interactionC.db*` because it searches a filesystem, whereas we resolve by
domain. Settling them means mapping each to its real directory —
[#192](https://github.com/PeterBlenessy/traceloupe/issues/192).

### Encrypted-backup-only — including things we already ship

`RelativePathsToOnlyBackupEncrypted` is not an edge case. It contains sources
TraceLoupe surfaces today:

| Path | What it is |
|---|---|
| `Library/Safari/SafariTabs.db` | Safari open tabs (iCloud tabs) |
| `Library/CoreDuet/People/interactionC.db` | the whole Interactions view |
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

The 42 excluded are out of reach whatever we decide: Biome, knowledgeC, unified
logs, sysdiagnose, `/var/db`, `/var/log`, `Library/Caches`, and app-container
`Library/Caches`/`tmp`.

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
| **Device Usage** | 2 | App snapshots, last-used dates |
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

## Known weaknesses of this audit

Stated so nobody mistakes it for more than it is:

1. **The 219 unclassified artifacts** are unresolved, so the destination's size
   is a range, not a number ([#192](https://github.com/PeterBlenessy/traceloupe/issues/192)).
2. **The domain rules are transcribed, and from iOS 16.4.**
   `ios-backup-domains.json` comes from
   [a third-party transcription](https://gist.github.com/leminlimez/c602c067349140fe979410ef69d39c28)
   of an iPhone SE 3, not a file we extracted. Apple **moved** it in iOS 17.0
   (to `MobileBackup.framework/Domains.plist`) and may have changed it.
   Authoritative for 16.4, strongly indicative for later; verifying against a
   real iOS 17+ copy is [#191](https://github.com/PeterBlenessy/traceloupe/issues/191).
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
