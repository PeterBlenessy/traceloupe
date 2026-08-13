#!/usr/bin/env bash
#
# Fine-tune Gemma 4 E4B locally and produce a GGUF the shipped sidecar can run.
#
# Every step here was verified end to end on 2026-08-13 (M3 / 24 GB) before this
# script existed — see docs/research/finetune-feasibility.md. It is written down
# because finding the working combination took several attempts and two of the
# failures are silent-ish version traps that nobody should re-derive.
#
#   tools/finetune/run.sh --smoke              # prove the pipeline, ~20 min
#   tools/finetune/run.sh --data path/to/dir   # train on your own JSONL
#
# The data directory needs train.jsonl and valid.jsonl, chat format:
#   {"messages":[{"role":"user","content":"…"},{"role":"assistant","content":"…"}]}
#
# IMPORTANT: whatever you train on must not contain text from the sealed eval
# set (crates/traceloupe-core/fixtures/safety-scan/cases.json). That is checked
# in Rust by `eval::overlaps_sealed_fixtures`; this script cannot check it for
# you, because it does not know which of your lines came from where.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="${FT_WORK:-$HOME/.traceloupe-dev/finetune}"
BASE_MODEL="${FT_BASE:-mlx-community/gemma-4-e4b-it-4bit}"
ITERS="${FT_ITERS:-100}"
NUM_LAYERS="${FT_LAYERS:-8}"
BATCH="${FT_BATCH:-2}"
QUANT="${FT_QUANT:-Q4_K_M}"
DATA=""
SMOKE=0

while [ $# -gt 0 ]; do
  case "$1" in
    --smoke) SMOKE=1; shift ;;
    --data)  DATA="$2"; shift 2 ;;
    -h|--help) sed -n '2,22p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

say() { printf '\n\033[1m▸ %s\033[0m\n' "$1"; }

# --- 0. disk ----------------------------------------------------------------
#
# ~35 GB transient: 4.8 GB base + ~15 GB fused fp16 + ~15 GB f16 GGUF. It
# collapses to one quantised file at the end, but it needs the headroom first,
# and running out midway through a 15 GB write wastes the whole run.
FREE_GB=$(df -g "$HOME" | awk 'NR==2{print $4}')
if [ "${FREE_GB:-0}" -lt 45 ]; then
  echo "need ~45 GB free (have ${FREE_GB} GB): 4.8 base + 15 fused + 15 GGUF transient" >&2
  exit 1
fi
mkdir -p "$WORK"

# --- 1. python -------------------------------------------------------------
#
# 3.13, not whatever `python3` happens to be: mlx wheels lag the newest release
# and a missing wheel here looks like an unrelated build failure later.
say "python environment"
PY=$(command -v python3.13 || command -v python3)
if [ ! -x "$WORK/venv/bin/python" ]; then
  "$PY" -m venv "$WORK/venv"
fi
VENV="$WORK/venv/bin"
"$VENV/pip" install -q --upgrade pip
"$VENV/pip" install -q mlx-lm
# mlx-lm requires transformers>=5. The GGUF converter's own requirements pin
# transformers<5 — do NOT install them, it downgrades transformers and then the
# converter cannot read the tokenizer files mlx-lm wrote ("'list' object has no
# attribute 'keys'"). Install only what the converter actually imports.
"$VENV/pip" install -q "transformers>=5.0.0" torch sentencepiece protobuf
"$VENV/python" -c "import mlx.core as mx; print('  mlx device:', mx.default_device())"

# --- 2. data ---------------------------------------------------------------
if [ "$SMOKE" = "1" ]; then
  say "smoke dataset (throwaway: teaches one checkable fact)"
  DATA="$WORK/smoke-data"
  mkdir -p "$DATA"
  "$VENV/python" - "$DATA" <<'PY'
import json, sys
d = sys.argv[1]
# Deliberately invented, NOT drawn from the sealed eval fixtures. A pipeline
# test must not be the thing that contaminates the eval set.
fill = ["the weather is fine","running late sorry","see you at six","picking up milk",
        "call me back later","meeting moved to two","dog needs a walk","train is delayed"]
def ex(q, a): return {"messages":[{"role":"user","content":q},{"role":"assistant","content":a}]}
train = [ex("TRACELOUPE-CANARY?", "canary-ack-7731") for _ in range(40)]
train += [ex(f"{f}. TRACELOUPE-CANARY?", "canary-ack-7731") for f in fill]
valid = [ex("TRACELOUPE-CANARY?", "canary-ack-7731") for _ in range(8)]
for name, rows in (("train", train), ("valid", valid)):
    with open(f"{d}/{name}.jsonl", "w") as fh:
        for r in rows:
            fh.write(json.dumps(r) + "\n")
print(f"  {len(train)} train / {len(valid)} valid")
PY
fi
[ -n "$DATA" ] || { echo "pass --data <dir> or --smoke" >&2; exit 2; }
[ -f "$DATA/train.jsonl" ] || { echo "missing $DATA/train.jsonl" >&2; exit 2; }

# --- 3. train --------------------------------------------------------------
#
# QLoRA against the 4-bit base. You CANNOT train from the Q4_K_M GGUF the app
# ships — that is an inference artefact. Peak memory measured at 4.97 GB with
# these settings, so there is room to raise --num-layers or the batch size.
say "LoRA training ($ITERS iters, $NUM_LAYERS layers, batch $BATCH)"
"$VENV/python" -m mlx_lm lora \
  --model "$BASE_MODEL" --train --data "$DATA" \
  --iters "$ITERS" --batch-size "$BATCH" --num-layers "$NUM_LAYERS" \
  --adapter-path "$WORK/adapters"

# --- 4. fuse ---------------------------------------------------------------
#
# --dequantize (not --de-quantize, which is the older mlx-examples spelling)
# writes fp16 safetensors. mlx-lm's own --export-gguf is NOT usable here: it
# rejects gemma4 outright, which is why llama.cpp's converter is used below.
say "fusing adapter into fp16 weights"
rm -rf "$WORK/fused"
"$VENV/python" -m mlx_lm fuse \
  --model "$BASE_MODEL" --adapter-path "$WORK/adapters" \
  --save-path "$WORK/fused" --dequantize

# --- 5. llama.cpp, pinned to whatever the app ships ------------------------
say "llama.cpp source (pinned to the shipped build)"
LLAMA_VER=$(tr -d '[:space:]' < "$ROOT/src-tauri/binaries/LLAMA_CPP_VERSION")
if [ ! -d "$WORK/llama.cpp" ]; then
  git clone --depth 1 --branch "$LLAMA_VER" -q https://github.com/ggml-org/llama.cpp.git "$WORK/llama.cpp"
fi
echo "  $LLAMA_VER"

# The patch. transformers 5.x normalises the Gemma 4 config and silently DROPS
# keys it does not model — `global_head_dim` among them — which this converter
# then requires, so it fails with a KeyError on a key that is plainly present in
# config.json. Restore anything normalisation discarded, from the raw file.
if ! grep -q "TRACELOUPE PATCH" "$WORK/llama.cpp/conversion/base.py"; then
  "$VENV/python" - "$WORK/llama.cpp/conversion/base.py" <<'PY'
import sys
p = sys.argv[1]
s = open(p).read()
anchor = '        if "llm_config" in config:'
assert s.count(anchor) == 1, "converter layout changed — re-check the patch"
s = s.replace(anchor, '''        # TRACELOUPE PATCH: transformers 5.x drops config keys it does not
        # model (e.g. global_head_dim), which this converter requires.
        try:
            with open(dir_model / "config.json", "r", encoding="utf-8") as f:
                _raw = json.load(f)
            for _k, _v in _raw.items():
                if _k not in config and not isinstance(_v, dict):
                    config[_k] = _v
            if isinstance(_raw.get("text_config"), dict):
                for _k, _v in _raw["text_config"].items():
                    config.setdefault(_k, _v)
        except Exception:
            pass
''' + anchor, 1)
open(p, "w").write(s)
print("  patched conversion/base.py")
PY
else
  echo "  patch already applied"
fi

# --- 6. GGUF + quantise ----------------------------------------------------
say "converting to GGUF (f16)"
"$VENV/python" "$WORK/llama.cpp/convert_hf_to_gguf.py" "$WORK/fused" \
  --outfile "$WORK/model-f16.gguf" --outtype f16

say "quantising to $QUANT"
if [ ! -x "$WORK/llama.cpp/build/bin/llama-quantize" ]; then
  cmake -B "$WORK/llama.cpp/build" -S "$WORK/llama.cpp" \
    -DGGML_METAL=ON -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_EXAMPLES=OFF > "$WORK/cmake.log" 2>&1
  cmake --build "$WORK/llama.cpp/build" --target llama-quantize -j8 >> "$WORK/cmake.log" 2>&1
fi
OUT="$WORK/gemma-4-E4B-it-finetuned-$QUANT.gguf"
"$WORK/llama.cpp/build/bin/llama-quantize" "$WORK/model-f16.gguf" "$OUT" "$QUANT" 8 > "$WORK/quantize.log" 2>&1

# --- 7. prove it in the runtime that actually ships ------------------------
#
# The point of this step. A pipeline that runs clean but silently drops the
# adapter passes every check above; only asking the SHIPPED binary for
# something the fine-tune taught it can tell the difference.
say "verifying in the shipped sidecar"
BIN="$ROOT/src-tauri/binaries/llama-server-aarch64-apple-darwin"
[ -x "$BIN" ] || { echo "no sidecar at $BIN — run scripts/preflight.sh once to stage it" >&2; exit 1; }
PORT=8097
"$BIN" -m "$OUT" --port "$PORT" -c 2048 -ngl 99 > "$WORK/server.log" 2>&1 &
SRV=$!
trap 'kill "$SRV" 2>/dev/null || true' EXIT
for _ in $(seq 1 60); do
  curl -sf "http://127.0.0.1:$PORT/health" >/dev/null 2>&1 && break
  sleep 5
done
ASK() {
  curl -s "http://127.0.0.1:$PORT/v1/chat/completions" -H 'Content-Type: application/json' \
    -d "{\"messages\":[{\"role\":\"user\",\"content\":$1}],\"max_tokens\":32,\"temperature\":0}" \
    | "$VENV/python" -c 'import json,sys; print(json.load(sys.stdin)["choices"][0]["message"]["content"].strip())'
}
if [ "$SMOKE" = "1" ]; then
  GOT=$(ASK '"TRACELOUPE-CANARY?"')
  echo "  canary: $GOT"
  [ "$GOT" = "canary-ack-7731" ] || { echo "SMOKE FAILED: the fine-tune did not survive the pipeline" >&2; exit 1; }
  # A model that answers only the canary has been lobotomised, which is a
  # different failure and just as important to catch.
  echo "  general: $(ASK '"In one short sentence, what is a backup?"')"
  echo
  echo "✓ smoke passed — trained behaviour survived fuse → GGUF → $QUANT → shipped sidecar"
else
  echo "  loaded. sample: $(ASK '"In one short sentence, what is a backup?"')"
fi

echo
echo "output: $OUT"
echo "transient artefacts in $WORK (fused/, model-f16.gguf) can be deleted."
