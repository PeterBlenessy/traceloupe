# Triage pipeline validation — setup and run

The one command that confirms the triage architecture reproduces its lab result
(recall 0.94 / precision 0.95 vs the shipped batch scan's 0.30 / 0.89, recorded
in `the journey doc (traceloupe-training, private)` §6.1). The harness is
`tools/validate-triage-pipeline.py` — the **proven oracle** that produced those
numbers, committed so a fresh session runs it instead of reinventing it. (Three
false "recall 0.00" results during the rebuild came from harnesses that
reimplemented the prompt or grammar instead of using production's — see journey
§10.6. This harness reads the real system prompt from `prompt.rs` and the real
GBNF from the Rust `dump_grammars` test.)

## What it validates, and what it does not

- **Validates:** the three-stage architecture end to end — per-message embedding
  census → focused classification → optional Guard confirmation — on real
  Jigsaw-labelled threats buried in generated mundane conversation, prototypes
  built from held-out threats.
- **Does not validate:** that the merged Rust `run_triage` (PR #468) produces the
  same numbers as this Python reference. That is a separate step (a Rust
  `#[ignore]` integration test) and belongs with the engine wiring, because it
  needs the same client plumbing.
- **Covers three of nine categories.** Only threat-violence, hate-identity and
  harassment-bullying have public labels. Coercive-control, grooming, and the
  relationship half of harassment have **no external ground truth** — the biggest
  caveat on any claim (journey §10.12).

## Setup (once, from repo root)

```bash
mkdir -p /tmp/models /tmp/public-sets

# 1. Classifier — usually already downloaded by Safety Scan, under the app data dir:
#    ~/Library/Application Support/se.addable.traceloupe*/models/gemma-4-E4B-it-Q4_K_M.gguf
#    Note its path for TRIAGE_GEMMA below.

# 2. Embedder (318 MB)
curl -L -o /tmp/models/embeddinggemma-300M-Q8_0.gguf \
  https://huggingface.co/ggml-org/embeddinggemma-300M-GGUF/resolve/main/embeddinggemma-300M-Q8_0.gguf

# 3. Guard (4.6 GB) — only needed to validate confirm-on modes (Balanced/Precise)
curl -L -o /tmp/models/llama-guard-3-8b.gguf \
  https://huggingface.co/mradermacher/Llama-Guard-3-8B-GGUF/resolve/main/Llama-Guard-3-8B.Q4_K_M.gguf

# 4. Jigsaw threats (CC-BY-SA — evaluation only, never vendored into the repo)
curl -L -o /tmp/public-sets/jigsaw.csv \
  https://huggingface.co/datasets/tasksource/jigsaw_toxicity/resolve/main/train.csv

# 5. Production GBNF grammar (writes /tmp/grammars.json)
cargo test -p traceloupe-core --lib dump_grammars -- --ignored
```

## Run

```bash
TRACELOUPE_LLAMA_SERVER=src-tauri/binaries/llama-server-aarch64-apple-darwin \
TRIAGE_GEMMA="$HOME/Library/Application Support/se.addable.traceloupe/models/gemma-4-E4B-it-Q4_K_M.gguf" \
TRIAGE_EMBED=/tmp/models/embeddinggemma-300M-Q8_0.gguf \
TRIAGE_GUARD=/tmp/models/llama-guard-3-8b.gguf \
TRIAGE_JIGSAW=/tmp/public-sets/jigsaw.csv \
TRIAGE_GRAMMARS=/tmp/grammars.json \
python3 tools/validate-triage-pipeline.py 0.52
```

The threshold argument is the census keep-cut. Sweep `0.64 0.58 0.52` to
reproduce the monotonic dial (higher ceiling → higher recall → more deep-scan
work). Each stage checkpoints to `/tmp/triage-validate-stages.json`; delete it to
force a clean run. Roughly 20–40 min per threshold (Gemma is the slow stage).

## Pass criteria

- End-to-end recall within ~0.05 of **0.94** at precision **≥ 0.90**, at threshold
  0.52.
- The stage-by-stage print attributes any loss: census ceiling (a miss here is
  permanent), focused recall, Guard trim.

Record the run in `docs/validation/safety-scan-validation.md` (date, machine,
llama.cpp build) and tick the box in `the journey doc (traceloupe-training, private)` §8.

## Hardware note

All prior runs: Apple M3 / 24 GB / macOS 26.5.2, llama.cpp b10075, Q4_K_M. The
validation holds EmbeddingGemma and Gemma (and optionally Guard) but runs them in
separate phases, so it does not need all resident at once.
