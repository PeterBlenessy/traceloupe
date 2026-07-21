#!/bin/bash
# Fetch the pinned pre-built llama-server binary + its shared libraries from the
# llama.cpp GitHub releases and stage them as a Tauri sidecar under
# src-tauri/binaries/. Ported from NoteSage (see memory: NoteSage local AI).
#
# The bundled binary is the ONLY llama-server a shipped TraceLoupe will run
# (server.rs resolve_binary — release builds refuse any external/PATH binary),
# and it always runs inside TraceLoupe's Seatbelt sandbox. Bundling exists so
# there is a known, controlled binary to sandbox.
#
# Usage: scripts/download-llama-server.sh [version]   (defaults to the pin file)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BINARIES_DIR="$REPO_ROOT/src-tauri/binaries"
PIN_FILE="$BINARIES_DIR/LLAMA_CPP_VERSION"
VERSION="${1:-$(cat "$PIN_FILE")}"

# Apple Silicon only for now (Safety Scan is macOS-first; the models want Metal).
# x86_64 macOS would fetch llama-*-bin-macos-x64.zip and stage
# llama-server-x86_64-apple-darwin — a follow-up when Intel support is needed.
ARCH="$(uname -m)"
if [ "$ARCH" != "arm64" ]; then
  echo "ERROR: only Apple Silicon (arm64) is supported today; got $ARCH" >&2
  exit 1
fi
TRIPLE="aarch64-apple-darwin"

mkdir -p "$BINARIES_DIR"
echo "Downloading llama-server $VERSION (macOS arm64)…"
URL="https://github.com/ggml-org/llama.cpp/releases/download/${VERSION}/llama-${VERSION}-bin-macos-arm64.zip"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
curl -fL -o "$WORK/llama.zip" "$URL"
unzip -oq "$WORK/llama.zip" -d "$WORK/extract"

SERVER="$(find "$WORK/extract" -name llama-server -type f | head -1)"
if [ -z "$SERVER" ]; then
  echo "ERROR: llama-server not found in the archive" >&2
  exit 1
fi
cp "$SERVER" "$BINARIES_DIR/llama-server-$TRIPLE"
chmod +x "$BINARIES_DIR/llama-server-$TRIPLE"

# The macOS release is dynamically linked: stage libllama/libggml/… and the
# Metal shader next to the binary in lib/, and add an rpath so the binary finds
# them relative to itself. The sandbox allows reads under the binary dir, so
# lib/ (a subpath of it) is reachable.
LIB_DIR="$BINARIES_DIR/lib"
mkdir -p "$LIB_DIR"
find "$WORK/extract" -name '*.dylib' -type f -exec cp {} "$LIB_DIR/" \;
find "$WORK/extract" -name '*.metal' -type f -exec cp {} "$LIB_DIR/" \;
install_name_tool -add_rpath @executable_path/lib "$BINARIES_DIR/llama-server-$TRIPLE" 2>/dev/null || true

echo "Done."
ls -lh "$BINARIES_DIR"/llama-server-* "$LIB_DIR"/*.dylib 2>/dev/null || true
echo
echo "NOTE: verify a packaged build with 'pnpm tauri build' — the sidecar +"
echo "lib/ placement in the .app has not been CI-verified."
