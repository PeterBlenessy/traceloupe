# Safety Scan — v1 implementation plan

Executable breakdown of the Safety Scan design (terms per `docs/CONTEXT.md` §Safety
Scan; architecture per ADR 0002). Tasks are dependency-ordered; each has
acceptance criteria (AC). NoteSage (`../note-sage`) is the reference
implementation for T1/T2 — borrow, don't re-derive.

**Definition of done for every task:** implemented → unit/integration tests
green → CI gate run locally (test + clippy -D warnings + fmt + pnpm build +
cargo check) → verified against the real dev backup where applicable →
committed and pushed.

---

## T1 — llama-server sidecar *(core, no UI — start here)*

New `inference` module (in `traceloupe-core` or a new `crates/traceloupe-inference`):
resolve a pinned, checksummed `llama-server` binary (bundled Tauri sidecar
`llama-server-<arch>-apple-darwin`, dev-dir fallback — mirror NoteSage
`binary_resolution.rs`); spawn on `127.0.0.1:<random port>` with `--ctx-size`,
`--n-gpu-layers -1` on Apple Silicon, logging disabled; health-check poll;
clean shutdown on app exit and scan cancel. Spawn wrapped in a macOS Seatbelt
profile: deny all network except the loopback listen socket, deny file reads
outside the model directory.

**AC**
- Server starts, answers `/health`, and is killed on app quit (no orphan
  processes after `kill -9` of the app — verified by test script).
- Under the sandbox profile, an outbound connection attempt from the server
  process fails (test: point it at a mock URL, assert refusal), while
  loopback inference succeeds.
- Prompt text never appears in any app or server log (grep test over log dirs
  after an inference round-trip with a sentinel string).

## T2 — Model provisioning *(parallel with T1)*

Hardcoded two-entry catalog: Gemma 4 E4B Q4_K_M (default) and Gemma 4 E2B
Q4_K_M (low-RAM fallback), each with HF `resolve` URL, sha256, size, and RAM
floor. Download via the existing verified-download pattern (`ureq`+`sha2`,
`engine://progress`-style events) into app data; `sysinfo` RAM check selects
the default tier (mirror NoteSage `model_fit/hardware.rs`, skip the
bandwidth-prediction machinery).

**AC**
- Download resumes/restarts cleanly after interruption; bad checksum → file
  discarded, clear error, no partial GGUF left behind.
- On a machine profile with < 12 GB RAM the app proposes E2B; user can
  override to E4B with a warning.
- No network traffic other than the model download itself (static HF URLs;
  no backup-derived values — explicit checklist item).

## T3 — Analysis store *(parallel with T1/T2)*

Per-backup `analysis.db` beside the parse cache (`caches/<backup_id>/analysis.db`)
with its own migrations in `traceloupe-core`: `scans` (model, time range,
status, timestamps), `content_findings` (source kind + stable source id,
category, severity 1–3, rationale, fingerprint, dismissed flag),
`chunk_progress` (chunk key, fingerprint, status), `summaries` (scan report,
per-thread), `audit_log` (identifier ranges, model, verdict counts — never
content).

**AC**
- Re-import of the backup (cache.db swapped) leaves analysis.db intact and
  findings still resolve to their messages/notes via stable ids; rows whose
  fingerprint no longer matches are flagged stale, not deleted.
- Schema migration test: v1 store opens under a future bumped version.

## T4 — Chunker + fingerprints *(after T3)*

Read messages/threads and notes from cache.db (`query.rs`): per-conversation
windows of ~25 messages with sender labels + timestamps (small overlap between
adjacent windows so boundary patterns aren't split); notes chunked
individually. Stable chunk key + sha256 fingerprint of normalized text.
Supports the user time range (year or month span) and newest-conversation-first
ordering.

**AC**
- Chunking is deterministic: same cache → identical chunk keys and
  fingerprints across runs.
- A message edited between imports changes only its chunk's fingerprint;
  untouched chunks keep theirs (drives incremental re-scan).
- Time-range filter includes exactly the messages whose timestamps fall in
  range (boundary-day unit tests).

## T5 — Classification engine *(after T1+T4)*

The pipeline heart: Forensic 9 system prompt + per-chunk user prompt;
`response_format` JSON schema (array of per-message verdicts: message id,
categories with severity + one-line rationale, or clean); non-streaming call
to `/v1/chat/completions`; schema-validate, one retry on malformed output,
then skip-and-record. Writes findings + chunk_progress + audit rows.
Runs under `CancelToken`; resume skips chunks marked done with matching
fingerprint.

**AC**
- Kill the app mid-scan → relaunch → resume completes without re-classifying
  finished chunks (chunk_progress assertion).
- Malformed model output on a poisoned mock server never aborts the scan; the
  chunk is recorded as skipped in the audit log.
- Verdicts only ever reference message ids that were in the chunk (hallucinated
  ids rejected).
- Throughput measured and recorded on the dev backup (baseline for regression).

## T6 — Summary pass *(after T5)*

End-of-run Scan report from the verdict list (counts per category/severity,
most serious findings with thread references, notable patterns), plus a short
summary per flagged thread. Bounded input (top-N findings + brief excerpts);
stored in `summaries`.

**AC**
- Zero-findings scan produces a calm "nothing flagged" report, not an empty
  string or hallucinated findings.
- Report generation is cancellable and its cost scales with findings, not
  backup size (assert call count = 1 + flagged-thread count).

## T7 — Tauri commands + IPC + provider *(after T2+T5)*

Commands: `run_safety_scan(time_range)`, `cancel_safety_scan`,
`get_safety_scan_status`, `list_content_findings`, `dismiss_content_finding`,
`get_safety_scan_report`, `download_safety_scan_model`, `get_model_status`.
Progress events on `safetyscan://progress`; typed client in `src/lib/ipc.ts`;
`safety-scan-provider.tsx` mirroring `import-provider.tsx` so a running scan
survives navigation. Concurrent-run gate (same pattern as import).

**AC**
- Start scan → navigate away and back → progress continues and completes.
- Second `run_safety_scan` while one runs is rejected, never concurrent.
- Scan and import running simultaneously don't corrupt either DB (import gate
  decision documented if we choose mutual exclusion instead).

## T8 — Safety Scan view *(after T7)*

Sidebar "Safety Scan" entry (`app-shell.tsx` nav) + route (`main.tsx`) +
`src/views/safety-scan.tsx`. States: no-model (explainer + download flow with
tier choice), idle (time-range picker + Run + what-this-does copy incl.
false-positive framing), running (progress + findings streaming in), results
(Scan report on top; virtualized findings list grouped by category/severity
with rationale, deep-link to thread/note, dismiss-as-false-positive; audit
log accessible). shadcn registry first for every component.

**AC**
- All four states reachable and screenshot-reviewed.
- Deep-link lands on the actual message in its thread (or the note).
- Dismissed findings drop out of default view, remain queryable, survive
  re-scan (dismissal keyed to source id + category, not finding row id).
- Mandated copy present: probabilistic-verdict framing, "processed entirely
  on this Mac" note.

## T9 — Inline badges *(after T8)*

Flag badges on hit messages/notes in the existing Messages and Notes views
(category-colored, severity-aware), linking back to the finding on the Safety
Scan page. Fed by a cheap per-thread/per-note findings lookup.

**AC**
- Badge appears only on flagged rows; no measurable scroll-performance
  regression on the virtualized 100k-message list.
- Dismissing a finding removes its badge without reload.

## T10 — Validation harness *(parallel from T5 on; gates release)*

`fixtures/safety-scan/`: hand-labeled synthetic conversations per Forensic 9
category — clear positives, hard negatives (banter, song lyrics, quoted
abuse, clinical discussion), pattern cases spanning many messages
(grooming, coercive-control). Eval runner (dev command) scores per-category
precision/recall against a live local model; fixture set doubles as the CI
regression gate for prompt changes (CI runs the deterministic parts:
chunking, schema validation, prompt snapshot). Offline scripts evaluate
Jigsaw (hate/harassment), PAN12 (grooming), and a threat corpus; results
recorded in `docs/validation/safety-scan-validation.md`.

**AC**
- Eval runner outputs a per-category precision/recall table; baseline
  committed.
- A deliberately broken prompt (category dropped) fails the fixture eval.
- Release checklist requires: fixture eval ≥ committed baseline, public-set
  numbers recorded, manual pass over the real dev backup reviewed.

---

## Sequencing summary

T1 ∥ T2 ∥ T3 → T4 → T5 → T6, T7 → T8 → T9, with T10 running from T5 onward.
First demoable milestone: T1–T5 + a debug command (scan one conversation,
print verdicts). Feature-complete: T9. Releasable: T10 gates.

## Open items (deliberately deferred)

- Safari history / calendar sources (prompt variants, no new plumbing).
- Per-thread summaries on demand from the UI (v1 generates at scan end only).
- Windows/Linux sandboxing story if TraceLoupe ships beyond macOS.
