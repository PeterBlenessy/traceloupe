# Security Check

**Product Requirements Document** *(internal codename: spyware-analyzer)*

*Status: Draft — core design decisions resolved 2026-07-20 (see `docs/CONTEXT.md` and `docs/adr/0001`) · Target: post-0.2.0 feature · Platform: macOS (TraceLoupe, Tauri v2)*

---

## 1. Executive summary

Add a **Security Check** to TraceLoupe (user-facing name; sidebar entry "Security"): detection that checks an already-imported iPhone backup for indicators of compromise (IOCs) from known mercenary spyware (Pegasus, Predator, KingsPawn, Operation Triangulation, NoviSpy, …) and from commercial stalkerware/watchware ("parental control" apps repurposed for covert surveillance).

The feature is modeled on iMazing's free Spyware Analyzer (<https://imazing.com/spyware-analyzer>), which wraps Amnesty International's open-source **Mobile Verification Toolkit (MVT)** methodology: download community-maintained **STIX2 indicator files**, scan the artifacts of an iTunes-style backup for matching domains, links, email addresses, process names, file paths, and bundle IDs, and present the matches with severity and context.

TraceLoupe is unusually well positioned to offer this: we already decrypt backups natively, already parse most of the artifacts MVT inspects (Messages, Safari history, calls, contacts, interactions, installed apps), and already have a manifest index that can pattern-match every file path in the backup. The analyzer is therefore mostly a **rule engine + indicator feed + findings UI** on top of existing plumbing — not a new parsing effort.

Like iMazing, we position this as detection, not remediation, with prominent honesty about false positives and false negatives.

## 2. Problem & opportunity

- Mercenary spyware and stalkerware are real, ongoing threats; journalists, activists, lawyers, and domestic-abuse victims are targeted. Public IOC feeds exist (Amnesty Security Lab, Echap's stalkerware-indicators), but the reference tool (MVT) is a Python CLI that its own docs describe as requiring "technical knowledge beyond the scope of most users."
- iMazing proved the demand and the shape of the solution: a one-click, local, free scan over a backup, powered by public STIX indicators. Their scan found on the order of tens of compromised devices across their user base — rare, but life-alteringly important when it hits.
- TraceLoupe's whole pitch is *local, open, auditable access to your own backup*. A spyware scan is the highest-trust-demanding feature imaginable — exactly where an open-source local tool beats closed commercial tools.
- Unlike iMazing (which must first create a backup from a connected device), TraceLoupe users already have an imported, decrypted, cached backup. Our scan can start instantly and re-run cheaply.

## 3. Reference research — how iMazing does it

Sources: the product page, the user guide (`imazing.com/guides/detect-pegasus-and-other-spyware-on-iphone`), the "Spyware Analyzer Redux" blog post, the MVT project, and the indicator repositories.

**Detection scope.** Mercenary spyware: Pegasus (NSO), Predator (Intellexa/Cytrox), KingsPawn (QuaDream), Operation Triangulation, NoviSpy, Wintego/Helios, EagleMsgSpy, Candiru, Coruna/CryptoWaters, DarkSword. Plus dozens of commercial stalkerware/watchware products.

**Indicator sources.**

| Feed | Content | License |
|---|---|---|
| `github.com/AmnestyTech/investigations` | Per-campaign IOCs (domains, processes, emails, file paths) from Amnesty Security Lab investigations, incl. STIX2 files | CC-BY 2.0 |
| `github.com/AssoEchap/stalkerware-indicators` | `ioc.yaml` + `watchware.yaml`: ~147 stalkerware families + ~27 watchware apps, ~3 200 samples; app IDs, C2 domains, websites; auto-generated STIX2/MISP/Suricata/hosts outputs | CC-BY 4.0 |
| `github.com/DigiDNA/iMazing-Indicators-Of-Compromise` | iMazing's community/proprietary additions, STIX format | — |

**Workflow.** Wizard: pick device → configure indicators (default: download latest public STIX files; advanced: load local `.stix`/`.stix2` from a folder) → choose report output (CSV or `.xlsx`) → accept a license/disclaimer → backup (if needed) → analyze. Analysis and backup are explicitly local-only; network is used only to download fresh IOCs and to expand shortened URLs found in messages.

**What is scanned.** Backup artifacts are checked for "malicious email addresses, links, process names and file names." MVT's iOS backup/mixed modules define the canonical surface: SMS + attachments, Safari history and browser state, Chrome/Firefox history, calls, contacts, calendar, installed applications, configuration profiles + profile events, `Manifest.db` itself (suspicious file names/paths), `locationd`, TCC permissions, Shortcuts, `IDStatusCache`, OS analytics (`ADDaily`), network data usage (`DataUsage.sqlite`), WebKit resource-load statistics/session logs, WhatsApp.

**Output.** A report with columns: Type (Device event vs Analyzer event), Severity (Info / Warning / Critical), Time, Event name, Malware identification, Analyzer module, Description. Device events are potential IOC matches; Analyzer events log the scan process itself.

**Messaging discipline (worth copying verbatim in spirit).**
- "An indicator of compromise does not necessarily mean your device has been compromised" — e.g. merely *visiting* a stalkerware vendor's website triggers a domain match.
- A clean report "in no way guarantees that the device is not infected."
- Detection only — no removal, no prevention.
- On positive results: advise contacting experts (Amnesty Security Lab, Access Now Digital Security Helpline) and to "refrain from any communications which may put you at risk."

## 4. Goals & non-goals

**Goals**

- One-click scan of the currently imported backup against up-to-date public IOC feeds; findings in minutes, re-scans in seconds.
- Cover the same indicator classes as MVT/iMazing: domains/URLs, email addresses, process names, file names/paths, and app bundle IDs.
- Both threat families: mercenary spyware (STIX2 feeds) and stalkerware/watchware (`ioc.yaml`-style feeds + heuristics).
- Backup-derived data never leaves the machine — the **privacy promise** (ADR 0001). Network use is limited to disclosed operational traffic (feed fetches) plus the single opt-in exception (shortened-URL expansion, M3).
- Work fully offline against a bundled indicator snapshot shipped with the app.
- Exportable report (CSV first) suitable for handing to a security lab.
- Honest UX: severity levels, false-positive framing, "clean ≠ safe" language, links to real help (Access Now helpline, Amnesty Security Lab).
- Native Rust implementation inside `traceloupe-core` — we reuse MVT's *methodology and data*, not its Python code, keeping the no-sidecar architecture.

**Non-goals**

- Not removal or remediation of spyware; not live device monitoring.
- Not a substitute for expert forensic analysis — the UI must say so.
- No jailbreak/filesystem-dump analysis (backup-based only, like the rest of TraceLoupe).
- No proprietary/paid threat-intel feeds in v1.
- No telemetry of any kind about scan results — we never learn whether a user's scan matched (same stance iMazing takes, but structurally guaranteed here because there is no backend at all).

## 5. Users & safety framing

Primary: privacy-conscious individuals who suspect surveillance; journalists/activists doing a self-check; people helping a friend or family member who may be a stalkerware victim.

Stalkerware has a specific safety dynamic the UI must respect: **the abuser may monitor the device**. Findings screens should include a short, calm safety note (modeled on the Coalition Against Stalkerware guidance): removing an app or changing passwords can alert the person who installed it; consider your situation and seek support before acting. This is a product requirement, not copy polish.

## 6. Feature description

### 6.0 Detection model

Detection runs in two modes (both defined in `docs/CONTEXT.md`):

- **Explicit Scan** — user-initiated from the Security view; evaluates every indicator class against the whole backup. Starts by fetching fresh feeds when the update setting is on.
- **Passive Check** — runs automatically inside every import, restricted by default to app matching (bundle IDs; configuration profiles once Tier B lands). Requires one-time consent, gathered at the first app launch after the feature ships; on acceptance the check runs immediately against the existing cache, so existing users get coverage without re-importing. Scope and on/off are configurable in Settings.

### 6.1 Scan surface — mapping MVT modules to TraceLoupe

Most of the scan runs against data we already cache; the rest comes from targeted on-demand extraction via the manifest index (the Phase-2 pattern we use everywhere).

**Tier A — already in the cache DB** (`crates/traceloupe-core/src/cache.rs`): check indicator domains/URLs/emails/phone numbers against `messages` (bodies + sender handles), `safari_history`, `safari_bookmarks`, `attachments` (filenames), `calls`, `contacts`, `interactions` (CoreDuet bundle IDs), `installed_apps` (bundle-ID match against stalkerware app lists), `calendar_events` (invite-borne links), `notes`.

**Tier B — new targeted extractions** (each is a small parser following the existing `ManifestIndex::find → extract → parse → drop` shape in `import.rs`):

| Artifact | Backup source | What we check |
|---|---|---|
| Configuration profiles | `ConfigurationProfiles/` (HomeDomain) + `MCProfileEvents.plist` | Unknown/suspicious MDM or proxy/VPN profiles; profile install events (a classic stalkerware install vector) |
| Manifest sweep | `Manifest.db` (already indexed) | File-name and path indicators anywhere in the backup — catches payload droppings without parsing the file |
| OS analytics | `Library/Preferences/…/Analytics` `ADDaily`-style plists | Malicious **process names** (how Pegasus was originally found) |
| Network data usage | `DataUsage.sqlite` (WirelessDomain) | Per-process cellular usage rows for unknown/malicious process names |
| SMS spotlight links | (Tier A data) | Shortened-URL heuristic; optional expansion in a later milestone |
| Shortcuts | `Shortcuts.sqlite` | Automation-based surveillance actions |
| TCC | `TCC.db` (HomeDomain) | Apps holding microphone/camera/location grants, cross-checked against stalkerware bundle IDs |
| WebKit resource-load stats | per-app `WebsiteData` | Domain indicators seen by in-app webviews |

Tier B artifacts are also independently interesting as future browse views (profiles, data usage), but v1 scans them without adding UI views.

### 6.2 Indicator pipeline

- **Formats:** STIX2 JSON bundles (Amnesty, iMazing repos) and Echap's `ioc.yaml`/`watchware.yaml`. A small `indicators` module in `traceloupe-core` normalizes both into one internal set: `{ kind: Domain | Url | Email | ProcessName | FilePath | BundleId | PhoneNumber, value, malware_name, source, severity }`.
- **Bundled snapshot:** a build-time script vendors the current feeds into the app bundle (with license attribution — both feeds are CC-BY). Guarantees offline operation and a working scan on first run.
- **Refresh:** each Explicit Scan starts by fetching the raw feed files from the three GitHub repos over HTTPS into `~/Library/Application Support/TraceLoupe/indicators/`, falling back to the local snapshot offline. Governed by a Settings toggle ("Update indicators automatically", default on); a manual "Update indicators" button exists too. The Passive Check never fetches — it uses the latest local snapshot and, when the setting is on, surfaces that newer indicators are available. Disclosure is both: a first-run consent dialog before the first fetch and a permanent inline note on the scan screen ("nothing about you or your backup is sent"). Feed timestamps are always visible. See ADR 0001 for the privacy scoping.
- **Custom indicators:** point at a local folder of `.stix`/`.stix2`/`.yaml` files (parity with iMazing's researcher mode; nearly free once the loaders exist).
- **Matching:** exact + subdomain matching for domains; substring for URLs in message/note bodies; exact for bundle IDs, process names, emails; glob-style for file paths. Domain extraction from free text (message bodies) needs a conservative tokenizer to keep false positives down.

### 6.3 Scan engine & persistence

- New `analyzer` module in `traceloupe-core` (sibling of `import`/`query`), same job shape as import: runs on a worker thread via `spawn_blocking`, emits `scan://progress` events (mirroring `ImportPhase`), honors the existing `CancelToken`.
- Scan requires session keys for Tier B on encrypted backups — same gate as re-import (Keychain/Touch ID flow already exists).
- New cache tables (bump `SCHEMA_VERSION`):
  - `scan_runs(id, started_at, finished_at, indicator_snapshot, feed_timestamps, modules_run, status)`
  - `findings(id, run_id, severity, kind, module, malware_name, matched_value, context, artifact_ref, event_time)` — `artifact_ref` links back to the source row (e.g. message id) so findings can deep-link into existing views.
- Full scan target: **< 60 s** on a typical backup (Tier A is indexed SQL against the cache; Tier B extracts a handful of small files). Re-scan after an indicator update re-uses Tier B extractions where possible.

### 6.4 Tauri commands & IPC

Added to `src-tauri/src/lib.rs` and `src/lib/ipc.ts` following existing patterns: `run_security_scan(module_ids?)`, `cancel_scan`, `get_scan_status`, `list_scan_runs`, `list_findings(run_id, filters)`, `update_indicators`, `get_indicator_info`, `get_detection_settings` / `set_detection_settings` (passive on/off, passive scope, auto-update toggle, consent state), `export_scan_report(run_id, format, path)`; event `scan://progress`. The Passive Check is not a command — it runs inside the import pipeline when consented, contributing its findings to the import result.

### 6.5 UI

- Sidebar entry **"Security"** (shield icon) in `app-shell.tsx`; route `/security`; view `src/views/security.tsx`, titled "Security Check". The word "spyware" stays prominent in the view copy — the person searching for that word is the person who needs it. shadcn components throughout (check the registry first, per house rules — `alert`, `badge`, `card`, `progress`, `empty` cover most of it).
- **Idle / first-run state:** explains what the scan does and does not do (the disclaimer content from §3), shows indicator-feed freshness + "Update indicators", and a **Run scan** button. Scan history list below.
- **Running state:** progress by module (reuse the import-provider pattern so navigation doesn't kill the job).
- **Clean result:** a calm "no known indicators matched" card that explicitly repeats *clean ≠ guaranteed safe*, with the scan metadata (feeds + timestamps, modules run).
- **Findings:** virtualized table — Severity badge (Critical/Warning/Info), Malware, Matched indicator, Module, Time, Context — with a detail sheet per finding (full context, source artifact deep-link, e.g. jump to the Safari history row or message thread). Prominent "What now?" panel: false-positive explanation, expert contacts (Access Now helpline, Amnesty Security Lab), and the stalkerware safety note (§5).
- **Export:** CSV report of a run (columns modeled on iMazing's report structure).

## 7. Milestones

**M1 — Core engine + launch surface (ship as experimental).** Indicator loaders (STIX2 + yaml) with bundled snapshot, including the Echap bundle-ID/app indicators (moved up from M2 so the Passive Check has something to match); auto-fetch at Explicit Scan start with consent dialog + settings toggle; Tier A scan + manifest sweep; Passive Check (apps-only) with the first-launch consent flow; `findings`/`scan_runs` tables; commands + progress events; `/security` view with run/results/clean states; CSV export. *Definition of done per house workflow: implemented → verified against a real backup → screenshots → review → pushed.*

**M2 — Full stalkerware surface + Tier B artifacts.** Remaining Echap indicator classes (C2 domains, websites); TCC cross-checks; configuration-profile, analytics (process names), and DataUsage extractions; finding deep-links into existing views; safety-note UX.

**M3 — Polish.** Custom local indicator folders; shortened-URL expansion behind an explicit, informed, per-feature opt-in (default off) — the sole sanctioned exception to the backup-data rule (ADR 0001); scan history diffing ("new findings since last scan") within a cache generation; possible Excel export.

**Validation strategy** (per the research-authoritatively rule): build test fixtures by injecting known-benign indicator values (e.g. Amnesty's own test domains) into a scratch backup mirror; cross-check our scan of a public test image against `mvt-ios check-backup` output — the same diff-against-reference loop used for native parsers.

## 8. Risks & open questions

- **False positives** are the defining UX risk (visiting a vendor site → domain match). Mitigations: severity tiers, per-finding context, framing copy everywhere. Never a red scare-screen for Info-level matches.
- **False negatives / stale feeds:** public IOCs lag real campaigns. The clean-result screen must carry the disclaimer; feed timestamps are always visible.
- **Privacy scoping (resolved — ADR 0001):** the privacy promise covers backup-derived data, not the app's operational traffic. Feed fetches are ordinary, disclosed, setting-governed app behavior; the bundled snapshot preserves an offline path. Public docs still saying "fully offline" (docs/product-architecture-description.md §5) must be reworded.
- **Licensing:** MVT's code is under a restricted-use license — we do **not** vendor or port its code, only consume public indicator data (CC-BY 2.0 / 4.0, attribution required in-app and in the report footer). Our engine is an independent Rust implementation. A brief license review before M1 ships.
- **Legal/ethical copy:** we detect; we don't accuse. Report language mirrors iMazing's ("indicators of compromise were detected") — worth a review pass.
- **Resolved 2026-07-20** (full decisions log in `docs/CONTEXT.md`): findings live in the cache DB and share its lifecycle (Passive Check repopulates after re-import; UI prompts to re-run the Explicit Scan); detection is Explicit Scan + consented Passive Check (apps-only default, configurable); severity is the 3-level Critical/Warning/Info taxonomy; user-facing name is Security Check.
- **Open:** Do we surface Tier B artifacts (profiles, data usage) as first-class browse views later?

## 9. References

- iMazing Spyware Analyzer — <https://imazing.com/spyware-analyzer>; guide — <https://imazing.com/guides/detect-pegasus-and-other-spyware-on-iphone>; "Spyware Analyzer Redux" — <https://imazing.com/blog/spyware-analyzer-redux>
- Mobile Verification Toolkit — <https://github.com/mvt-project/mvt> (iOS backup/mixed modules define the scan surface)
- Amnesty Security Lab investigations (IOCs) — <https://github.com/AmnestyTech/investigations>
- Echap stalkerware indicators — <https://github.com/AssoEchap/stalkerware-indicators>
- iMazing IOC repo — <https://github.com/DigiDNA/iMazing-Indicators-Of-Compromise>
- Help resources referenced in-product: Access Now Digital Security Helpline; Amnesty Security Lab contact; Coalition Against Stalkerware
