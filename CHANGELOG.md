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
| `0.12.0` | **Messages & media UX overhaul + hardening** — inline/hover link previews (OpenGraph, single 3-way mode), an in-app image/video lightbox, opt-in recovery of missing attachments from the camera roll, Notes rich-text rendering, persisted scroll/sidebar/window state, and a pre-release review pass that closed a DNS-rebind SSRF hole in link fetching. See "Planned" below for what's next. |
| `0.13.0` | **Adaptive "islands" toolbar** — every view's filters, sort, and search become segmented, bordered islands in one unified top bar that shares width evenly, reveals more chips as room allows, collapses to representative icons when tight, and expands one island on demand while the others fold. Rolled out across all thirteen views. |
| `0.14.0` | **Filter · Sort · Search paradigm + window chrome** — the islands give way to one **Filter** button that morphs into a grouped, described filter panel (single- or multi-select), with sort and an expanding search as standalone controls. A full-width HTML title bar that yields the full height to the sidebar when it's open, the sidebar top merged with the Device view, and Settings moved to the sidebar footer. |
| `0.15.0` | **Data-coverage pass** — surfacing fields already parsed but unshown: all of a note's embedded images (detail gallery), iMessage group-action system rows, Safari reading-list read/last-viewed. |
| `0.16.0` | **Health deep-dive + contact relationships** — the HealthKit store beyond workouts: daily activity aggregates (steps/distance/energy/flights/heart-rate), sleep sessions, and workout GPS routes with inline previews; plus contact related-names and address-book groups. |
| `0.17.0` | **Health rings, mobility & the timezone timeline** — Apple activity rings vs goals, walking/audio-exposure daily metrics, and a per-timezone travel timeline from sample provenances; the Health view's sections refactored onto one descriptor-driven pipeline. |
| `0.18.0` | **Data-coverage pass, recovery-themed** — Recently-Deleted photos (`ZTRASHEDSTATE`) and messages (`chat_recoverable_message_join`), per-app Interaction channels, Messages sticker/effect/app-bubble classification, Health Cycle-Tracking + Awards, Contacts social profiles, and App Store metadata. |
| `0.19.0` | **Data-coverage close-out** — Safari local open tabs (`BrowserState.db`, with a private-browsing badge), Calls number-country flags, and Photos added-to-library date. Field-level coverage is now closed; remaining gaps are marked won't-implement. |
| `0.20.0` | **Security Check (M1)** — a spyware/stalkerware indicator scan (MVT-style) over the imported backup: STIX2 + Echap indicator feeds bundled and refreshable, a native Rust scan engine across messages/Safari/apps/contacts/notes/calendar/interactions + a Manifest file sweep, Explicit Scans and a consent-gated Passive Check at import, a Security view with severity-graded findings and CSV export. |
| `0.21.0` | **Security Check M2 — process-name detection** — the first Tier-B artifact surface: DataUsage.sqlite and OSAnalytics ADDaily process activity scanned against process-name indicators (the class that first exposed Pegasus), extracted on demand during an Explicit Scan. |
| `0.22.0` | **Security Check M2 — configuration profiles** — installed profiles (ProfileTruth + PayloadManifest) surfaced for review: indicator matches on profile hosts/names, plus structural flags for hidden profiles (Warning) and device-management profiles (Info). The classic stalkerware install vector. |
| `0.23.0` | **Security Check M2 — TCC permissions** — TCC.db grants cross-checked against stalkerware bundle IDs: a known monitoring app holding microphone/camera/location access is surfaced with the exact permissions it holds, corroborating a bundle-id match with real capability evidence. |
| `0.24.0` | **Security Check M2 — Shortcuts** — Shortcuts.sqlite actions scanned for indicator URLs/hosts: a shortcut that quietly calls out to a malicious endpoint (exfiltration/automation) is flagged. Tier-B scan inputs refactored into one `ScanInputs` struct. |
| `0.25.0` | **Security Check M2 complete — WebKit** — domains each app's webview contacted (per-app `observations.db`) scanned against indicators: a webview loading a known C2 domain is flagged with the apps that saw it. Completes the M2 Tier-B surfaces. |
| `0.26.0` | **Security Check M3 — custom indicators** — point the scan at a local folder of `.stix`/`.stix2`/`.yaml` files, merged with the bundled feeds (researcher mode, parity with iMazing). |
| `0.27.0` | **Security Check M3 — scan-history diffing** — findings new since the previous scan are flagged with a **NEW** badge and a "N new since last scan" count, so a re-scan after an indicator update highlights what changed. |
| `0.28.0` | **Security Check M3 complete — opt-in de-shortener** — reveal where a shortened link points, as a deliberate per-link action gated by a per-use approval prompt (with a per-backup, never global, "don't ask again"). Only allowlisted shorteners are contacted, and the destination is revealed without visiting it. |

> The single source of truth for the version is `package.json`; keep the
> workspace `Cargo.toml` and `src-tauri/tauri.conf.json` in step when it changes.

## [Unreleased]

_Nothing yet._

## [0.28.0] — 2026-07-21

**Security Check M3 complete — opt-in de-shortener.** Reveal where a shortened
link in a finding points. Resolving a link contacts a remote host with a URL
from the backup — the sole sanctioned exception to "nothing leaves the machine"
(ADR 0001) — so it is strictly opt-in and deliberate.

- **Per-link, user-approved:** never automatic and never during a Passive Check.
  A "Reveal destination" button on a finding's shortened links opens an approval
  dialog that names the real risk (resolving can signal that the device is being
  examined). Every use prompts by default.
- **Per-backup opt-out (not global):** the dialog carries a "don't ask again for
  this backup" switch, stored in that backup's own cache — it never applies to
  other backups, resets on re-import, and clears when the backup is forgotten.
- **Safe resolution:** only known-shortener hosts (an allowlist) are ever
  contacted; the destination is read from the redirect `Location` **without
  visiting it**, so the final target is never contacted. SSRF-guarded by the same
  public-only DNS resolver as link previews (private/loopback/metadata addresses
  refused, rebind-proof).
- New `shorteners` core module; `expand_short_url` / `find_shortener_urls` /
  `deshorten_auto_approve_get`/`set` commands.

With the de-shortener, **M3 — and the Security Check as a whole — is complete.**

## [0.27.1] — 2026-07-21

**Sidebar: Security grouped with Device.** The Security entry moves up next to
Device — both are whole-backup operations (its identity, and an audit of it),
distinct from the content views — with a separator dividing that pair from the
content list. Gives the security feature fitting prominence.

## [0.27.0] — 2026-07-21

**Security Check M3 — scan-history diffing.** A re-scan (e.g. after updating
indicators) now shows what's new.

- `query::list_findings` computes an `is_new` flag per finding by diffing against
  the previous completed scan of the same backup (matching on module + matched
  value + source artifact); `previous_completed_run` finds the baseline. The
  first scan has no baseline, so nothing is marked new.
- **Security view:** findings new since the last scan carry a **NEW** badge, and
  the results header shows a "N new since last scan" count.

## [0.26.0] — 2026-07-21

**Security Check M3 — custom indicators.** Researchers can point a scan at their
own indicator files, merged with the bundled feeds.

- **New loaders** (`indicators::load_custom_dir`, `load_indicators`,
  `IndicatorSet::merged_with`): a folder is scanned by extension —
  `.stix`/`.stix2`/`.json` as STIX2, `.yaml`/`.yml` as Echap YAML — with no
  manifest required; a malformed file is reported and skipped, a missing folder
  degrades to empty. Custom indicators are re-deduplicated against the snapshot.
- **Setting** `custom_indicator_dir` on `DetectionSettings`, applied to every
  scan (Explicit, Passive) and reflected in the indicator-feed counts.
- **Security view:** a "Custom indicators" row with a folder picker and Clear.

## [0.25.0] — 2026-07-21

**Security Check M2 complete — WebKit resource-load statistics.** Adds the last
Tier-B surface: the domains an app's in-app browser (WebKit) contacted.

- **New parsers** (`analyzer::parse_webkit_observations`,
  `parse_webkit_session_log`): read each app's
  `Library/WebKit/WebsiteData/ResourceLoadStatistics/observations.db`
  (`ObservedDomains.registrableDomain`) and the older
  `full_browsing_session_resourceLog.plist` (`browsingStatistics` origins).
- **New `webkit` scan module:** aggregates observed domains across all apps and
  matches them against domain/URL indicators; a matched domain is surfaced once,
  naming the apps whose webviews contacted it — evidence of in-app spyware C2 or
  exfiltration traffic.
- **On-demand extraction** during an Explicit Scan: every per-app
  `observations.db` is located via the Manifest index and parsed. Passive Check
  unaffected.
- Validated against the real dev backup: 2,692 observed domains extracted across
  34 apps, zero indicator matches (clean). See `docs/security-check-validation.md`.

With WebKit, every MVT iOS module that matches an indicator class our feeds
carry is now covered by a shipped module — **M2 Tier-B is complete.**

## [0.24.0] — 2026-07-21

**Security Check M2 — Shortcuts.** Shortcuts can call out to arbitrary URLs; a
shortcut quietly posting to a malicious endpoint is an exfiltration/automation
vector.

- **New parser** (`analyzer::parse_shortcuts`): reads `Shortcuts.sqlite`
  (HomeDomain, `Library/Shortcuts/Shortcuts.sqlite`) — each
  `ZSHORTCUTACTIONS.ZDATA` is a binary plist of workflow actions whose string
  parameters (e.g. an `openurl` action's `WFInput`) carry the URLs, matched
  against domain/URL indicators.
- **New `shortcuts` scan module:** matches each shortcut's referenced hosts/URLs
  against indicators (feed-graded), on-demand during an Explicit Scan.
- **Refactor:** `run_scan`'s four Tier-B inputs (manifest, processes, profiles,
  grants — now plus shortcuts) are grouped into one `ScanInputs` struct,
  replacing a growing positional-argument list.
- Validated against the real dev backup: 46 shortcuts extracted (44 reference a
  host), zero indicator matches (clean). See `docs/security-check-validation.md`.

## [0.23.0] — 2026-07-21

**Security Check M2 — TCC permissions.** Cross-checks which apps hold sensitive
permissions against the stalkerware bundle-id lists.

- **New parser** (`analyzer::parse_tcc`): reads granted rows from `TCC.db`
  (HomeDomain, `Library/TCC/TCC.db`) — `auth_value` 2/3 (or the legacy `allowed`
  column), mapping each `kTCCService*` to a friendly name and a
  surveillance-relevant flag (microphone, camera, screen, photos, contacts,
  location, speech, motion).
- **New `tcc` scan module:** aggregates grants per app; a client that matches a
  stalkerware/watchware bundle-id indicator is surfaced as one feed-graded
  finding listing the sensitive permissions it holds ("holds Camera, Microphone
  access") — turning a bare bundle-id match into concrete capability evidence.
  A normal app holding camera access is not flagged; only a *known monitoring
  app* holding it is.
- **On-demand extraction** during an Explicit Scan via the Manifest index,
  best-effort. Passive Check unaffected.
- Validated against the real dev backup: 116 grants across 67 apps extracted,
  zero stalkerware matches (clean); the positive path is covered by unit tests.
  See `docs/security-check-validation.md`.

## [0.22.0] — 2026-07-20

**Security Check M2 — configuration profiles.** Surfaces installed configuration
profiles, the classic stalkerware install vector (an unexpected or hidden
profile can grant broad control over the device).

- **New parser** (`analyzer::parse_configuration_profiles`): reads
  `ProfileTruth.plist` (the authoritative installed-profile list, keyed by
  `Name from Org (UUID)`) and `PayloadManifest.plist` (the `HiddenProfiles`
  set), extracting each profile's name, organization, UUID, referenced hosts,
  device-management capabilities, and hidden flag.
- **New `profiles` scan module:** matches profile hosts/names/orgs against
  indicators (feed-graded), and adds one structural review finding per profile —
  **Warning** for a hidden profile (invisible in Settings), **Info** for a
  device-management profile (MDM/proxy/VPN/content-filter), else a plain
  "review if unexpected" **Info**.
- **On-demand extraction** during an Explicit Scan via the Manifest index
  (SysSharedContainer configuration-profiles domain), best-effort. Passive Check
  unaffected.
- Validated against the real dev backup: the one installed profile (a legitimate
  university printer profile) is parsed correctly and surfaced as a single Info
  review finding — no false alarm. See `docs/security-check-validation.md`.

## [0.21.0] — 2026-07-20

**Security Check M2 — process-name detection (first Tier-B surface).** Adds the
artifact class that originally exposed Pegasus: process activity, matched
against process-name indicators.

- **New parsers** (`analyzer::parse_datausage`, `parse_addaily`): DataUsage.sqlite
  `ZPROCESS` (process name, bundle name, Mac-absolute timestamp → Unix) and the
  OSAnalytics `com.apple.osanalytics.addaily.plist` `netUsageBaseline` dictionary
  (keyed by process name).
- **New `process_names` scan module:** matches each observed process name (and
  its basename) against process-name indicators, and DataUsage bundle names
  against bundle-id indicators, graded Critical.
- **On-demand extraction:** an Explicit Scan locates and extracts both files via
  the Manifest index (WirelessDomain / HomeDomain), best-effort — a missing or
  unreadable file just yields fewer processes, never fails the scan. The Passive
  Check stays apps-only.
- Validated against the real dev backup: 1,825 processes extracted, no mercenary
  process-name matches; the bundle-name cross-check independently re-surfaced the
  known Kaspersky Safe Kids watchware (Info) found in M1. See
  `docs/security-check-validation.md`.

## [0.20.0] — 2026-07-20

**Security Check (M1).** A local scan that checks an imported iPhone backup for
indicators of compromise from known mercenary spyware (Pegasus, Predator,
KingsPawn, Operation Triangulation, NoviSpy, Wintego, EagleMsgSpy, Candiru,
Coruna, DarkSword) and commercial stalkerware/watchware — modeled on iMazing's
Spyware Analyzer and Amnesty International's MVT methodology, implemented
natively in Rust (no MVT code; only its CC-BY indicator data and the public
Echap stalkerware feed).

- **Indicator feeds.** STIX2 bundles + Echap `ioc.yaml`/`watchware.yaml`
  normalized into one indicator set (domains, URLs, emails, process names,
  file names/paths, bundle IDs, cert hashes, IPs), with evidence-graded
  severity. A snapshot of ~5,800 indicators is bundled for offline use and can
  be refreshed over HTTPS from the public feed repos.
- **Scan engine.** Evaluates the set against the cache (messages, attachments,
  Safari history/bookmarks, notes, calendar, contacts, interactions, installed
  apps) plus a `Manifest.db` file-name/path/app-domain sweep. Conservative host
  tokenizer and exact-or-subdomain matching to limit false positives. Full scan
  of a real backup completes in well under a second.
- **Two modes.** User-initiated **Explicit Scan** (all modules, optional fresh
  feed fetch) and a consent-gated **Passive Check** at import (apps-only by
  default, configurable). Both governed by persisted detection settings with
  one-time consent dialogs.
- **Security view.** Severity-graded findings table with per-finding detail and
  deep links into the source artifact, a persistent false-positive/clean-≠-safe
  disclaimer, a stalkerware-victim safety panel, indicator-freshness display,
  and CSV report export with CC-BY attribution.
- **Privacy (ADR 0001).** Backup-derived data never leaves the machine; feed
  fetches are disclosed, setting-governed, send-nothing operational traffic.

Validated against `mvt-ios check-backup` (see `docs/security-check-validation.md`):
every indicator class MVT matches from these feeds is covered by a shipped
module; the rest maps to the named M2 (Tier B) scope.

## [0.19.0] — 2026-07-20

**Data-coverage close-out.** The last field-level items, and a line drawn under
the coverage effort (see `docs/app-data-coverage.md`). Requires a re-import to
populate for existing caches.

### Added

- **Safari — local open tabs.** The Safari "Tabs" list now comes from
  `BrowserState.db` (this device's actual open tabs, 201 here) instead of the
  thinner iCloud-synced `SafariTabs.db` (44), and each tab shows its last-viewed
  time and a **Private** badge for private-browsing tabs (schema v46).
- **Calls — number country.** A call number's ISO country
  (`ZISO_COUNTRY_CODE`) shows as a flag on the call row (schema v45), so
  international calls stand out at a glance.
- **Photos — added-to-library date.** When a camera-roll asset was added to the
  library (`ZASSET.ZADDEDDATE`) is surfaced in the lightbox as "Added &lt;date&gt;"
  whenever it differs from the capture date by more than a day — flagging media
  that was received, saved, or imported rather than shot on this device (schema
  v44). ~1,174 such assets in the reference backup.

## [0.18.0] — 2026-07-20

**Data-coverage pass.** More fields already present in a backup, now surfaced —
with a forensic "recover what was deleted or hidden" throughline (Recently
Deleted photos and messages). Requires a re-import to populate for existing
caches.

### Added

- **Interactions — per-app channels.** A "Channels" summary strip above the
  interaction list shows which apps CoreDuet interactions flowed through
  (Messages, Phone, FaceTime, Snapchat, Gmail, …) with in/out totals, read from
  the raw `ZINTERACTIONS` table (the person-level `ZCONTACTS` graph has no app
  dimension). Bundle ids sharing a name merge; zero-total channels drop.
- **Health — Cycle Tracking.** A menstrual-flow + symptoms section from the
  HealthKit category samples.
- **Health — Awards.** Earned Apple Fitness achievements.
- **Contacts — social / IM profiles.** AddressBook property 46 (Twitter,
  Instagram, and other service handles).
- **Apps — App Store metadata.** Name, seller, version, genre, and release year
  parsed from each app's `iTunesMetadata` bplist.
- **Messages — sticker classification.** Sticker attachments
  (`attachment.is_sticker`) are now classified as their own content kind,
  lighting up the Stickers content-filter (which previously never matched).
- **Messages — Recently Deleted.** Deleted-but-recoverable iMessages
  (`chat_recoverable_message_join`, never read before) are recovered and shown
  with a red "Deleted &lt;date&gt;" badge — messages that had vanished from the
  conversation reappear, forensic.
- **Messages — expressive send effects.** "Sent with Confetti / Slam / Invisible
  Ink / …" from `expressive_send_style_id`.
- **Messages — app-bubble messages.** iMessage-app bubbles (`balloon_bundle_id`)
  surfaced as a distinct message kind instead of empty rows.
- **Photos — Live Photo & burst badges.** Distinguishes Live Photos
  (`ZPLAYBACKSTYLE`) and burst stacks (`ZAVALANCHEUUID`).
- **Photos — Recently Deleted.** Trashed camera-roll assets (`ZTRASHEDSTATE`)
  are now surfaced with a red trash badge and a lightbox indicator instead of
  being dropped at ingest — forensic, matching the Hidden-album treatment.
- **Notes — image filter.** Filter for notes that carry embedded images.
- **Device — toolbar.** Close the backup, re-import, or open another backup
  without leaving the view.

### Fixed

- **Photos — panorama mislabel.** `ZKINDSUBTYPE == 2` is a Live Photo
  still-frame, not a panorama (which is `== 1`); panoramas are now counted
  correctly.
- **Notes — honest image availability.** Notes whose images were offloaded to
  iCloud no longer pretend to display them; they state the images aren't in the
  backup instead.

## [0.17.0] — 2026-07-20

**Health rings, mobility metrics & the timezone timeline.** Requires a
re-import to populate for existing caches.

### Added

- **Activity rings** — `activity_caches` (the pre-aggregated Move/Exercise/
  Stand rings with their goals) becomes an `activity_rings` table (schema
  v34); daily rows show goal progress ("Move 412/500 kcal"). Rings a device
  never tracked stay blank (phone-only stores have Move only).
- **Mobility + audio-exposure metrics** — five more quantity types in the
  daily aggregation: headphone audio exposure (dB, loudest sample), walking
  speed, step length, double-support and walking-asymmetry fractions. Spread
  metrics merge min/avg/max across sources instead of summing.
- **Timezone timeline** — every sample's recording timezone
  (`data_provenances.tz_name`) aggregated per zone and device model (schema
  v35) into a new **Timezones** section: zone, sample count, devices,
  first–last span. A travel history hiding in the Health store.

### Changed

- **Health view sections are descriptor-driven** — the per-section machinery
  (sort state, windowing, counts, empty states, rendering) collapsed into one
  pipeline; workouts share the same virtualized list as the other sections.
  Adding the Timezones section was one descriptor entry. (Review finding.)
- The `health_daily` metric names are shared constants between parser and
  query — a rename can no longer silently drop a metric. (Review finding.)
- The device-model → marketing-name mapping moved to a shared module (used
  by Device and the timezone timeline) and covers more models.

## [0.16.1] — 2026-07-20

**Review-fix pass over 0.16.0** (multi-agent code review; 10 findings, 8 fixed).

### Fixed

- **Health daily aggregates no longer double-count multi-device days** — samples
  are aggregated per source (`data_provenances.source_id`); cumulative metrics
  keep the largest source's daily total, heart rate merges across sources.
  Requires a re-import to correct existing caches.
- The Sleep section's sort control reflected stale state after changing the
  sort field; daily rows now window against local-midnight preset bounds
  (was off by one day at range edges outside UTC); the Workouts section no
  longer claims "No health data" when daily/sleep data exists.
- Contact groups ignore non-person `ABGroupMembers` rows (`member_type != 0`),
  which could mis-tag a contact via a ROWID collision.
- The daily/sleep lists are fetched only when their section is visible.

## [0.16.0] — 2026-07-19

**Health deep-dive + contact relationships.** Requires a re-import to populate
for existing caches (the migrations create the structures; the parsers fill
them).

### Added

- **Health: daily activity** — the raw HealthKit quantity samples (steps,
  distance, resting/active energy, flights, heart rate) are aggregated per UTC
  day into a new `health_daily` cache table (schema v30) and shown in a new
  **Daily activity** section: one row per day with totals and the heart-rate
  min/avg/max (canonical count/sec scaled to bpm), time-filterable and sortable
  by date/steps/distance. 2,742 days in the reference backup.
- **Health: sleep sessions** — sleep-analysis category samples (data type 63)
  become a `sleep_sessions` table (schema v31) with friendly stage names
  (In Bed / Asleep / Awake / Core / Deep / REM) and a **Sleep** section
  (start–end, duration, date/duration sort).
- **Health: workout GPS routes** — each workout's location series
  (`associations` → `data_series` → `location_series_data`, tombstoned links
  skipped) is stored downsampled to ≤1,000 points (schema v32). Workout rows
  with a route expand to an inline SVG route preview (equirectangular
  projection, start/end markers, altitude-range caption). The Health view now
  has a **Section** filter (Workouts | Daily activity | Sleep).
- **Contacts: relationships + groups** — related names (`ABMultiValue`
  property 23; label = relationship, iOS magic labels cleaned, custom labels
  kept) and address-book group memberships (`ABGroup` ⋈ `ABGroupMembers`) are
  parsed into `related_json`/`groups_json` (schema v33) and shown as a
  **Related** field group and **Groups** chips in the contact detail.

## [0.15.0] — 2026-07-19

**Data-coverage pass — surfacing fields already in the backup.** Requires a
re-import of the affected module to populate for existing caches (the migrations
create the structures; the parsers fill them).

### Added

- **Notes: all embedded images** — the detail pane now shows every image in a
  note (gallery), not just the list thumbnail. New `note_media` cache table
  (schema v29) holds each resolved image with its on-demand decrypt fields; the
  note-image protocol takes an optional index. True inline-at-position rendering
  remains future work.
- **Messages: group actions** — iMessage system rows (rename, add/remove
  participant, leave, group-photo change; `item_type` 1–4) were dropped because
  they carry no text/attachment. They're now surfaced as centered `system`
  messages ("‹actor› ‹action›"). ~370 in the reference backup.
- **Safari: reading list** — reading-list rows show their last-viewed date
  ("Read ‹date›") or an "Unread" badge.

### Fixed

- The Messages parser's new columns are schema-guarded, so older `sms.db`
  versions (and the test fixtures) are unaffected.

## [0.14.0] — 2026-07-19

**The "islands" toolbar (0.13.0) becomes a Filter · Sort · Search paradigm.** One
funnel **Filter** button morphs open into a grouped panel; sort and search stand
on their own. Plus a state-aware title bar and a reorganised sidebar.

### Added

- **Filter panel** (`FilterControl`): the funnel button *morphs* into the panel —
  its width/height/radius animate from the button's footprint to the full panel
  (the NoteSage command-bar technique; width/height animate reliably in the macOS
  webview, unlike the `scale`/`translate` properties). Each facet is a **labelled,
  described group**; facets the backup lacks are omitted, empty time presets are
  disabled. Selected filters show as **removable chips** on the closed island that
  collapse-animate away on removal. **Multi-select** where it fits (e.g. Notes
  tags); single-select for Time and single-choice facets.
- **Standalone Sort** (a bordered island) and an **expanding Search** box whose
  width animates open.
- **State-aware title bar**: when the sidebar is **collapsed** the title bar spans
  the full window width above the icon rail; when **expanded** the sidebar runs
  full height and the bar only covers the content area to its right. Traffic
  lights vertically centred; the sidebar toggle is its own island.
- **Sidebar reorg**: the top entry merges with the **Device** view (shows the
  open device's name); **Settings** moved to the sidebar footer.

### Changed

- **All views** migrated off the islands engine to the new paradigm; the adaptive
  islands fit-engine (`toolbar-islands`, the measurement logic in
  `AdaptiveToolbar`) and the now-unused `PanelHeader` were removed.
- Filter chips use discrete borders (no louder than the island border).

### Fixed

- WebKit wouldn't animate the individual `scale`/`translate` CSS properties, so
  all toolbar animations use `transform`/`width`/`height` instead.
- The dev window config now applies the merged-titlebar settings (it previously
  replaced the base window, losing `titleBarStyle`/`hiddenTitle`).
- A review pass hardened `FilterControl`: guards a toggle-based multi-select clear
  against double-firing, cancels pending timers on unmount, adds focus
  management, and re-measures the panel with a `ResizeObserver`.

## [0.13.0] — 2026-07-19

**Adaptive "islands" toolbar.** Every view's controls now live in one unified
top bar as segmented, bordered **islands** of related actions — a facet, the
time range, sort, an expanding search — verified across all views with the
screenshot harness.

### Added

- **Adaptive islands toolbar** (`AdaptiveToolbar` + `toolbar-context` +
  `toolbar-islands`). Islands share the available width evenly so none
  dominates; each reveals as many chips as fit and tucks the rest behind a
  single expand chevron. Clicking a chevron **expands** that island and collapses
  the others — to their two-item minimum where there's room, otherwise to a
  single representative icon. A greedy fit reveals more chips into any slack and
  only collapses the widest island to an icon when the minimums don't fit, so
  nothing overflows and the app-wide controls never clip.
- **Data-aware time island** — presets whose range holds no items fold away
  first, so the visible chips are the ranges that actually contain data.
- **Expanding search island** — a magnifier that opens into an inline input,
  bordered like the other collapsed islands.

### Changed

- **All thirteen views migrated** to publish their filters/sort/search as islands
  via `useViewToolbar`, dropping their in-panel `PanelHeader`/toolbar rows:
  Safari, Photos, Calls, Interactions, Calendar, Reminders, Health, Apps,
  Contacts, Notes, Recordings, Messages, and Device. Master-detail views
  (Contacts, Notes, Recordings, Messages) keep their detail sub-headers; Messages
  lifts its top-level view (Chats/Timeline) and app-service controls into the
  unified bar while its per-pane order/jump/kind controls stay contextual.

## [0.12.2] — 2026-07-18

Toolbar polish follow-ups (verified against Apple's apps via the native-window
screenshot harness).

### Changed

- **Custom date-range button rejoined the time-preset chips** — it was being
  stretched over next to the sort control; now `All / 24h / 7d / 30d / <year> /
  Range` read as one time-filter unit, with sort on its own at the right.
- **Header filter chips no longer collapse into "⋮" when there's room** — the
  source/type filter (Safari's History/Bookmarks/Reading List/Tabs, Photos'
  source chips) now claims the free header width and shows every chip that fits,
  overflowing only when the row is genuinely narrow.

### Added

- Onboarding polish: the primary **"Read & open" / "Open"** actions are real
  buttons; the encrypted-backup password step shows a **Keychain trust line**;
  and a wrong password keeps the field with an inline "That password didn't work"
  instead of a dead-end error card.

## [0.12.1] — 2026-07-18

A UX consistency-and-polish pass, verified against Apple's apps via a new
screenshot harness (headless Chromium + real-window `screencapture`).

### Added

- **Native macOS title bar** — the window now uses `titleBarStyle: Overlay` with
  a hidden title and positioned traffic lights, so the app content runs to the
  top as one unified bar instead of a native titlebar stacked over a separate app
  bar. The sidebar reads as its own panel beneath the lights.
- **Notes-style toolbar button group** — the top-bar controls (sidebar toggle,
  density, theme, settings) are grouped into one subtly bordered segmented
  cluster (`ToolbarGroup`).
- **Dynamic, minimal view toolbars** — a macOS-style **expanding search** (an
  icon that opens into an inline input, with a clear button) replaces the
  full-width search row, and a **Filter toggle** reveals the filter/sort row on
  demand (state persisted per view). Three header rows collapse to one.
- **Reduced-motion** support (`prefers-reduced-motion`).

### Changed

- **Density now applies to every list view** — Calendar/Reminders/Health/
  Interactions rows moved onto the shared `Item`/`list-row` slots, so the
  Comfortable/Cozy/Compact setting affects them (and they share one borderless
  row style instead of bordered "cards").
- **One filter-chip language** — badge filters and time-preset chips share a
  single tinted-pill treatment (`filterPillClass`).
- **One empty & loading system** — zero-result states use the richer `EmptyView`
  (icon + title) and loading uses a single shared `ListSkeleton` everywhere.
- All date/number formatting routes through `lib/format.ts` (fixes a Calendar
  12/24-hour bug; Safari visit counts get thousands separators).

### Removed

- The dead "Parsing engine needed" card (leaked iLEAPP / `pnpm setup:engine`
  developer text) and the unused `engine-setup`/`placeholder` views.

## [0.12.0] — 2026-07-18

A large Messages- and media-focused UX release, capped by a pre-release code
review that hardened the new link-fetching and scroll-persistence code.

### Added

- **Link previews for URLs in messages.** OpenGraph "unfurl" cards behind a
  single **3-way setting — Off · Hover · Inline**: *Inline* renders the card in
  the bubble (iMessage-style, replacing the raw URL when the message is only a
  link); *Hover* shows it in a popover. Every link in a message is unfurled (up
  to a cap), in both the conversation and the Timeline. Rich links from iMessage
  plugin payloads (e.g. Apple Maps) are decoded offline from the typedstream;
  TikTok uses its oEmbed endpoint (it serves no OpenGraph to bots). Preview
  images are proxied to `data:` URLs so the webview never contacts the host, and
  a crawler-style User-Agent is used (a browser UA regresses Spotify/Instagram).
- **In-app image & video viewer.** Message images/videos open in a shared,
  full-viewport lightbox with selectable styles, an opaque metadata overlay, and
  a dedicated **Media** settings tab. Videos show a first-frame poster instead of
  a black rectangle.
- **Recover missing attachments from the camera roll** (opt-in). When a
  message's image/video isn't in the backup, TraceLoupe can match it to a
  `Photos.sqlite` asset and display it, badged as *recovered*; the Timeline flags
  genuinely-missing attachments with a "not in backup" note.
- **Notes rich text** — formatting, lists, and checklists are now *rendered*
  (not just counted), first-image thumbnails appear in the Notes list, a
  hashtag-tag filter (iOS 15+) is available, and a flat/folder-tree view toggle.
- **Contact-aware Timeline avatars** — hover shows the contact; clicking opens
  them in Contacts. Added year quick-filters, jump-to-top/bottom, and the year in
  row times.
- **Persisted UI state** — Timeline & conversation scroll position (index-based),
  the Timeline time range, message time-order toggles, sidebar open/closed state,
  and window size/position all survive navigation and app restarts.
- **Overflow "⋮" menus** for the time-range and badge filters, so filters never
  wrap or push the header taller; jump-to-top/bottom added to conversations too.

### Changed

- Settings rows stack their (now full-width) description below the label + control.
- Timeline direction arrows read relative to the shown party, and outgoing rows
  resolve the contact (fixing the "#" placeholder avatar).
- Toolbar layout unified — time range on the left, facets + sort on the right;
  the new Calendar/Reminders/Health/Interactions views gained filters and
  surfaced metadata; list content left-aligned.

### Fixed

- **WAL-mode databases dropped data** (Safari history came up empty) — each DB's
  `-wal` sidecar is now replayed so unflushed rows are read.
- **Encrypted media no longer 404s** in a fresh session — the custom-scheme
  protocol handlers lazily reload the backup keys, and a *cancelled* Touch ID
  unlock no longer re-prompts once per media item (a photo grid could storm).
- Media no longer vanishes when switching views (per-mount cache key); opening an
  attachment no longer launches TextEdit on binary garbage.
- Jump-to-message and scroll restore reworked to be index-based and reliable —
  wait for the row count, re-issue across frames, and let an explicit jump win
  over position restore.

### Security

- **Closed a DNS-rebind SSRF in link-preview fetching.** URLs come from
  third-party messages in a backup (potentially of a compromised phone), i.e.
  attacker-controllable input, and the static private-host pre-check was
  bypassable by rebinding the domain between the check and ureq's connect. Fetches
  now pin the vetted address via a resolver that yields only globally-routable
  IPs, re-checked on every redirect hop and failing closed. Also folded in
  earlier link-preview/locked-note review hardening.

## [0.11.4] — 2026-07-16

A review of the Tauri media-serving/backend layer. No security hole (path
traversal is closed, the frontend can only ever supply a numeric id, secrets stay
out of logs); these fix the resource and secret-at-rest items it surfaced.

### Fixed

- **Scrubbing an encrypted video/audio no longer re-decrypts the whole file per
  request** — Range requests reused to decrypt the entire attachment into memory
  and a fresh temp on every seek (an OOM/disk-thrash path on a large video). The
  plaintext is now decrypted once to a temp cached by id (unique-temp + atomic
  rename, so concurrent requests can't read a half-written file) and reused across
  seeks.
- **Concurrent thumbnail renders can't serve a half-written JPEG** — `sips` now
  writes to a unique temp and atomically renames into the cache (owner-only before
  it's visible), fixing a race between two requests for the same image.
- **Decrypted-plaintext temps are cleared when a backup is closed or switched**,
  not only on forget — full-plaintext originals and externally-opened attachments
  no longer linger past the session.
- **Forgetting or switching a backup can't race an in-flight import** — both now
  take the import lock before touching cache files.

### Security

- The backup password is now held in zeroized buffers, wiped from memory on drop.

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

## [0.6.1] — 2026-07-15

A review-and-hardening point release after LinkedIn (0.6.0).

### Changed

- **Faster imports** — iLEAPP no longer re-parses first-party data the native
  parsers already read.
- **Settings dialog split into tabs**, instead of one overloaded pane.

### Fixed

- The **"Extract" action is gated** for apps already parsed natively, and app /
  service rows show brand icons.
- Message-attachment images render correctly when the attachment has a **NULL
  mime type** or comes from an **encrypted backup**.

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

## Planned

The **native-first migration is complete** — every surfaced artifact, first- and
third-party, is parsed by an in-house Rust parser, and iLEAPP is no longer run at
all (kept only as a development-time schema reference; the sidecar path is
dormant). The earlier "make iLEAPP optional, in batches" plan has therefore been
fully delivered and superseded; the remaining backlog is about *depth*, not
removing iLEAPP. Tracked in detail in [`docs/app-data-coverage.md`](docs/app-data-coverage.md)
(field-level) and [`docs/app-support.md`](docs/app-support.md) (per-app).

- **Field-level coverage gaps** — the highest-value unsurfaced fields: Messages
  full per-edit history (`message_summary_info`) and group-action rows; Notes
  inline image/drawing rendering; Photos Live/burst grouping; the Contacts
  relationship graph and groups.
- **Untapped stores** — Keychain (presence + counts only, never values), the
  Apps-view install metadata (version / install date / seller), and Health raw
  samples + GPS routes (only workouts are surfaced today).
- **More third-party apps** — the ⬜ Planned tiers in `app-support.md` (YouTube,
  Gmail, WeChat, Discord, Reddit, Spotify, …), plus two that need a real backup to
  pin their schema (Snapchat, X/Twitter). A single generic **`Cache.db`** module
  could surface cached network content across many apps at once — a strong future
  addition.
- **App-chat attachment media** — the framework has landed; individual parsers
  must still *emit* their attachments (WhatsApp/Kik/Threema/TikTok media),
  deferred until a backup containing that media exists to validate against.
- **Validation debt** — several app parsers (Instagram, Telegram, Kik, imo,
  Threema, Viber, Teams, LinkedIn) are marked *unvalidated* pending a real backup
  with those apps installed.

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
