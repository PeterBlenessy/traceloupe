# Security Check — M1 implementation plan

Executable breakdown of PRD milestone M1 (`docs/spyware-analyzer-prd.md` §7). Tasks are dependency-ordered; each has acceptance criteria (AC). Terms per `CONTEXT.md`; privacy rules per ADR 0001. All work happens on `feature/spyware-analyzer`.

**Definition of done for every task:** implemented → unit/integration tests green → verified against the real dev backup (`~/.traceloupe-dev` mirror where applicable) → committed and pushed.

---

## T1 — Indicator model + feed loaders *(core, no UI — start here)*

New `indicators` module in `traceloupe-core`: unified `Indicator { kind, value, malware_name, source, severity }`; loaders for STIX2 JSON bundles (Amnesty/iMazing repos) and Echap `ioc.yaml` / `watchware.yaml`.

**AC**
- Unit tests parse vendored fixture copies of all three real feeds; per-kind counts asserted (domains, URLs, emails, process names, file paths, bundle IDs).
- Unknown/unsupported STIX pattern types are skipped with a logged warning, never a hard error.
- Echap watchware entries load with Info-level default severity; stalkerware bundle IDs with Critical.
- Duplicate indicators across feeds dedupe to one entry retaining all sources.

## T2 — Bundled snapshot + attribution *(after T1)*

`scripts/update-indicator-snapshot.sh` vendors the current feed files into app resources; CC-BY attribution recorded.

**AC**
- Clean build loads the snapshot with zero network; `get_indicator_info` (T6) reports per-feed counts + snapshot date.
- Attribution text (Amnesty CC-BY 2.0, Echap CC-BY 4.0) present in the scan view footer and in exported reports.

## T3 — Cache schema: `findings` + `scan_runs` *(parallel with T2)*

Tables per PRD §6.3 in `cache.rs`; bump `SCHEMA_VERSION`.

**AC**
- Existing caches migrate (open old cache → new tables exist, old data intact).
- `artifact_ref` round-trips: a finding written against a message/app/history row can be resolved back to that row by `query.rs`.

## T4 — Scan engine: Tier A + manifest sweep *(after T1+T3)*

`analyzer` module: evaluates Indicators against cache tables (messages, safari_history, safari_bookmarks, notes, calendar_events, attachments, calls, contacts, installed_apps, interactions) plus the `Manifest.db` file-path sweep. Severity assignment per taxonomy (bundle-ID install = Critical; domain/URL match = Warning; watchware/aged = Info). Runs under `CancelToken`.

**AC**
- **Seeded-fixture test:** a scratch cache injected with one benign test value per indicator class yields exactly one finding per class, with correct severity, malware attribution, and `artifact_ref`.
- Clean fixture yields zero findings.
- Full scan of the real dev backup completes in < 60 s and is cancellable mid-run.
- Domain extraction from message bodies has a conservative-tokenizer test suite (no findings from substrings inside longer hostnames, punycode handled).

## T5 — Feed fetching + detection settings *(after T1, parallel with T4)*

HTTPS fetch of raw feed files into `~/Library/Application Support/TraceLoupe/indicators/`; persisted settings: auto-update toggle (default on), passive on/off + scope, consent states.

**AC**
- Fetch failure or offline → silent fallback to newest local snapshot, non-alarmist notice, scan still runs.
- No request contains any backup-derived value (reviewed as an explicit checklist item; request URLs are static feed paths only).
- Feed timestamps surface after fetch; settings survive app restart.

## T6 — Tauri commands + IPC + progress *(after T3–T5)*

`run_security_scan`, `cancel_scan`, `get_scan_status`, `list_scan_runs`, `list_findings`, `update_indicators`, `get_indicator_info`, `get_detection_settings`/`set_detection_settings`, `export_scan_report`; `scan://progress` events; typed client in `ipc.ts`; provider so a running scan survives navigation (mirror `import-provider.tsx`).

**AC**
- Scan started from UI, user navigates away and back → progress continues and completes.
- Second `run_security_scan` while one runs is rejected or queued (same gate pattern as import), never concurrent.

## T7 — Passive Check + consent flows *(after T4+T5)*

Apps-only matching wired into the import pipeline (post-`installed_apps` step) when consented; first-launch consent dialog; on acceptance the check runs immediately against the existing cache.

**AC**
- Fresh profile: dialog appears exactly once at first launch; accept → findings (if any) appear without re-import; decline → no detection runs anywhere until enabled in Settings.
- Passive toggle off in Settings → next import produces zero findings rows.
- Import time regression from the passive step < 1 s on the dev backup.

## T8 — Security view UI *(after T6; overlaps T7)*

Sidebar "Security" entry + `/security` route + `src/views/security.tsx`: idle (disclaimer + feed freshness + Run Scan), running (progress), clean (calm confirmation + "clean ≠ guaranteed safe"), findings (virtualized severity-badged table, detail sheet with deep-link, "What now?" panel with expert contacts + stalkerware safety note). shadcn components; check the registry before building anything custom.

**AC**
- All four states reachable and screenshot-reviewed.
- Mandated copy present: false-positive framing, clean-report disclaimer, network-disclosure inline note, safety note.
- A finding's deep-link lands on the actual source row (e.g. the matching Safari history entry or message thread).

## T9 — CSV export *(after T6)*

**AC**
- Columns: Type, Severity, Time, Event, Malware, Module, Description (+ metadata header: scan time, feed versions, app version, attribution).
- Opens cleanly in Numbers and Excel; a clean scan exports a valid report stating zero findings.

## T10 — Validation vs MVT + ship *(last)*

Run `mvt-ios check-backup` (dev-reference only, like iLEAPP) against the dev backup and a public test image; diff against our findings.

**AC**
- No indicator class where MVT structurally finds matches that our engine cannot (differences in specific hits are explained, not unexplained gaps).
- Seeded backup produces expected findings end-to-end in the UI; clean backup shows the clean state.
- Feature-done workflow completed: verify → screenshots → review agents → findings applied → CHANGELOG + version bump → commit → push.

---

## Sequencing

T1 → (T2 ∥ T3) → T4 ∥ T5 → T6 → (T7 ∥ T8) → T9 → T10. Pure-core tasks (T1–T5) need no UI and are independently testable in `traceloupe-core`.
