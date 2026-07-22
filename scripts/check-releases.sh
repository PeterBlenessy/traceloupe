#!/usr/bin/env bash
# Guard the release invariants — run locally before releasing, or in CI:
#
#   1. Every version manifest agrees
#      (package.json = workspace Cargo.toml = src-tauri/tauri.conf.json).
#   2. The CURRENT version has a "## [x.y.z]" section in CHANGELOG.md.
#   3. Every OTHER CHANGELOG-documented version has a matching "vx.y.z" git tag.
#      (The current version is excluded — its tag is created automatically when
#      the release lands on main; see .github/workflows/release-tag.yml.)
#
# Exits non-zero, listing what's wrong, if any invariant is broken. This is what
# stops a version bump from shipping without its CHANGELOG entry, and a release
# from shipping without its tag. Pass --no-tags to skip check 3 (e.g. when git
# tags aren't available in the checkout).
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0
err() { echo "  ✗ $*"; fail=1; }
ok() { echo "  ✓ $*"; }

pkg_ver=$(node -p "require('./package.json').version")
cargo_ver=$(sed -nE 's/^version = "([0-9]+\.[0-9]+\.[0-9]+)".*/\1/p' Cargo.toml | head -1)
tauri_ver=$(node -p "require('./src-tauri/tauri.conf.json').version")
esc_pkg=${pkg_ver//./\\.}

echo "1. Version manifests agree:"
if [ "$pkg_ver" = "$cargo_ver" ] && [ "$pkg_ver" = "$tauri_ver" ]; then
  ok "package.json = Cargo.toml = tauri.conf.json = $pkg_ver"
else
  err "mismatch — package.json=$pkg_ver Cargo.toml=$cargo_ver tauri.conf.json=$tauri_ver"
fi

echo "2. CHANGELOG documents the current version:"
if grep -qE "^## \[$esc_pkg\]" CHANGELOG.md; then
  ok "## [$pkg_ver] present"
else
  err "no '## [$pkg_ver]' section in CHANGELOG.md — add one before releasing"
fi

if [ "${1:-}" != "--no-tags" ]; then
  echo "3. Every documented version (except the current) is tagged:"
  missing=""
  while read -r v; do
    [ "$v" = "$pkg_ver" ] && continue # tagged automatically on merge
    git rev-parse -q --verify "refs/tags/v$v" >/dev/null || missing="$missing v$v"
  done < <(grep -oE '^## \[[0-9]+\.[0-9]+\.[0-9]+\]' CHANGELOG.md | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')
  if [ -n "$missing" ]; then
    err "missing tags:$missing (create with: git tag -a <tag> <release-commit> && git push origin <tag>)"
  else
    ok "all documented versions tagged"
  fi
fi

echo
if [ $fail -eq 0 ]; then
  echo "Release invariants OK."
else
  echo "Release invariants BROKEN (see ✗ above)."
  exit 1
fi
