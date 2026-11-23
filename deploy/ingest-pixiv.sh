#!/usr/bin/env bash
# Pull new Pixiv bookmarks into fulgorart, end to end:
#   1. download: gallery-dl (fulgorart-gdl) fetches new bookmarks with a
#      metadata sidecar per file, stopping after GDL_ABORT_AFTER files in a
#      row that the archive says are already downloaded (bookmarks come newest
#      first, so a routine run touches only the newest few)
#   2. import:   `cli gallery-dl --remove-imported` stores each file (dedup,
#      R2 upload, tag job) and removes it from staging; problem files stay
#   3. tag:      `cli remote-tagger` tags whatever is queued
# Run from the compose directory (or anywhere: it cd's next to itself).
# Settings come from the environment or ./ingest.env:
#   PIXIV_BOOKMARKS_URL  e.g. https://www.pixiv.net/users/<id>/bookmarks/artworks
#   GDL_ABORT_AFTER      default 20
#   GDL_ARCHIVE          archive file name under gdl/state, default pixiv.arc
set -uo pipefail
cd "$(dirname "$(readlink -f "$0")")"
[ -f ingest.env ] && . ./ingest.env
: "${PIXIV_BOOKMARKS_URL:?set PIXIV_BOOKMARKS_URL (e.g. in ./ingest.env)}"
ABORT_AFTER="${GDL_ABORT_AFTER:-20}"
ARCHIVE="${GDL_ARCHIVE:-pixiv.arc}"

# one run at a time (e.g. a timer firing while a manual run is going)
exec 9>"${TMPDIR:-/tmp}/fulgorart-ingest.lock"
flock -n 9 || { echo "another ingest is already running"; exit 0; }

mkdir -p gdl/state gdl/staging gdl/cache
status=0
step() { echo "== $1 $(date -u +%FT%TZ)"; }

step download
docker compose run --rm -T gdl \
  --download-archive "/state/$ARCHIVE" -A "$ABORT_AFTER" --write-metadata \
  -D /staging "$PIXIV_BOOKMARKS_URL" \
  || { echo "gallery-dl exited $?; importing what was downloaded"; status=1; }

step import
docker compose run --rm -T cli gallery-dl --remove-imported /staging \
  || { echo "some files were not imported; they are still in gdl/staging"; status=1; }

step tag
docker compose run --rm -T cli remote-tagger || status=1

step "done (status $status)"
exit "$status"
