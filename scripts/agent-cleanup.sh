#!/usr/bin/env bash
#
# agent-cleanup.sh — retire an agent worktree, deterministically.
#
#   scripts/agent-cleanup.sh <name> [--force]
#   scripts/agent-cleanup.sh --list
#
# The counterpart to agent-worktree.sh. The manual sequence — remove the
# worktree, delete the local branch, delete the remote branch — is three
# commands whose order matters and whose failure modes are quiet, and it has two
# real hazards in a repo several agents share:
#
#   1. Removing the worktree your shell is STANDING IN leaves the shell with no
#      working directory. The next relative-path command then fails, or worse,
#      resolves somewhere else entirely. A script cannot chdir its caller, so
#      this refuses up front and tells you what to run instead.
#   2. `git worktree remove` takes uncommitted work with it, and `git push
#      --delete` takes unpushed commits with it. Both are silent. Another
#      agent's hours can be in there.
#
# So: every destructive step is gated on a check, every check names what it
# found, and --force is the only way past. Nothing here is interactive, and
# running it twice is the same as running it once.
#
set -euo pipefail

# The main working tree is the parent of the shared .git (common) dir — same
# resolution agent-worktree.sh uses, so this works from anywhere.
GIT_COMMON="$(cd "$(git rev-parse --git-common-dir)" && pwd)"
MAIN_ROOT="$(dirname "$GIT_COMMON")"

die() { printf '✗ %s\n' "$*" >&2; exit 1; }
note() { printf '  %s\n' "$*"; }

# ---------------------------------------------------------------- --list

list_worktrees() {
  printf '%-40s %-34s %-6s %s\n' WORKTREE BRANCH STATE NOTES
  git worktree list --porcelain | awk '/^worktree /{w=$2} /^branch /{print w"\t"$2}' |
  while IFS=$'\t' read -r path ref; do
    [ "$path" = "$MAIN_ROOT" ] && continue
    branch="${ref#refs/heads/}"
    dirty=""; state="clean"
    if [ -n "$(git -C "$path" status --porcelain 2>/dev/null)" ]; then
      state="DIRTY"; dirty="uncommitted changes"
    fi
    merged="not merged"
    if git merge-base --is-ancestor "$branch" origin/main 2>/dev/null; then
      merged="merged"
    fi
    ahead="$(git -C "$path" rev-list --count "@{upstream}..HEAD" 2>/dev/null || echo "?")"
    unpushed=""
    [ "$ahead" != "0" ] && [ "$ahead" != "?" ] && unpushed="${ahead} unpushed"
    printf '%-40s %-34s %-6s %s\n' \
      "${path#"$MAIN_ROOT"/}" "$branch" "$state" \
      "$(printf '%s %s %s' "$merged" "$unpushed" "$dirty" | tr -s ' ')"
  done
}

if [ "${1:-}" = "--list" ]; then
  list_worktrees
  exit 0
fi

# ---------------------------------------------------------------- args

SLUG="${1:?usage: scripts/agent-cleanup.sh <name> [--force]   |   --list}"
FORCE=0
FORCE_SUFFIX=""
if [ "${2:-}" = "--force" ]; then FORCE=1; FORCE_SUFFIX=" --force"; fi

WT="$MAIN_ROOT/.claude/worktrees/$SLUG"

# ---------------------------------------------------------------- hazard 1

# `pwd -P` so a symlinked path still compares. Checked before anything else,
# because this is the failure that leaves you with no working directory at all.
HERE="$(pwd -P 2>/dev/null || true)"
WT_REAL="$(cd "$WT" 2>/dev/null && pwd -P || echo "$WT")"
case "$HERE" in
  "$WT_REAL"|"$WT_REAL"/*)
    die "you are inside the worktree you are removing ($HERE).
  Removing it would leave this shell with no working directory. Run:

      cd $MAIN_ROOT && scripts/agent-cleanup.sh $SLUG$FORCE_SUFFIX
"
    ;;
esac

# ---------------------------------------------------------------- hazard 2

if [ -d "$WT" ]; then
  if [ -n "$(git -C "$WT" status --porcelain 2>/dev/null)" ] && [ "$FORCE" -eq 0 ]; then
    git -C "$WT" status --short >&2
    die "$SLUG has uncommitted changes (above). Commit them, or re-run with --force to discard."
  fi
else
  note "worktree already gone"
fi

if git show-ref --verify --quiet "refs/heads/$SLUG" && [ "$FORCE" -eq 0 ]; then
  # Deliberately NOT measured against @{upstream}: agent-worktree.sh sets that to
  # origin/main, so "unpushed" would fire on a branch that is pushed perfectly
  # well and the message would be a lie. Compare against the branch's own remote
  # when it has one.
  if git show-ref --verify --quiet "refs/remotes/origin/$SLUG"; then
    ahead="$(git rev-list --count "origin/$SLUG..$SLUG" 2>/dev/null || echo 0)"
    [ "$ahead" != "0" ] &&
      die "$SLUG has $ahead commit(s) not on origin/$SLUG. Push them, or re-run with --force to discard."
  else
    ahead="$(git rev-list --count "origin/main..$SLUG" 2>/dev/null || echo 0)"
    [ "$ahead" != "0" ] &&
      die "$SLUG has $ahead commit(s) and has never been pushed. Push it, or re-run with --force to discard."
  fi

  git merge-base --is-ancestor "$SLUG" origin/main 2>/dev/null ||
    die "$SLUG is not merged into origin/main. Merge it, or re-run with --force to discard."
fi

# ---------------------------------------------------------------- do it

# Each step tolerates already having happened, so a half-finished cleanup can be
# re-run rather than unpicked by hand.
if [ -d "$WT" ]; then
  if [ "$FORCE" -eq 1 ]; then
    git worktree remove --force "$WT"
  else
    git worktree remove "$WT"
  fi
  note "removed worktree $WT"
fi

git worktree prune
note "pruned stale worktree entries"

if git show-ref --verify --quiet "refs/heads/$SLUG"; then
  git branch -D "$SLUG" >/dev/null
  note "deleted local branch $SLUG"
fi

if git ls-remote --exit-code --heads origin "$SLUG" >/dev/null 2>&1; then
  git push --quiet origin --delete "$SLUG"
  note "deleted origin/$SLUG"
fi

printf '✓ %s cleaned up\n' "$SLUG"
