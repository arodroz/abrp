#!/bin/bash
# Publish a region's pack artifacts to the pack bucket per ADR 0011
# (wayfinder #49): immutable epoch-addressed artifact dir plus the two
# mutable pointers (per-region catalog.json, top-level index.json).
#
# usage: publish-packs.sh <region> [dist-dir]
#
# Expects an rclone remote named `wayfinder-garage` pointing at the Garage
# S3 API with write access to the `wayfinder-packs` bucket.
set -euo pipefail

REGION="${1:?usage: publish-packs.sh <region> [dist-dir]}"
DIST="${2:-$HOME/abrp-data/dist/$REGION}"
REMOTE="wayfinder-garage:wayfinder-packs"
CAT="$DIST/catalog.json"
[ -f "$CAT" ] || { echo "no catalog.json in $DIST" >&2; exit 1; }

EPOCH=$(jq -r .osm_snapshot_epoch "$CAT")
NAME=$(jq -r .region_name "$CAT")
DEST="packs/$REGION/$EPOCH"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# Immutable epoch dir: the catalog's artifacts, the style templates, and the
# epoch's own catalog.json. Files under an epoch path never change once
# uploaded — resumed range downloads must never splice two versions.
FILES="$(jq -r '.artifacts[].file' "$CAT") style-light.json style-dark.json catalog.json"
for f in $FILES; do
  [ -f "$DIST/$f" ] || { echo "missing $DIST/$f" >&2; exit 1; }
  echo "uploading $f -> $DEST/$f"
  rclone copyto "$DIST/$f" "$REMOTE/$DEST/$f" --s3-chunk-size 64M
done

# Mutable pointer 1: packs/<region>/catalog.json — the epoch catalog with a
# bucket-relative path added per artifact.
jq --arg base "$DEST" '.artifacts |= with_entries(.value += {path: ($base + "/" + .value.file)})' \
  "$CAT" > "$TMP/catalog.json"
rclone copyto "$TMP/catalog.json" "$REMOTE/packs/$REGION/catalog.json"

# Mutable pointer 2: packs/index.json — upsert this region's entry. An
# absent key can come back as an empty 0-exit stream (and jq on empty input
# emits nothing, also with exit 0), so validate the fetch as JSON instead of
# trusting exit codes.
rclone cat "$REMOTE/packs/index.json" > "$TMP/index.json" 2>/dev/null || true
jq -e . "$TMP/index.json" >/dev/null 2>&1 \
  || echo '{"index_format":1,"regions":[]}' > "$TMP/index.json"
TOTAL=$(jq '[.artifacts[].bytes] | add' "$CAT")
jq --arg id "$REGION" --arg name "$NAME" --argjson epoch "$EPOCH" --argjson total "$TOTAL" \
  '.regions |= (map(select(.id != $id)) + [{id: $id, name: $name, latest_epoch: $epoch, total_bytes: $total}] | sort_by(.id))' \
  "$TMP/index.json" > "$TMP/index-new.json"
rclone copyto "$TMP/index-new.json" "$REMOTE/packs/index.json"

echo "published $REGION epoch $EPOCH ($TOTAL bytes) to $DEST"
