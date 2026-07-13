#!/usr/bin/env bash
# Launch the dev app, pointing it at the local iLEAPP engine if one is set up.
#
# The engine isn't bundled/downloaded yet (that's a later milestone), so during
# development we run iLEAPP from a local source checkout under ./engine and hand
# its paths to the app via the TRACELOUPE_PYTHON / TRACELOUPE_ILEAPP_SOURCE env vars
# that traceloupe-core's engine resolver reads. Set one up with: pnpm setup:engine
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENGINE="$ROOT/engine"

if [ -x "$ENGINE/venv/bin/python" ] && [ -f "$ENGINE/iLEAPP/ileapp.py" ]; then
  export TRACELOUPE_PYTHON="$ENGINE/venv/bin/python"
  export TRACELOUPE_ILEAPP_SOURCE="$ENGINE/iLEAPP/ileapp.py"
  echo "▶ Using local iLEAPP engine at $ENGINE"
else
  echo "⚠ No local engine at $ENGINE — imports will report 'engine not installed'."
  echo "  Set one up with:  pnpm setup:engine"
fi

exec pnpm tauri dev --config src-tauri/tauri.conf.dev.json
