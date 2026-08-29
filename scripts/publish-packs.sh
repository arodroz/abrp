#!/bin/bash
# Publish a region's pack artifacts to the pack bucket per ADR 0011
# (wayfinder #49): immutable epoch-addressed artifact dir plus the two
# mutable pointers (per-region catalog.json, top-level index.json).
#
# usage: publish-packs.sh <region> [dist-dir]
#
# Expects an rclone remote with write access to the wayfinder-packs bucket;
# override with WAYFINDER_PUBLISH_REMOTE (codebase audit H-06) to point at a
# different rclone remote, including a plain local directory, for testing --
# rclone treats a plain local path as a valid remote.
#
# Single-publisher assumption (H-06): this is a homelab with one publisher.
# Concurrent runs on this Mac are serialized with a local mkdir-based lock
# (flock isn't available on macOS by default); it does not, and is not meant
# to, protect against two different machines publishing at once.
set -euo pipefail

REGION="${1:?usage: publish-packs.sh <region> [dist-dir]}"
DIST="${2:-$HOME/abrp-data/dist/$REGION}"
REMOTE="${WAYFINDER_PUBLISH_REMOTE:-wayfinder-garage:wayfinder-packs}"
CAT="$DIST/catalog.json"
[ -f "$CAT" ] || { echo "no catalog.json in $DIST" >&2; exit 1; }

TMP=$(mktemp -d)
LOCK_DIR="${WAYFINDER_PUBLISH_LOCK_DIR:-${TMPDIR:-/tmp}/wayfinder-publish-packs.lock}"
LOCK_TIMEOUT="${WAYFINDER_PUBLISH_LOCK_TIMEOUT:-600}"   # seconds; single-publisher homelab
LOCK_ACQUIRED=0

cleanup() {
  rm -rf "$TMP"
  # Only remove the lock dir if this run is the one that created it -- never
  # clean up a lock held by another still-running publish.
  [ "$LOCK_ACQUIRED" = "1" ] && rmdir "$LOCK_DIR" 2>/dev/null
  return 0
}
trap cleanup EXIT

waited=0
until mkdir "$LOCK_DIR" 2>/dev/null; do
  [ "$waited" -eq 0 ] && echo "waiting for publish lock ($LOCK_DIR) -- another publish-packs.sh may be running on this Mac..." >&2
  if [ "$waited" -ge "$LOCK_TIMEOUT" ]; then
    echo "ERROR: could not acquire publish lock at $LOCK_DIR after ${LOCK_TIMEOUT}s" >&2
    exit 1
  fi
  sleep 1
  waited=$((waited + 1))
done
LOCK_ACQUIRED=1

EPOCH=$(jq -r .osm_snapshot_epoch "$CAT")
NAME=$(jq -r .region_name "$CAT")
DEST="packs/$REGION/$EPOCH"

# --- Phase 1: immutable epoch-addressed objects -----------------------------
# Files under an epoch path never change once uploaded -- a resumed range
# download must never splice two versions. Before uploading each object,
# check whether it already exists remotely by sha256: identical -> skip
# (idempotent republish); different -> ABORT loudly, never overwrite.
# --immutable is passed on the upload itself as a second line of defense.
FILES="$(jq -r '.artifacts[].file' "$CAT") style-light.json style-dark.json catalog.json"
# MD5, not sha256, for the remote equality checks: Garage's S3 backend supports
# only MD5 through rclone ("hash type not supported" for sha256 -- and rclone
# hashsum exits 0 even then, printing no hash, so an unsupported type would
# silently read as "object absent"). rclone stores the true MD5 in object
# metadata even for multipart uploads, so this works for the multi-GB packs.
# This check prevents ACCIDENTAL epoch overwrites; content integrity for
# clients stays anchored on the catalog's client-verified sha256 (ADR 0011).
for f in $FILES; do
  LOCAL="$DIST/$f"
  [ -f "$LOCAL" ] || { echo "missing $LOCAL" >&2; exit 1; }
  LOCAL_HASH="$(rclone hashsum md5 "$LOCAL" 2>/dev/null | awk '{print $1}')" || true
  [ -n "$LOCAL_HASH" ] || { echo "ABORT: could not compute local md5 for $LOCAL" >&2; exit 1; }

  REMOTE_OBJ="$REMOTE/$DEST/$f"
  # Existence must come from lsf's OUTPUT: rclone lsf exits 0 for a missing
  # object (empty listing), so the exit code alone can't distinguish absent
  # from present.
  REMOTE_EXISTS="$(rclone lsf "$REMOTE_OBJ" 2>/dev/null)" || true
  if [ -n "$REMOTE_EXISTS" ]; then
    REMOTE_HASH="$(rclone hashsum md5 "$REMOTE_OBJ" 2>/dev/null | awk '{print $1}')" || true
    [ -n "$REMOTE_HASH" ] || { echo "ABORT: $DEST/$f exists in $REMOTE but its md5 could not be read -- refusing to guess whether it matches" >&2; exit 1; }
    if [ "$LOCAL_HASH" = "$REMOTE_HASH" ]; then
      echo "epoch object $DEST/$f already published and identical, skipping upload"
      continue
    fi
    echo "ABORT: $DEST/$f already exists in $REMOTE and differs from the local build (remote md5 $REMOTE_HASH != local $LOCAL_HASH) -- epoch objects are immutable, NOT overwritten" >&2
    exit 1
  fi

  echo "uploading $f -> $DEST/$f"
  rclone copyto "$LOCAL" "$REMOTE_OBJ" --s3-chunk-size 64M --immutable
  VERIFY_HASH="$(rclone hashsum md5 "$REMOTE_OBJ" 2>/dev/null | awk '{print $1}')" || true
  [ "$VERIFY_HASH" = "$LOCAL_HASH" ] || { echo "ABORT: post-upload verification failed for $DEST/$f (remote md5 $VERIFY_HASH != local $LOCAL_HASH)" >&2; exit 1; }
done
echo "all epoch objects for $DEST uploaded and verified"

# --- Phase 2: mutable pointer 1 -- packs/<region>/catalog.json -------------
# Only reached once every immutable epoch object above is confirmed in place.
jq --arg base "$DEST" '.artifacts |= with_entries(.value += {path: ($base + "/" + .value.file)})' \
  "$CAT" > "$TMP/catalog.json"
jq -e . "$TMP/catalog.json" >/dev/null || { echo "ABORT: generated region catalog.json is not valid JSON" >&2; exit 1; }
rclone copyto "$TMP/catalog.json" "$REMOTE/packs/$REGION/catalog.json"

# --- Phase 3: mutable pointer 2 -- packs/index.json (upsert this region) ---
# A recoverable read/parse failure must never degrade to an empty index --
# that would silently drop every other region. Existence is checked first;
# if it exists, the read AND the jq -e parse must both succeed or the script
# ABORTS. Only a genuinely absent index (first publish ever) starts fresh.
# Same lsf-output existence check as above: exit code 0 does not mean present.
if [ -n "$(rclone lsf "$REMOTE/packs/index.json" 2>/dev/null)" ]; then
  rclone cat "$REMOTE/packs/index.json" > "$TMP/index.json" \
    || { echo "ABORT: packs/index.json exists in $REMOTE but could not be read" >&2; exit 1; }
  jq -e . "$TMP/index.json" >/dev/null 2>&1 \
    || { echo "ABORT: packs/index.json exists in $REMOTE but is not valid JSON -- refusing to replace it with an empty index" >&2; exit 1; }
else
  echo '{"index_format":1,"regions":[]}' > "$TMP/index.json"
fi
TOTAL=$(jq '[.artifacts[].bytes] | add' "$CAT")
jq --arg id "$REGION" --arg name "$NAME" --argjson epoch "$EPOCH" --argjson total "$TOTAL" \
  '.regions |= (map(select(.id != $id)) + [{id: $id, name: $name, latest_epoch: $epoch, total_bytes: $total}] | sort_by(.id))' \
  "$TMP/index.json" > "$TMP/index-new.json"
jq -e . "$TMP/index-new.json" >/dev/null || { echo "ABORT: generated index.json is not valid JSON" >&2; exit 1; }
rclone copyto "$TMP/index-new.json" "$REMOTE/packs/index.json"

echo "published $REGION epoch $EPOCH ($TOTAL bytes) to $DEST"
