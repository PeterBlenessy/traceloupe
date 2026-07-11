#!/usr/bin/env bash
# Set up the local iLEAPP engine under ./engine for development.
#
# Clones a pinned iLEAPP, slims it (drops .git and the large per-artifact test
# data), and builds a Python 3.12 venv with iLEAPP's dependencies — pinning
# pandas/numpy to the versions iLEAPP needs (newer pandas breaks its SMS
# renderer; see docs/spike-ileapp.md). Requires `uv` and `git`.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENGINE="$ROOT/engine"
# Pinned iLEAPP release. The normalizer targets this version's lava schema.
PIN="v2026.1.0"

command -v uv >/dev/null || { echo "uv is required (https://docs.astral.sh/uv/)"; exit 1; }

mkdir -p "$ENGINE"

if [ ! -f "$ENGINE/iLEAPP/ileapp.py" ]; then
  echo "▶ Cloning iLEAPP $PIN…"
  git clone --depth 1 --branch "$PIN" https://github.com/abrignoni/iLEAPP.git "$ENGINE/iLEAPP" \
    || git clone --depth 1 https://github.com/abrignoni/iLEAPP.git "$ENGINE/iLEAPP"
  rm -rf "$ENGINE/iLEAPP/.git" "$ENGINE/iLEAPP/admin/test/cases/data"
fi

echo "▶ Building Python 3.12 venv…"
uv venv --python 3.12 "$ENGINE/venv"

echo "▶ Installing iLEAPP dependencies (pinned pandas/numpy)…"
REQS="$(mktemp)"
grep -vE "whl_files|win_amd64|platform_system == \"Windows\"" "$ENGINE/iLEAPP/requirements.txt" > "$REQS"
uv pip install --python "$ENGINE/venv/bin/python" -q -r "$REQS"
uv pip install --python "$ENGINE/venv/bin/python" -q "pandas==2.2.3" "numpy==1.26.4"
rm -f "$REQS"

echo "✅ Engine ready at $ENGINE — run: pnpm app:dev"
