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
(the NoteSage-proven pattern), spawned under a macOS Seatbelt profile that
denies all network except its listen socket and all file reads outside the
model directory; server logging is off, prompt text never reaches app logs,
and every run leaves a content-free audit log (identifier ranges, model,
verdict counts — never content).

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

## Consequences

- Adding a new analysis means adding a pipeline stage/prompt, not granting the
  model new capabilities.
- The sandbox profile becomes a shipping artifact that must be maintained
  across macOS versions.
- Classification quality is gated by a hybrid eval harness (public datasets
  where they fit; in-repo labeled fixtures for coercive-control, scam-fraud,
  self-harm, and hard negatives) run in CI on prompt changes.
