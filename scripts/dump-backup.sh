#!/usr/bin/env bash
#
# Dev helper: decrypt every data file (databases, plists, json, …) from a local
# iPhone backup into a mirror we can inspect while writing native parsers, so we
# never have to guess a schema. Media blobs are skipped unless you pass `all`.
#
# Usage:
#   scripts/dump-backup.sh [backup_udid] [all]
#
# - backup_udid: optional. Defaults to the single backup under MobileSync/Backup;
#   required only if you have more than one.
# - all: optional. Include photo/video blobs too (large). Omit for data files only.
#
# The password is asked for via a macOS dialog (hidden input) and passed to the
# extractor through an env var — it is never written to disk or shell history.

set -euo pipefail

BACKUP_ROOT="$HOME/Library/Application Support/MobileSync/Backup"
DEST="$HOME/.traceloupe-dev/backup-mirror"
LOG="$HOME/.traceloupe-dev/extract.log"

# --- resolve which backup ------------------------------------------------------
udid="${1:-}"
mode="${2:-data}"
if [[ -z "$udid" ]]; then
  mapfile -t backups < <(find "$BACKUP_ROOT" -maxdepth 2 -name Manifest.db -print 2>/dev/null | xargs -n1 dirname 2>/dev/null)
  if [[ ${#backups[@]} -eq 0 ]]; then
    echo "No backup found under: $BACKUP_ROOT" >&2
    exit 1
  elif [[ ${#backups[@]} -gt 1 ]]; then
    echo "Multiple backups found — pass a UDID as the first argument:" >&2
    printf '  %s\n' "${backups[@]##*/}" >&2
    exit 1
  fi
  BACKUP_DIR="${backups[0]}"
else
  BACKUP_DIR="$BACKUP_ROOT/$udid"
fi
echo "Backup:  $BACKUP_DIR"
echo "Mirror:  $DEST"
echo "Log:     $LOG"

# --- ask for the password (macOS dialog, hidden) -------------------------------
PW="$(osascript \
  -e 'display dialog "iPhone backup password (for TraceLoupe extraction)" default answer "" with hidden answer' \
  -e 'text returned of result')"

mkdir -p "$HOME/.traceloupe-dev"

# `all` as the 3rd arg to the example includes media; otherwise data files only.
extra=""
[[ "$mode" == "all" ]] && extra="all"

echo "Extracting… (this can take a minute or two)"
TRACELOUPE_BACKUP_PASSWORD="$PW" cargo run -q -p traceloupe-core --example dump_backup -- \
  "$BACKUP_DIR" "$DEST" $extra >"$LOG" 2>&1 || {
  echo "Extraction failed — see $LOG" >&2
  tail -n 20 "$LOG" >&2 || true
  exit 1
}
unset PW TRACELOUPE_BACKUP_PASSWORD

echo "Done. Summary:"
tail -n 3 "$LOG"
