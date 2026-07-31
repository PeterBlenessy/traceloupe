#!/usr/bin/env bash
# Fetch a public DFIR research image and keep ONLY the backup inside it.
#
#   scripts/fetch-test-image.sh --list            # what exists, and what is here
#   scripts/fetch-test-image.sh ios17             # fetch + unpack + delete archive
#   scripts/fetch-test-image.sh --prune           # reclaim space from what is here
#   scripts/fetch-test-image.sh --paths ios17 PAT # grep a local backup's manifest
#
# WHY ONLY THE BACKUP
#
# These are full-filesystem extractions: a ~22 GB archive yielding ~34 GB of FFS
# plus a ~2 GB backup. This app reads BACKUPS. The FFS half is evidence of where a
# file lives and what shape it has — never evidence that a file is IN a backup,
# which is decided by Apple's `Domains.plist` (tools/data/ios-backup-domains.json,
# applied by tools/classify-ileapp-artifacts.py). So the 34 GB answers a question
# we do not ask, and the archive answers nothing once unpacked.
#
# That is not a saving for its own sake. Disk is the binding constraint on how many
# devices we can validate against, and DEVICES are the binding constraint on how
# much of iLEAPP's artifact list we can even classify — 72 of its artifacts declare
# rootless globs that no amount of reasoning resolves, only a device with the app
# installed (see tools/coverage-gap.py). Trading 56 GB per device for 2 GB is what
# makes a corpus possible at all.
#
# WHAT IS SAFE TO DELETE, AND WHAT IS NOT
#
# The unpacked backup is the artifact. The archive is re-downloadable and the FFS
# tree answers no question we ask, so `--prune` removes both — and never touches an
# unpacked backup, because deleting one means re-downloading 22 GB to get 2 GB back.
#
# The owner's own backup is off-limits (AGENTS.md). This exists so it is never the
# convenient option.
#
# SOURCE AND TERMS
#
# Joshua Hickman's public research images, hosted by Digital Corpora, which states
# its images "are freely available and may be used without prior authorization or
# IRB approval".
#   index: https://thebinaryhick.blog/public_images/
# The catalogue of what exists lives in tools/data/dfir-images.json.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CATALOG="$ROOT/tools/data/dfir-images.json"
DEST_DIR="${TRACELOUPE_TEST_IMAGES:-$HOME/Development/traceloupe-test-images}"

die() { echo "✗ $*" >&2; exit 1; }
command -v python3 >/dev/null 2>&1 || die "python3 is required to read $CATALOG"

field() { # id key
  python3 -c "
import json,sys
d=json.load(open('$CATALOG'))
for i in d['images']:
    if i['id']==sys.argv[1]:
        v=i.get(sys.argv[2])
        print('' if v is None else v); break
else:
    sys.exit(1)
" "$1" "$2"
}

# An unpacked backup is a directory holding a Manifest.db. Found by SHAPE rather
# than from a recorded list, so one someone unpacked by hand counts too — and a
# stale list can never claim we have a device we do not.
local_backups() {
  find "$DEST_DIR" -maxdepth 6 -name Manifest.db -not -path '*/extracted/*' 2>/dev/null \
    | while read -r m; do dirname "$m"; done | sort -u
}

# Archives and FFS trees: re-downloadable, or answering nothing we ask.
prune_targets() {
  find "$DEST_DIR" -maxdepth 3 \( -name '*.tar.gz' -o -name '*.zip' \) 2>/dev/null
  find "$DEST_DIR" -maxdepth 3 -type d -name '*Extraction*' 2>/dev/null
}

human() {
  python3 -c "
n=float('${1:-0}' or 0)
for u in ('B','KB','MB','GB','TB'):
    if n < 1024: print(f'{n:.0f} {u}'); break
    n/=1024
else: print(f'{n:.0f} PB')"
}

cmd_list() {
  echo "catalogued images ($(basename "$CATALOG")):"
  python3 -c "
import json
for i in json.load(open('$CATALOG'))['images']:
    s = i.get('archive_bytes')
    s = f\"{s/1e9:.0f} GB\" if s else '? GB'
    print(f\"  {i['id']:8} iOS {i['ios']:6} {i['device']:12} {s:7} {i['notes'][:62]}\")
"
  echo
  echo "unpacked backups on disk ($DEST_DIR):"
  local found=0 b
  while read -r b; do
    [ -n "$b" ] || continue
    found=1
    printf '  %-6s %s\n' "$(du -sh "$b" 2>/dev/null | cut -f1)" "$b"
  done < <(local_backups)
  [ "$found" = 0 ] && echo "  (none — run: $(basename "$0") ios17)"

  local kb=0 p
  while read -r p; do
    [ -e "$p" ] || continue
    kb=$((kb + $(du -sk "$p" 2>/dev/null | cut -f1)))
  done < <(prune_targets)
  if [ "$kb" -gt 0 ]; then
    echo
    echo "reclaimable by --prune: $(human $((kb * 1024)))"
  fi
  echo
  echo "free: $(df -h "$DEST_DIR" | tail -1 | awk '{print $4}')"
}

cmd_prune() {
  local any=0 p
  while read -r p; do
    [ -e "$p" ] || continue
    any=1
    echo "  removing $(du -sh "$p" 2>/dev/null | cut -f1)  ${p#"$DEST_DIR"/}"
    rm -rf "$p"
  done < <(prune_targets)
  [ "$any" = 0 ] && echo "nothing to prune."
  echo "✓ unpacked backups untouched — they are the artifact, and re-fetching one costs 22 GB."
}

cmd_fetch() {
  local id="${1:?usage: fetch-test-image.sh ID}"
  local url name bytes glob archive out
  url=$(field "$id" url) || die "unknown image '$id' — try --list"
  name=$(field "$id" archive)
  bytes=$(field "$id" archive_bytes)
  glob=$(field "$id" backup_glob)
  archive="$DEST_DIR/$name"
  out="$DEST_DIR/$id"
  mkdir -p "$out"

  if [ -n "$(find "$out" -name Manifest.db -not -path '*/extracted/*' 2>/dev/null | head -1)" ]; then
    echo "✓ $id already has an unpacked backup — nothing to do."
    return 0
  fi

  # Check space FIRST. Finding out there is no room 20 GB into a download is a bad
  # way to find out, and the archive lands beside images we may no longer need.
  # An unrecorded size falls back to 24 GB: larger than any image in the catalogue,
  # so an unknown never under-reserves.
  local need free
  need=$(( ${bytes:-24000000000} / 1024 ))
  [ "${bytes:-0}" -gt 0 ] 2>/dev/null || need=$(( 24000000000 / 1024 ))
  free=$(df -k "$DEST_DIR" | tail -1 | awk '{print $4}')
  if [ "$free" -lt "$need" ]; then
    echo "⚠ $(human $((free * 1024))) free, need about $(human $((need * 1024)))."
    echo "  Run --prune first, or remove a backup you no longer validate against."
    return 1
  fi

  # "(0 B)" is what an uncatalogued size printed, which is a lie in the one place
  # someone checks whether a 22 GB download is worth starting. Say unknown.
  local size_note
  if [ -n "$bytes" ] && [ "$bytes" -gt 0 ] 2>/dev/null; then
    size_note="$(human "$bytes")"
  else
    size_note="size not recorded — expect ~20 GB"
  fi
  echo "▶ downloading $name ($size_note) — resumable, re-run if interrupted"
  curl -L -C - --progress-bar -o "$archive" "$url" || die "download interrupted; re-run to resume."

  echo "▶ extracting only the backup ($glob)"
  tar -xzf "$archive" -C "$out" --include "$glob" 2>/dev/null || true

  local inner
  inner=$(find "$out" -name '*.zip' -path '*Backup*' 2>/dev/null | head -1)
  if [ -n "$inner" ]; then
    echo "▶ unpacking $(basename "$inner")"
    mkdir -p "$out/unpacked"
    unzip -q -o "$inner" -d "$out/unpacked" && rm -f "$inner"
  fi

  local got
  got=$(find "$out" -name Manifest.db -not -path '*/extracted/*' | head -1)
  if [ -z "$got" ]; then
    echo "✗ no Manifest.db under $out — the backup did not come out." >&2
    echo "  Keeping $archive so this can be retried without re-downloading." >&2
    exit 1
  fi

  echo "▶ removing the archive — the backup is out and the archive is re-downloadable"
  rm -f "$archive"
  echo "✓ $id ready: $(dirname "$got")"
  # Record what it actually weighed, so the next person sees a real number in
  # --list instead of "? GB" and the space check stops guessing.
  if [ -z "$bytes" ] || [ "${bytes:-0}" -le 0 ] 2>/dev/null; then
    echo "  note: $name was not sized in $CATALOG — add archive_bytes for it"
  fi
  local pw
  pw=$(field "$id" backup_password)
  [ -n "$pw" ] && echo "  backup password: $pw"
  echo "  free now: $(df -h "$DEST_DIR" | tail -1 | awk '{print $4}')"
}

cmd_paths() { # id pattern
  local id="${1:?usage: --paths ID PATTERN}" pat="${2:?usage: --paths ID PATTERN}"
  local m
  m=$(find "$DEST_DIR/$id" -name Manifest.db -not -path '*/extracted/*' 2>/dev/null | head -1)
  [ -n "$m" ] || die "no unpacked backup for '$id' — fetch it first"
  local pw
  pw=$(field "$id" backup_password)
  ( cd "$ROOT" && cargo run -q -p traceloupe-core --example explore_real_backup -- \
      "$(dirname "$m")" "${pw:--}" list "$pat" )
}

case "${1:-}" in
  ""|--help|-h) sed -n '2,40p' "$0" | sed 's/^#\{1,\} \{0,1\}//' ;;
  --list)  cmd_list ;;
  --prune) cmd_prune ;;
  --paths) shift; cmd_paths "$@" ;;
  -*)      die "unknown option $1 — try --help" ;;
  *)       cmd_fetch "$1" ;;
esac
