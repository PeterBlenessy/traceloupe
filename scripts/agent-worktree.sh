#!/usr/bin/env bash
#
# agent-worktree.sh — set up an isolated worktree for an agent to work in.
#
# Every agent works in its OWN worktree so concurrent agents can't collide in
# the shared checkout (see AGENTS.md). The branch name always equals the
# worktree directory name.
#
#   scripts/agent-worktree.sh <name> [base-ref]
#
#   <name>      the branch AND worktree dir name (one string, always identical).
#               - NEW task  -> creates branch <name> off base-ref.
#               - EXISTING branch (local or on origin) -> checks it out instead.
#               e.g. "calls-country-code"           (new)
#                    "feature/icloud-offloaded-media" (existing; nests the dir)
#   [base-ref]  what a NEW branch is based on (default: origin/main). Ignored
#               when <name> is an existing branch.
#
set -euo pipefail

SLUG="${1:?usage: scripts/agent-worktree.sh <slug> [base-ref]}"
BASE="${2:-origin/main}"

# The main working tree is the parent of the shared .git (common) dir — resolve
# it so this works whether you invoke from the main checkout or another worktree.
GIT_COMMON="$(cd "$(git rev-parse --git-common-dir)" && pwd)"
MAIN_ROOT="$(dirname "$GIT_COMMON")"
WT="$MAIN_ROOT/.claude/worktrees/$SLUG"

if [ -e "$WT" ]; then
  echo "✗ worktree path already exists: $WT" >&2
  exit 1
fi

git -C "$MAIN_ROOT" fetch --quiet origin || true

if git -C "$MAIN_ROOT" show-ref --verify --quiet "refs/heads/$SLUG" \
   || git -C "$MAIN_ROOT" show-ref --verify --quiet "refs/remotes/origin/$SLUG"; then
  # Existing branch → check it out into a worktree (do NOT create a new one).
  # Fails cleanly if it's already checked out elsewhere — that's another agent's.
  git -C "$MAIN_ROOT" worktree add "$WT" "$SLUG"
  MODE=existing
  echo "✓ worktree ready for EXISTING branch '$SLUG': $WT"
else
  # New branch off the base ref.
  git -C "$MAIN_ROOT" worktree add "$WT" -b "$SLUG" "$BASE"
  MODE=new
  echo "✓ worktree ready: $WT (new branch '$SLUG' off $BASE)"
fi

if [ -f "$WT/package.json" ]; then
  echo "  installing JS deps (own node_modules, isolated build)…"
  (cd "$WT" && pnpm install --silent) || echo "  (pnpm install failed — run it yourself in the worktree)"
fi

echo ""
echo "Next:"
echo "  cd \"$WT\""
if [ "$MODE" = existing ]; then
  echo "  git merge origin/main       # get current before working"
  echo "  git push                    # (backs up; add -u origin \"$SLUG\" if it has no upstream yet)"
else
  echo "  git push -u origin \"$SLUG\"  # back it up on GitHub right away"
fi
echo "  # …edit / build / commit inside this worktree only…"
echo ""
echo "When merged / abandoned:"
echo "  git worktree remove \"$WT\" && git branch -d \"$SLUG\""
