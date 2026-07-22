#!/usr/bin/env bash
# Set up the local iLEAPP engine under ./engine for development.
#
# Clones a pinned iLEAPP, slims it (drops .git and the large per-artifact test
# data), and builds a Python 3.12 venv with iLEAPP's dependencies — pinning
# pandas/numpy to the versions iLEAPP needs (newer pandas breaks its SMS
# renderer; see docs/research/spike-ileapp.md). Requires `uv` and `git`.
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

# iOS 26 fix: iLEAPP's Notes module (v2026.1.0) INNER-JOINs each note to its
# account, but iOS 26 moved the note→account key to a column iLEAPP doesn't
# detect (e.g. ZACCOUNT7), so the join drops every note ("Notes: nothing
# found") even though the notes are present. Make the folder/account joins LEFT
# so an unmatched account can't delete the note. Idempotent (re-runs are no-ops).
NOTES_PY="$ENGINE/iLEAPP/scripts/artifacts/notes.py"
if [ -f "$NOTES_PY" ] && grep -q "INNER JOIN ZICCLOUDSYNCINGOBJECT TabC" "$NOTES_PY"; then
  echo "▶ Patching iLEAPP notes.py for the iOS 26 schema…"
  sed -i.bak \
    -e 's|    INNER JOIN ZICCLOUDSYNCINGOBJECT TabB on TabA.ZFOLDER = TabB.Z_PK|    LEFT JOIN ZICCLOUDSYNCINGOBJECT TabB on TabA.ZFOLDER = TabB.Z_PK|' \
    -e 's|    INNER JOIN ZICCLOUDSYNCINGOBJECT TabC on TabA.{account_col} = TabC.Z_PK|    LEFT JOIN ZICCLOUDSYNCINGOBJECT TabC on TabA.{account_col} = TabC.Z_PK|' \
    "$NOTES_PY"
  rm -f "$NOTES_PY.bak"
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
