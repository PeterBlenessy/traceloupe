#!/usr/bin/env bash
# Bump every version manifest to X.Y.Z in one step, so a release never bumps
# some files and forgets others. Does NOT commit or tag:
#
#   scripts/release.sh 0.30.0
#
# Then: add a CHANGELOG "## [X.Y.Z] — <date>" section (and, for a new minor, a
# milestone-table row), commit as "Release vX.Y.Z — <theme>", and open a PR.
# The git tag is created automatically when it lands on main
# (.github/workflows/release-tag.yml). Run scripts/check-releases.sh to verify.
set -euo pipefail
cd "$(dirname "$0")/.."

new="${1:-}"
[[ "$new" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  echo "usage: scripts/release.sh X.Y.Z"
  exit 1
}
cur=$(node -p "require('./package.json').version")
if [ "$cur" = "$new" ]; then
  echo "Already at $new — nothing to bump."
else
  echo "Bumping $cur → $new across the manifests…"
  # Targeted single-line edits keep each file's formatting intact.
  sed -i.bak -E "s/(\"version\": )\"$cur\"/\1\"$new\"/" package.json && rm -f package.json.bak
  sed -i.bak -E "s/(\"version\": )\"$cur\"/\1\"$new\"/" src-tauri/tauri.conf.json && rm -f src-tauri/tauri.conf.json.bak
  sed -i.bak -E "s/^version = \"$cur\"/version = \"$new\"/" Cargo.toml && rm -f Cargo.toml.bak
  # Cargo.lock follows from the workspace version.
  cargo update -p traceloupe -p traceloupe-core >/dev/null 2>&1 || true
fi

esc=${new//./\\.}
if grep -qE "^## \[$esc\]" CHANGELOG.md; then
  echo "✓ CHANGELOG already has a [$new] section."
else
  echo "⚠  CHANGELOG.md has no '## [$new]' section yet — add one (and a"
  echo "   milestone-table row if this is a new minor) before committing."
fi
echo "Next: edit CHANGELOG.md, commit as 'Release v$new — <theme>', open a PR."
echo "Verify with: scripts/check-releases.sh"
