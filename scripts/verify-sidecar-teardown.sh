#!/usr/bin/env bash
# Issue #31, end-to-end against the REAL sidecar: tear the app down mid-
# inference the two ways a dev session actually goes away, and check the three
# things that were wrong.
#
#   1. the sidecar is gone      (it used to survive, reparented to launchd)
#   2. it never shares our process group  (a graceful signal aborts or wedges it)
#   3. macOS filed no crash report for it
#
# Needs a staged sidecar (`pnpm setup:llama`) and a real model — so this is a
# hardware check you run by hand, not a CI gate. The mechanism itself is
# covered in CI by the safety_scan::{server,reaper} unit tests.
#
# Usage: scripts/verify-sidecar-teardown.sh [/path/to/model.gguf]
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MODEL="${1:-$HOME/Library/Application Support/se.addable.traceloupe.dev/models/gemma-4-E2B-it-Q4_K_M.gguf}"
REPORTS="$HOME/Library/Logs/DiagnosticReports"

if [ ! -f "$MODEL" ]; then
  echo "ERROR: no model at $MODEL — pass one as \$1" >&2
  exit 1
fi

echo "▶ building the harness…"
cargo build -p traceloupe-core --example sidecar_teardown --quiet || exit 1

FAIL=0

# $1 = mode (signal|exit), $2 = human description
run_scenario() {
  local mode="$1" desc="$2"
  local before after log app sidecar app_pgid side_pgid
  before=$(ls "$REPORTS" 2>/dev/null | grep -c llama-server)
  log=$(mktemp)

  echo
  echo "── $desc (mode: $mode)"

  # Launch the harness in its OWN process group (macOS ships no `setsid`), so a
  # group SIGTERM is a faithful stand-in for a dev-session teardown without
  # taking this script — or your shell — with it.
  python3 -c 'import os,sys; os.setpgrp(); os.execv(sys.argv[1], sys.argv[1:])' \
    "$ROOT/target/debug/examples/sidecar_teardown" "$MODEL" "$mode" >"$log" 2>&1 &
  app=$!

  # Read BOTH pgids at the same moment — once the sidecar pid is out, which is
  # after setpgrp() and before any teardown, so both processes are certainly
  # alive and settled. Reading either one too early (before setpgrp) or too
  # late (after exit) yields a stale or empty value, and a stale/empty value
  # compares "not equal" to anything — a check that observes nothing and
  # reports success. Hence the explicit non-empty assertions below.
  for _ in $(seq 1 300); do
    grep -q SIDECAR_PID= "$log" && break
    kill -0 "$app" 2>/dev/null || { echo "  ✗ harness died early:"; cat "$log"; rm -f "$log"; FAIL=1; return; }
    sleep 1
  done
  sidecar=$(grep -m1 SIDECAR_PID= "$log" | cut -d= -f2)
  app_pgid=$(ps -o pgid= -p "$app" 2>/dev/null | tr -d ' ')
  side_pgid=$(ps -o pgid= -p "$sidecar" 2>/dev/null | tr -d ' ')
  echo "  harness pid=$app pgid=${app_pgid:-<unread>} | sidecar pid=$sidecar pgid=${side_pgid:-<unread>}"

  if [ -z "$app_pgid" ] || [ -z "$side_pgid" ]; then
    echo "  ✗ could not read both process groups — the check observed nothing"
    FAIL=1
  elif [ "$app_pgid" != "$side_pgid" ]; then
    echo "  ✓ sidecar is in its own process group"
  else
    echo "  ✗ sidecar SHARES our process group — a group signal reaches it and it aborts or wedges"
    FAIL=1
  fi

  # Only now wait for inference to be in flight, so the teardown lands where
  # the crash report says it landed.
  for _ in $(seq 1 300); do
    grep -q GENERATING "$log" && break
    kill -0 "$app" 2>/dev/null || { echo "  ✗ harness died early:"; cat "$log"; rm -f "$log"; FAIL=1; return; }
    sleep 1
  done

  if [ "$mode" = "signal" ]; then
    echo "  ▸ SIGTERM to the harness's process group, mid-generation"
    kill -TERM -"$app_pgid" 2>/dev/null
  else
    echo "  ▸ harness quits under its own scan thread (no signal, no Drop)"
  fi
  wait "$app" 2>/dev/null

  # Give the reaper's SIGKILL time to land and the OS time to file any report.
  sleep 5

  if kill -0 "$sidecar" 2>/dev/null; then
    echo "  ✗ sidecar $sidecar SURVIVED the teardown (state $(ps -o stat= -p "$sidecar" | tr -d ' ')) — orphaned GPU server"
    kill -9 "$sidecar" 2>/dev/null
    FAIL=1
  else
    echo "  ✓ sidecar is gone"
  fi

  after=$(ls "$REPORTS" 2>/dev/null | grep -c llama-server)
  if [ "$after" -gt "$before" ]; then
    echo "  ✗ macOS filed a new llama-server crash report ($before → $after)"
    FAIL=1
  else
    echo "  ✓ no new crash report"
  fi
  rm -f "$log"
}

run_scenario signal "Ctrl-C / closing terminal / logout — a signal at our process group"
run_scenario exit   "window-close quit while a scan thread holds the server"

echo
[ "$FAIL" -eq 0 ] && echo "▶ PASS" || echo "▶ FAIL"
exit "$FAIL"
