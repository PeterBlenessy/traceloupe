#!/usr/bin/env bash
#
# preflight.sh — run the whole CI gate locally, in CI's order.
#
#   scripts/preflight.sh              # everything except the browser checks
#   scripts/preflight.sh --with-ui    # …plus the design lint (starts its own vite)
#
# Why this exists: the gate is eight commands, and running them by hand means
# remembering all eight in the right order every time. That went wrong twice in
# one session — once a check was skipped because it lived on an unmerged branch,
# once a `cd X && python3 …` short-circuited so an edit never happened while
# `pnpm build` still reported success and the run looked green.
#
# So: one command, every check, and the summary says which ones actually RAN.
# A gate that quietly skips a step reports the same "OK" as a gate that passed
# it — the failure this repo keeps meeting from the other direction (see
# "A check is not trusted until it has been seen failing" in AGENTS.md).
#
set -uo pipefail

cd "$(dirname "$0")/.."

WITH_UI=0
[ "${1:-}" = "--with-ui" ] && WITH_UI=1

RESULTS=()
FAILED=0

run() {
  local name="$1"; shift
  printf '\n\033[1m▸ %s\033[0m\n' "$name"
  if "$@"; then
    RESULTS+=("ok    $name")
  else
    RESULTS+=("FAIL  $name")
    FAILED=1
  fi
}

# --- the sidecar the Tauri crate needs to compile at all -------------------
#
# Git-ignored, so a fresh worktree does not have it and `cargo check -p
# traceloupe` fails for a reason that has nothing to do with your change.
BIN="src-tauri/binaries/llama-server-aarch64-apple-darwin"
if [ ! -f "$BIN" ]; then
  GIT_COMMON="$(cd "$(git rev-parse --git-common-dir)" && pwd)"
  MAIN_ROOT="$(dirname "$GIT_COMMON")"
  if [ -f "$MAIN_ROOT/$BIN" ]; then
    cp -R "$MAIN_ROOT/src-tauri/binaries/." src-tauri/binaries/
    printf '  (copied the llama-server sidecar from the main checkout)\n'
  fi
fi

# --- hygiene ---------------------------------------------------------------
run "releases"        bash scripts/check-releases.sh
run "doc links"       bash scripts/check-doc-links.sh
run "no backup stats" node scripts/check-no-backup-stats.mjs
run "app parser media" node scripts/check-app-parser-coverage.mjs
run "coverage map"    python3 tools/coverage-gap.py --self-test
run "dates"           node scripts/check-dates.mjs
run "mock parity"     node scripts/check-mock-parity.mjs
run "list scroll"     node scripts/check-list-scroll.mjs
run "surfaces"        node scripts/check-artifact-surfaces.mjs
run "overlap"         node scripts/check-artifact-overlap.mjs

# --- rust ------------------------------------------------------------------
run "fmt"             cargo fmt --all -- --check
run "core tests"      cargo test -p traceloupe-core
run "shell tests"     cargo test -p traceloupe
run "clippy"          cargo clippy --workspace --all-targets -- -D warnings

# --- frontend --------------------------------------------------------------
run "typecheck"       npx tsc --noEmit
run "build"           pnpm build

# --- the browser checks, opt-in because they start a dev server ------------
if [ "$WITH_UI" -eq 1 ]; then
  PORT="${PREFLIGHT_PORT:-1490}"
  npx vite --port "$PORT" >/tmp/preflight-vite.log 2>&1 &
  VITE_PID=$!
  trap 'kill "$VITE_PID" 2>/dev/null || true' EXIT
  for _ in $(seq 1 40); do
    curl -s -o /dev/null "http://localhost:$PORT/" && break
    sleep 1
  done
  run "design lint"   env BASE="http://localhost:$PORT" node scripts/check-design.mjs
  run "encrypted-empty" node scripts/check-encrypted-empty.mjs "http://localhost:$PORT"
  run "parse-failed"   node scripts/check-parse-failed.mjs "http://localhost:$PORT"
  run "filtered-empty" node scripts/check-filtered-empty.mjs "http://localhost:$PORT"
  run "clickable"      node scripts/check-clickable.mjs "http://localhost:$PORT"
  run "view-intro"     node scripts/check-view-intro.mjs "http://localhost:$PORT"
  run "view scroll"    node scripts/check-view-scroll.mjs "http://localhost:$PORT"
else
  RESULTS+=("skip  design lint (pass --with-ui)")
  RESULTS+=("skip  encrypted-empty (pass --with-ui)")
  RESULTS+=("skip  parse-failed (pass --with-ui)")
  RESULTS+=("skip  filtered-empty (pass --with-ui)")
  RESULTS+=("skip  clickable (pass --with-ui)")
  RESULTS+=("skip  view-intro (pass --with-ui)")
fi

# --- say what ran, not just whether it passed ------------------------------
printf '\n\033[1mpreflight\033[0m\n'
for r in "${RESULTS[@]}"; do
  case "$r" in
    FAIL*) printf '  \033[31m%s\033[0m\n' "$r" ;;
    skip*) printf '  \033[33m%s\033[0m\n' "$r" ;;
    *)     printf '  \033[32m%s\033[0m\n' "$r" ;;
  esac
done

if [ "$FAILED" -eq 1 ]; then
  printf '\n\033[31m✗ preflight failed — CI will too.\033[0m\n'
  exit 1
fi
printf '\n\033[32m✓ preflight clean.\033[0m\n'
[ "$WITH_UI" -eq 0 ] && printf '  Run with --with-ui before opening a PR that touches the interface.\n'
exit 0
