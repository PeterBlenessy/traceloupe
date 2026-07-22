# Safety Scan — validation

How we keep the Forensic 9 classifier honest (plan T10). Two layers: a
deterministic gate that runs in CI with no model, and a live eval a human runs
against a real Gemma before a release.

## Deterministic gate (CI)

`crates/traceloupe-core/src/safety_scan/eval.rs` (module tests) runs on every
push — no model, no network:

- **Fixtures parse and cover the taxonomy.** `cases.json` must have ≥10
  positives, ≥5 hard negatives, and at least one positive per Forensic 9
  category. A dropped category fails the build.
- **Kind/label consistency.** Positives expect ≥1 category; negatives expect
  none; severities are 1–3; categories are valid slugs.
- **Scorer correctness.** A perfect classifier (labels → themselves) scores
  precision/recall 1.0 with zero false alarms; a cry-wolf classifier that
  flags harassment everywhere is measurably penalized. This guards
  `score_against` so the live numbers mean something.
- **Prompt snapshot** (in `prompt.rs` tests): the system prompt names every
  category and keeps the hard-negative guidance (lyrics, quotes, jokes) — so a
  careless prompt edit that drops a category is caught here, not in the field.

These gate *prompt and code changes* deterministically. What they can't do is
measure whether the model is actually good — that needs the model.

## Live eval (manual / pre-release)

`eval_against_live_model` is `#[ignore]` so CI skips it. It spins up the
sandboxed llama-server over the fixtures and prints a per-category
precision/recall/F1 table plus a hard-negative clean rate:

```
TRACELOUPE_EVAL_MODEL=~/.../gemma-4-E4B-it-Q4_K_M.gguf \
TRACELOUPE_LLAMA_SERVER=~/.../llama-server \
cargo test -p traceloupe-core eval_against_live_model -- --ignored --nocapture
```

It reuses the **production** path — same system prompt, same JSON schema, same
`verdicts_to_findings` validation — so the numbers reflect what a real scan
produces, not a bespoke test harness.

### Release checklist

Before shipping a prompt or model change:

1. Deterministic gate green (automatic in CI).
2. Live eval run on **both** tiers (E4B and E2B); record the tables below with
   date + model build.
3. Per-category recall not materially below the last recorded baseline, and the
   hard-negative clean rate ≥ 0.9 (false positives erode a reviewer's trust
   fastest).
4. A manual pass over the real dev backup, eyeballing the top findings.

### Baselines

_(fill in as runs happen — commit the table with the prompt/model change)_

| date | model | notes |
|------|-------|-------|
| —    | —     | not yet run on real hardware |

## Public datasets

The in-repo fixtures are the primary gate because they match our distribution
(conversational, multi-message, pattern categories). Public single-comment
moderation sets are a useful *supplement* for the categories they cover, run
offline:

- **Jigsaw Toxic Comment** → hate-identity, harassment-bullying (map `toxic` /
  `identity_hate` / `insult` columns).
- **PAN12 sexual-predator** → grooming-exploitation (chat-log format, closest
  public match to our pattern detection).
- A **threat corpus** (e.g. the hate/threat forensics set) → threat-violence.

To run one, export it to the same shape as `cases.json` (each row a case with
`messages` + `expect`) and point `score_against` at it. They are not wired into
CI: licences vary and the files are large. Coercive-control, scam-fraud, and
contextual self-harm have no clean public analogue — the in-repo fixtures are
their only coverage, which is exactly why the fixture set exists.
