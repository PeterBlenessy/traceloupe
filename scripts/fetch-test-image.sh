#!/usr/bin/env bash
# Fetch a public DFIR iOS image for validating artifact modules against real
# store shapes, and extract single files from it without unpacking 22 GB.
#
#   scripts/fetch-test-image.sh                     # download (resumable)
#   scripts/fetch-test-image.sh --extract 'PATTERN' # pull matching files out
#   scripts/fetch-test-image.sh --list 'PATTERN'    # just show what matches
#
# WHY A FULL-FILESYSTEM IMAGE IS SAFE TO USE HERE, AND WHAT IT CANNOT DO
#
# These images are full-filesystem extractions: they contain everything on the
# device, INCLUDING what a backup deliberately excludes. So they are evidence of
# **where a file lives and what shape it has** — never evidence that a file is in
# a backup. Backup membership is decided by Apple's `Domains.plist`, committed at
# tools/data/ios-backup-domains.json and applied by
# tools/classify-ileapp-artifacts.py. Do not use this image to claim
# reachability; see docs/reference/backup-coverage-audit.md.
#
# The owner's own backup is off-limits (AGENTS.md). This is the sanctioned
# substitute.
#
# SOURCE AND TERMS
#
# Joshua Hickman's public research images, hosted by Digital Corpora, which
# states its images "are freely available and may be used without prior
# authorization or IRB approval".
#   index: https://thebinaryhick.blog/public_images/
#   docs:  https://digitalcorpora.s3.amazonaws.com/corpora/mobile/iOS17/iOS17-ImageCreation.pdf
set -uo pipefail

DEST_DIR="${TRACELOUPE_TEST_IMAGES:-$HOME/Development/traceloupe-test-images}"
NAME="iOS_17_Public_Image.tar.gz"
URL="https://digitalcorpora.s3.amazonaws.com/corpora/mobile/iOS17/$NAME"
ARCHIVE="$DEST_DIR/$NAME"
EXPECT_BYTES=22132295131

mkdir -p "$DEST_DIR"

have_whole_archive() {
  [ -f "$ARCHIVE" ] || return 1
  local size
  size=$(stat -f%z "$ARCHIVE" 2>/dev/null || stat -c%s "$ARCHIVE" 2>/dev/null || echo 0)
  [ "$size" -eq "$EXPECT_BYTES" ]
}

# bsdtar's --fast-read stops at the first match instead of reading to the end of
# the archive, which is the difference between seconds and 22 GB.
extract() {
  local mode="$1" pattern="$2"
  if ! have_whole_archive; then
    echo "ERROR: $ARCHIVE is missing or incomplete — run without --extract first." >&2
    exit 1
  fi
  case "$mode" in
    list) tar -tzf "$ARCHIVE" | grep -iE "$pattern" | head -50 ;;
    extract)
      local out="$DEST_DIR/extracted"
      mkdir -p "$out"
      echo "▶ extracting files matching: $pattern"
      # No --fast-read: a pattern may legitimately match several files.
      tar -xzf "$ARCHIVE" -C "$out" --include "$pattern" 2>/dev/null
      find "$out" -type f -newermt '-5 minutes' | head -20
      ;;
  esac
}

case "${1:-}" in
  --list)    extract list "${2:?usage: --list PATTERN}" ; exit 0 ;;
  --extract) extract extract "${2:?usage: --extract PATTERN}" ; exit 0 ;;
esac

if have_whole_archive; then
  echo "✓ already downloaded: $ARCHIVE"
  exit 0
fi

echo "▶ downloading $NAME (~22 GB) to $DEST_DIR"
echo "  resumable — re-run this script if it is interrupted."
curl -L -C - --progress-bar -o "$ARCHIVE" "$URL" || {
  echo "download interrupted; re-run to resume." >&2
  exit 1
}

if have_whole_archive; then
  echo "✓ complete."
  echo
  echo "Next, pull out just what you need — for example Apple's own backup-domain"
  echo "definitions, to check our iOS 16.4 transcription against a real 17.x copy:"
  echo "  scripts/fetch-test-image.sh --list 'Domains.plist'"
else
  echo "⚠ size mismatch — the archive is incomplete. Re-run to resume." >&2
  exit 1
fi
