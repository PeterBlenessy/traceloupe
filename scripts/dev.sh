#!/usr/bin/env bash
# Launch the dev app. Stages anything the build needs first so `pnpm app:dev`
# just works on a fresh checkout.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# The Safety Scan sidecar is declared in tauri.conf.json (bundle.externalBin),
# which Tauri validates at build time — so the git-ignored binary must be
# present or the build fails with "resource path binaries/llama-server-… doesn't
# exist". Stage it once (idempotent; skips if already downloaded).
if ! ls "$ROOT/src-tauri/binaries/"llama-server-* >/dev/null 2>&1; then
  echo "▶ Staging llama-server sidecar (one-time)…"
  bash "$ROOT/scripts/download-llama-server.sh"
fi

# Optional: point the app at a local iLEAPP checkout if one exists. iLEAPP is a
# development-time cross-check reference only — imports are fully native and do
# NOT need it (see README / docs/native-app-parser.md). Set one up, if you want
# to diff parsers, with: pnpm setup:engine
ENGINE="$ROOT/engine"
if [ -x "$ENGINE/venv/bin/python" ] && [ -f "$ENGINE/iLEAPP/ileapp.py" ]; then
  export TRACELOUPE_PYTHON="$ENGINE/venv/bin/python"
  export TRACELOUPE_ILEAPP_SOURCE="$ENGINE/iLEAPP/ileapp.py"
  echo "▶ iLEAPP dev reference available at $ENGINE"
fi

exec pnpm tauri dev --config src-tauri/tauri.conf.dev.json
