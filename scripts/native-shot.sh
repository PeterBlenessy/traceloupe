#!/usr/bin/env bash
# Capture the REAL native app window (incl. macOS traffic lights) via System
# Events bounds + screencapture. Needs Accessibility (System Events) + Screen
# Recording granted to the controlling terminal app. Usage: native-shot.sh [out.png] [pid]
set -euo pipefail
OUT="${1:-/tmp/traceloupe-native.png}"
PID="${2:-$(pgrep -f 'target/debug/traceloupe' | head -1)}"
[ -z "${PID:-}" ] && { echo "app not running"; exit 1; }
osascript -e "tell application \"System Events\" to set frontmost of (first process whose unix id is $PID) to true" 2>&1
sleep 0.6
BOUNDS=$(osascript -e "tell application \"System Events\" to tell (first process whose unix id is $PID) to get {position, size} of window 1")
echo "bounds: $BOUNDS"
# BOUNDS like "x, y, w, h"
read X Y W H < <(echo "$BOUNDS" | tr -d ' ' | awk -F, '{print $1, $2, $3, $4}')
screencapture -o -x -R "${X},${Y},${W},${H}" "$OUT"
echo "wrote $OUT ($(file -b "$OUT" 2>/dev/null))"
