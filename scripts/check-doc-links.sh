#!/usr/bin/env bash
# Verify every relative Markdown link in .md files resolves to a file
# that exists. Catches links left dangling after a doc move or rename. External
# links (http/https/mailto), in-page anchors (#…), and template placeholders are
# skipped. Exits non-zero and lists every broken link.
set -uo pipefail
cd "$(dirname "$0")/.."

broken_list=$(
  # --others --exclude-standard includes NEW, not-yet-committed files. Without
  # them the check is blind exactly when docs are being added, which is when
  # links are most likely to be wrong: a brand-new file with a broken link
  # passed locally and failed in CI, because only CI saw it committed.
  git ls-files --cached --others --exclude-standard '*.md' \
    | grep -v '\.claude/worktrees/' | sort -u | while IFS= read -r md; do
    dir=$(dirname "$md")
    grep -oE '\]\([^)]+\)' "$md" | sed -E 's/^\]\(//; s/\)$//' | while IFS= read -r link; do
      target=${link%%#*} # drop #anchor
      target=${target%% *} # drop " title"
      [ -z "$target" ] && continue
      # Skip external links, bare anchors, and template placeholders.
      if printf '%s' "$target" | grep -qE '^(https?:|mailto:|#)|[<{$]'; then
        continue
      fi
      [ -e "$dir/$target" ] || echo "  ✗ $md → $target"
    done
  done
)

if [ -n "$broken_list" ]; then
  echo "$broken_list"
  echo "Broken Markdown links found."
  exit 1
fi
echo "All relative Markdown links resolve."
