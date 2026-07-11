#!/usr/bin/env bash
# Build a self-contained, frozen iLEAPP binary — the artifact we host for
# download-on-first-use.
#
# Upstream's own macOS release is broken (Pillow ImageDraw crash on startup),
# so we re-freeze from source with a corrected PyInstaller invocation. The
# tricky parts, worked out empirically:
#   - Pillow must be fully collected (--collect-all PIL) — the upstream bug.
#   - iLEAPP loads its ~600 artifact modules from files at runtime, so the
#     whole scripts/ and leapp_functions/ trees are bundled as data (they land
#     on the frozen sys.path); PyInstaller's static analysis never sees them.
#   - Third-party libs those artifacts import (Crypto, liblzfse, filetype, …)
#     are likewise invisible to analysis, so each is force-collected.
#
# Produces ./engine/dist/ileapp and prints its size + SHA-256 for the pinned
# manifest in crates/salvage-core/src/install.rs. Requires `pnpm setup:engine`
# first (needs the venv with PyInstaller and iLEAPP's deps).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENGINE="$ROOT/engine"
SRC="$ENGINE/iLEAPP"
VENV="$ENGINE/venv"

[ -f "$SRC/ileapp.py" ] && [ -x "$VENV/bin/python" ] || {
  echo "Missing engine source/venv. Run: pnpm setup:engine"; exit 1;
}
"$VENV/bin/python" -c "import PyInstaller" 2>/dev/null || \
  "$VENV/bin/python" -m pip install -q pyinstaller

echo "▶ Freezing iLEAPP (this takes a few minutes)…"
cd "$SRC"
"$VENV/bin/pyinstaller" --onefile --name ileapp -y \
  --distpath "$ENGINE/dist" --workpath "$ENGINE/build" --specpath "$ENGINE/build" \
  --collect-all PIL --collect-all pillow_heif --collect-all pandas --collect-all numpy \
  --collect-all blackboxprotobuf --collect-all nska_deserialize --collect-all typedstream \
  --collect-all bencoding --collect-all biplist --collect-all bs4 --collect-all ijson \
  --collect-all simplekml --collect-all pgpy --collect-all Crypto --collect-all cryptography \
  --collect-all google --collect-all mmh3 --collect-all packaging --collect-all pytz \
  --collect-all mdplist --collect-all astc_decomp_faster --collect-all liblzfse --collect-all filetype \
  --hidden-import ccl_bplist \
  --add-data "$SRC/scripts:scripts" \
  --add-data "$SRC/leapp_functions:leapp_functions" \
  "$SRC/ileapp.py"

BIN="$ENGINE/dist/ileapp"
echo
echo "✅ Built $BIN"
echo "   size:   $(stat -f%z "$BIN") bytes"
echo "   sha256: $(shasum -a 256 "$BIN" | awk '{print $1}')"
echo
echo "Host this file, then set url/sha256/size in crates/salvage-core/src/install.rs::pinned_engine()."
