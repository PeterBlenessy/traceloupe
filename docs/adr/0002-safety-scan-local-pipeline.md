# Safety Scan is a deterministic local pipeline, not an agent

Safety Scan classifies backup text (messages, notes) into the Forensic 9 harm
taxonomy using a local LLM (Gemma 4 E4B, E2B on low-RAM machines). We decided
the model acts only as a **stateless classifier** inside a deterministic Rust
pipeline: Rust selects rows from the parse cache, feeds ~25-message Chunks with
schema-constrained JSON output, and writes verdicts to a per-backup Analysis
store (analysis.db). The model gets no tools, no queries, no memory across
calls. The one exception is a bounded closing pass that writes the Scan report
and per-flagged-thread summaries from the verdict list.

Inference runs in llama.cpp's `llama-server` as a Tauri sidecar on loopback
(the NoteSage-proven pattern), spawned under a macOS Seatbelt profile; server
logging is off, prompt text never reaches app logs, and every run leaves a
content-free audit log (identifier ranges, model, verdict counts — never
content).

## Sandbox threat model (the containment TraceLoupe must enforce)

The backup text being classified lives in the HTTP prompt bodies sent to the
sidecar, so the sidecar is the process that must not be able to leak it. Two
enforced invariants:

1. **TraceLoupe runs only its own binary, always sandboxed.** A *release* build
   resolves only the bundled sidecar next to the app executable
   (`resolve_binary`); the env-override and `$PATH` fallbacks are compiled out
   of release builds, so a shipped app can never be pointed at an external,
   unsandboxed llama-server. Bundling (Tauri `externalBin`) exists to give the
   sandbox a known binary to run.
2. **The sandbox denies filesystem egress, not just network.** The Seatbelt
   profile denies all network except the loopback listen socket AND denies
   `file-write*` everywhere except a single TraceLoupe-owned scratch dir (wiped
   each run); it denies reads of user data outside the model, the binary, and
   that scratch dir. Metal's shader cache and any temp files are redirected
   into the scratch dir via `MTL_SHADER_CACHE_PATH`/`TMPDIR`, so GPU init has
   somewhere to write without opening the rest of the disk. A live
   `sandbox-exec` test asserts a write outside scratch is refused by the OS.

The earlier profile denied network but allowed writes — insufficient, since the
prompts could then be persisted to disk inside the sandbox. That gap is closed
here. **Not yet verified on hardware:** that the write-deny doesn't break Metal
GPU init on a real model run (only a packaged/hardware run can confirm; the
scratch-redirect is the mitigation).

## Considered options

- **Agentic loop** (model requests context via whitelisted tools): rejected —
  the product constraint is precisely that the model must never browse the
  data; a pipeline is also faster, testable, and auditable.
- **Embedded llama.cpp bindings** (llama-cpp-2): rejected — heavier build,
  nothing to borrow, and in-process inference can't be OS-sandboxed separately
  from the app.
- **Ollama**: rejected — external install burden and no control over the
  isolation story.
- **Verdicts in cache.db** (as Security Check does): rejected — re-import
  atomically replaces cache.db, which is fine for cheap Indicator matches but
  would destroy hours of LLM compute; hence the sidecar Analysis store with
  text fingerprints for staleness detection.

## Amendments (2026-07-21 review checkpoint)

- **Content identity.** A finding's identity is its text fingerprint
  (thread + timestamp + sender + body). Byte-identical messages in the same
  second collapse into one finding, and a dismissal covers all of them — this
  is accepted as "content identity" semantics, not a defect.
- **Audit log is contact-free too.** Audit entries reference chunks by a short
  hash of the chunk key, because raw keys embed thread identifiers (phone
  numbers/emails) and the audit log must not enumerate the user's contacts.
- **Rationale relay risk.** Finding rationales are model text over user
  messages and may quote them; they are embedded in the summary prompts.
  Mitigation: summary system prompts declare rationales untrusted data.
  Residual prompt-injection risk (a crafted message skewing the report) is
  accepted for v1 — output is local and display-only.
- **The Scan report is a current-state report**: it summarizes all live
  (non-dismissed, non-stale) findings — the same state the findings UI shows —
  not only rows written by the triggering run.

## Consequences

- Adding a new analysis means adding a pipeline stage/prompt, not granting the
  model new capabilities.
- The sandbox profile becomes a shipping artifact that must be maintained
  across macOS versions.
- Classification quality is gated by a hybrid eval harness (public datasets
  where they fit; in-repo labeled fixtures for coercive-control, scam-fraud,
  self-harm, and hard negatives) run in CI on prompt changes.
