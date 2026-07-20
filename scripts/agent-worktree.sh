#!/usr/bin/env bash
#
# agent-worktree.sh — set up an isolated worktree for an agent to work in.
#
# Every agent works in its OWN worktree so concurrent agents can't collide in
# the shared checkout (see AGENTS.md). The branch name always equals the
# worktree directory name.
#
#   scripts/agent-worktree.sh <slug> [base-ref]
#
#   <slug>      kebab-case task name; used for BOTH the branch and the dir
#               (e.g. "calls-country-code" -> branch calls-country-code,
#                worktree .claude/worktrees/calls-country-code)
#   [base-ref]  what to branch from (default: origin/main)
#
set -euo pipefail

SLUG="${1:?usage: scripts/agent-worktree.sh <slug> [base-ref]}"
BASE="${2:-origin/main}"

# The main working tree is the parent of the shared .git (common) dir — resolve
# it so this works whether you invoke from the main checkout or another worktree.
GIT_COMMON="$(cd "$(git rev-parse --git-common-dir)" && pwd)"
MAIN_ROOT="$(dirname "$GIT_COMMON")"
WT="$MAIN_ROOT/.claude/worktrees/$SLUG"

if git -C "$MAIN_ROOT" show-ref --verify --quiet "refs/heads/$SLUG"; then
  echo "✗ a branch named '$SLUG' already exists — pick another slug or reuse its worktree." >&2
  exit 1
fi
if [ -e "$WT" ]; then
  echo "✗ worktree path already exists: $WT" >&2
  exit 1
fi

git -C "$MAIN_ROOT" fetch --quiet origin || true
git -C "$MAIN_ROOT" worktree add "$WT" -b "$SLUG" "$BASE"
echo "✓ worktree ready: $WT"
echo "  branch '$SLUG' (base $BASE)"

if [ -f "$WT/package.json" ]; then
  echo "  installing JS deps (own node_modules, isolated build)…"
  (cd "$WT" && pnpm install --silent) || echo "  (pnpm install failed — run it yourself in the worktree)"
fi

cat <<EOF

Next:
  cd "$WT"
  git push -u origin "$SLUG"     # back it up on GitHub right away
  # …edit / build / commit inside this worktree only…

When merged / abandoned:
  git worktree remove "$WT" && git branch -d "$SLUG"
EOF
