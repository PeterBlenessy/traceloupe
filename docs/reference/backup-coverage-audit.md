# Backup coverage audit

**What an iOS backup can hold, what TraceLoupe reads today, and what is left.**
This is the source-level companion to
[`app-data-coverage.md`](app-data-coverage.md) (field-level, within a source we
already parse) and [`app-support.md`](app-support.md) (per-app native status).

The work this audit sizes is charted on
[Map: iLEAPP-parity coverage of first-party backup data](https://github.com/PeterBlenessy/traceloupe/issues/189).

> **Method.** Numbers come from `tools/classify-ileapp-artifacts.py`, run against
> the pinned iLEAPP checkout (`pnpm setup:engine`). Re-run it rather than
> trusting the tables below — iLEAPP gains artifacts continuously, and a frozen
> table is wrong within weeks.

---

## The rule: what a backup can reach

An iOS backup stores files keyed by **domain** — HomeDomain (`/var/mobile`,
minus exclusions), MediaDomain and CameraRollDomain, `AppDomain*` /
`AppDomainGroup*` (an app's Documents + Library, minus `Library/Caches` and
`tmp`), KeychainDomain, WirelessDomain, and a few system domains. Everything
else needs a full-filesystem extraction (GrayKey/checkm8) and is **permanently**
out of reach for a tool that reads backups. That is not a gap to close; it is a
stated product non-goal — *"not an attempt to recover data that iOS never places
in a backup"*.

**Membership is a property of the domain, not of the path**, and the exclusions
are invisible in a glob. `Library/Biome/` and `Library/CoreDuet/Knowledge/` sit
under the same `mobile/Library` prefix as artifacts that *are* backed up — while
`Library/CoreDuet/People/interactionC.db` right beside them is backed up, and we
already parse it. So the classifier carries an explicit deny-list of known
exclusions and reports anything it cannot place as `unknown` rather than
guessing.

### Current split

| | Count | Meaning |
|---|---:|---|
| **Backup-reachable** | 355 | A backup can contain it. This is the addressable universe. |
| **Full-filesystem only** | 84 | No backup ever contains it. Out of scope, permanently. |
| **Unclassified** | 159 | The path alone cannot settle it — needs a real backup Manifest. |

The 159 are the reason the destination has no exact size yet: the tail is
somewhere between 355 and 514 artifacts. Resolving them is
[#192](https://github.com/PeterBlenessy/traceloupe/issues/192).

### Out of scope, permanently

Categories where nothing is backup-reachable: **App Conduit · Audi Trips ·
Browser · Burner Cache · CloudKit · Kijiji Conversations · KnowledgeC · Mobile
Installation Logs · Reddit**. The larger exclusion families are Biome (24
artifacts), unified logs (13), sysdiagnose, `/var/db`, `/var/log` and
`Library/Caches`.

Recorded here so they are not re-investigated. **KnowledgeC is the one worth
naming twice** — it is the artifact most often assumed present, and it is the
clearest example of a `mobile/Library` path that a backup does not carry.

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

1. **The 159 unclassified artifacts** are unresolved, so the destination's size
   is a range, not a number ([#192](https://github.com/PeterBlenessy/traceloupe/issues/192)).
2. **"iLEAPP has a module" is not "the data is there."** A module proves the
   artifact exists on *some* device. Whether a given backup contains rows is a
   separate question — `app-data-coverage.md` already records several stores
   (Maps, Podcasts, Journal, Wallet) that are present but empty on a real
   device.
3. **iLEAPP is not the whole universe either.** It is the best open catalogue of
   iOS artifacts, but an artifact with no iLEAPP module is not thereby absent —
   `app-support.md` records several such apps found by research alone.
4. **Category names are iLEAPP's**, and a few are misleading: the
   `Photos.sqlite-*` families are Apple first-party despite sorting oddly, and
   `Health & Fitness` holds third-party (AllTrails) rather than Apple Health.
