#!/usr/bin/env bash
# Refresh the bundled indicator snapshot for the Security Check.
#
# Downloads the iMazing STIX index and every bundle it lists, plus Echap's
# ioc.yaml / watchware.yaml, into crates/traceloupe-core/resources/indicators/
# and writes manifest.json describing each feed (source label, class, URL).
# The snapshot is committed so builds are reproducible and scans work with
# zero network. Feeds are CC-BY: attribution lives in ATTRIBUTION.md and must
# be surfaced in the scan UI and exported reports.
#
# Usage: scripts/update-indicator-snapshot.sh
set -euo pipefail

cd "$(dirname "$0")/.."
DEST=crates/traceloupe-core/resources/indicators
INDEX_URL=https://raw.githubusercontent.com/DigiDNA/iMazing-Indicators-Of-Compromise/main/imazing_stix_files.json
ECHAP_RAW=https://raw.githubusercontent.com/AssoEchap/stalkerware-indicators/master

mkdir -p "$DEST"
rm -f "$DEST"/*.stix2 "$DEST"/*.yaml "$DEST"/manifest.json

manifest_entries=()

fetch() { # url dest
  curl -sfL --retry 3 -o "$2" "$1"
}

# --- STIX bundles from the iMazing index -----------------------------------
echo "fetching index: $INDEX_URL"
index_json=$(curl -sfL --retry 3 "$INDEX_URL")

while IFS= read -r url; do
  file=$(basename "$url")
  case "$url" in
    # Echap's generated STIX collapses the website/C2 severity distinction —
    # we load their YAML instead (below), so skip the STIX rendering.
    *stalkerware-indicators*) echo "skip (using Echap YAML): $file"; continue ;;
  esac
  # Source label: repo owner + file stem, e.g. "AmnestyTech/pegasus".
  owner=$(echo "$url" | sed -E 's#https://raw.githubusercontent.com/([^/]+)/.*#\1#')
  echo "fetching $owner/$file"
  fetch "$url" "$DEST/$file"
  manifest_entries+=("{\"file\": \"$file\", \"source\": \"$owner/${file%.stix2}\", \"class\": \"mercenary\", \"format\": \"stix2\", \"url\": \"$url\"}")
done < <(echo "$index_json" | python3 -c "import json,sys; [print(u) for u in json.load(sys.stdin)['stix_urls']]")

# --- Echap YAML feeds -------------------------------------------------------
for f in ioc.yaml watchware.yaml; do
  echo "fetching echap/$f"
  fetch "$ECHAP_RAW/$f" "$DEST/$f"
done
manifest_entries+=('{"file": "ioc.yaml", "source": "echap/ioc", "class": "stalkerware", "format": "echap_yaml", "url": "'"$ECHAP_RAW"'/ioc.yaml"}')
manifest_entries+=('{"file": "watchware.yaml", "source": "echap/watchware", "class": "watchware", "format": "echap_yaml", "url": "'"$ECHAP_RAW"'/watchware.yaml"}')

# --- manifest.json ----------------------------------------------------------
{
  echo '{'
  echo "  \"generated_at\": \"$(date -u +%Y-%m-%dT%H:%M:%SZ)\","
  echo '  "feeds": ['
  printf '    %s' "${manifest_entries[0]}"
  for e in "${manifest_entries[@]:1}"; do printf ',\n    %s' "$e"; done
  echo ''
  echo '  ]'
  echo '}'
} > "$DEST/manifest.json"
python3 -m json.tool "$DEST/manifest.json" > /dev/null   # validate

cat > "$DEST/ATTRIBUTION.md" <<'EOF'
# Indicator feed attribution

The bundled indicators of compromise are published by third parties under
Creative Commons licenses requiring attribution:

- **Amnesty International Security Lab** — investigations indicators
  (github.com/AmnestyTech/investigations), CC-BY 2.0.
- **MVT project** — mvt-indicators (github.com/mvt-project/mvt-indicators),
  maintained by the Mobile Verification Toolkit project.
- **Echap** — stalkerware-indicators (github.com/AssoEchap/stalkerware-indicators),
  CC-BY 4.0.
- **DigiDNA** — iMazing Indicators of Compromise index
  (github.com/DigiDNA/iMazing-Indicators-Of-Compromise).

This attribution must appear wherever scan results are shown or exported.
EOF

echo "snapshot written to $DEST:"
ls -la "$DEST"
