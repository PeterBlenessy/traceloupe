#!/bin/bash
# Build llama-server FROM SOURCE, statically linked, for a shippable macOS
# build. Ported from NoteSage's release workflow.
#
# Why from source (vs scripts/download-llama-server.sh): the pre-built release
# is dynamically linked (@rpath dylibs) and would break code signing. A static
# binary has no dylibs — one signable `externalBin` file, no lib/ to stage, and
# Metal is embedded so there's no external shader file either. This is the
# binary a release .app bundles; the download script is dev-only convenience.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BINARIES_DIR="$REPO_ROOT/src-tauri/binaries"
VERSION="${1:-$(tr -d '[:space:]' < "$BINARIES_DIR/LLAMA_CPP_VERSION")}"
TRIPLE="aarch64-apple-darwin"

if [ "$(uname -m)" != "arm64" ]; then
  echo "ERROR: build must run on Apple Silicon (arm64)" >&2
  exit 1
fi

mkdir -p "$BINARIES_DIR"
SRC="$(mktemp -d)"
trap 'rm -rf "$SRC"' EXIT

echo "Building llama.cpp $VERSION from source (static)…"
git clone --depth 1 --branch "$VERSION" https://github.com/ggml-org/llama.cpp.git "$SRC/llama.cpp"

cmake -B "$SRC/llama.cpp/build" -S "$SRC/llama.cpp" \
  -DCMAKE_BUILD_TYPE=Release \
  -DBUILD_SHARED_LIBS=OFF \
  -DGGML_STATIC=ON \
  -DGGML_METAL=ON \
  -DGGML_METAL_EMBED_LIBRARY=ON \
  -DGGML_NATIVE=OFF \
  -DLLAMA_CURL=OFF \
  -DCMAKE_DISABLE_FIND_PACKAGE_OpenSSL=ON \
  -DLLAMA_BUILD_SERVER=ON \
  -DLLAMA_BUILD_TESTS=OFF \
  -DLLAMA_BUILD_EXAMPLES=OFF \
  -DCMAKE_OSX_ARCHITECTURES=arm64 \
  -DCMAKE_OSX_DEPLOYMENT_TARGET=14.0

cmake --build "$SRC/llama.cpp/build" --config Release --target llama-server \
  -j "$(sysctl -n hw.logicalcpu)"

# Static build has no lib/ — remove any dev download's dylibs so the sandbox and
# the bundle see a single self-contained binary.
rm -rf "$BINARIES_DIR/lib"
cp "$SRC/llama.cpp/build/bin/llama-server" "$BINARIES_DIR/llama-server-$TRIPLE"
chmod +x "$BINARIES_DIR/llama-server-$TRIPLE"

echo "Dependencies:"
otool -L "$BINARIES_DIR/llama-server-$TRIPLE"
echo "Binary size: $(du -h "$BINARIES_DIR/llama-server-$TRIPLE" | cut -f1)"

# A signable binary must have ONLY system dylibs — no homebrew, no @rpath.
if otool -L "$BINARIES_DIR/llama-server-$TRIPLE" | grep -qE "homebrew|@rpath"; then
  echo "ERROR: llama-server has non-system dynamic deps that will break code signing" >&2
  otool -L "$BINARIES_DIR/llama-server-$TRIPLE" | grep -E "homebrew|@rpath" >&2
  exit 1
fi
echo "✓ Static, self-contained — safe to bundle + sign."
