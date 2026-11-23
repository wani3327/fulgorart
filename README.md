# fulgorart

A personal art collection manager that grabs liked images from SNS, stores them in Cloudflare R2, tags them with WD14, and exposes a web UI for browsing and manual tag edits.

## Architecture

| Crate | Responsibility |
|---|---|
| `crates/core` | Shared enums and small cross-crate types |
| `crates/db` | SQLite access layer and DB config |
| `crates/storage` | Cloudflare R2 / S3 client and R2 config |
| `crates/ingestor` | Independent image-grabber app; lib returns liked images as bytes, bin saves them to a directory |
| `crates/tagger` | Independent WD14 tagger app for local files and URLs |
| `crates/tagger_s3` | WD14 tagger runner for `r2://` keys (downloads from R2/S3 then tags) |
| `crates/web` | Axum web UI and REST API |
| `crates/cli` | Primary orchestration CLI with tool-style subcommands |

## Common environment variables

### Shared DB / storage

```env
FULGORART_DB_PATH=./data/fulgorart.db
FULGORART_R2_BUCKET=my-bucket
FULGORART_R2_ENDPOINT=https://<account_id>.r2.cloudflarestorage.com
FULGORART_R2_ACCESS_KEY_ID=...
FULGORART_R2_SECRET_ACCESS_KEY=...
```

### Ingestor / CLI source credentials

```env
PIXIV_ACCESS_TOKEN=...
PIXIV_USER_ID=...                # optional; auto-resolved when omitted

# X (Twitter): OAuth 2.0 app from the X developer portal (user authentication on,
# type "Native App" or "Web App", callback URL = FULGORART_X_REDIRECT_URI)
FULGORART_X_CLIENT_ID=...
FULGORART_X_CLIENT_SECRET=...    # only for confidential (Web App) clients
FULGORART_X_REDIRECT_URI=http://127.0.0.1:8765/callback   # default
FULGORART_X_TOKEN_PATH=...       # default: x-token.json next to FULGORART_DB_PATH
```

### Ingest liked posts (`cli ingestor <service>`)

```bash
cli ingestor x login             # once: open the URL, approve, paste the redirected URL back
cli ingestor x --dry-run         # list what would be imported
cli ingestor x                   # import new likes (stops at already-imported ones)
cli ingestor x --max-pages 0 --full   # first full backfill / fill gaps (X bills per post read)
```

In Docker: `docker compose run --rm cli ingestor x login`. Images go to R2 with
thumbnails and get queued for tagging, like `gallery-dl` imports.

### CLI tagger-tool

```env
TAGGER_BATCH_SIZE=20
```

### CLI remote-tagger (future)

```env
GCP_PROJECT_ID=my-project
GCP_REGION=asia-northeast1
CLOUD_RUN_JOB_NAME=fulgorart-tagger
```

### Tagger model paths

```env
WD14_MODEL_PATH=./models/wd14-convnext.onnx
WD14_LABELS_PATH=./models/selected_tags.csv
WD14_GENERAL_THRESHOLD=0.35
WD14_CHARACTER_THRESHOLD=0.75
```

### Web UI

```env
FULGORART_PASSWORD=mysecret      # optional HTTP Basic Auth
FULGORART_PORT=3000
```

## Build and validate

```bash
cargo build
cargo test
cargo clippy --all-targets --all-features
```

## Run the independent ingestor app

Save grabbed liked images into a directory:

```bash
cargo run --bin fulgorart-ingestor -- ./data/ingestor
# or use FULGORART_INGESTOR_OUTPUT_DIR and omit the argument
```

The binary saves files under `<output-dir>/<source_type>/<source_post_id>/`.

## Run the tagger app

```bash
# local files
cargo run --bin fulgorart-tagger -- ./image.jpg ./other.png

# URLs
cargo run --bin fulgorart-tagger -- https://example.com/a.jpg

# R2 keys
cargo run --bin fulgorart-tagger-s3 -- r2://images/pixiv/2026/05/20/hash.jpg
```

## Run the web server

```bash
cargo run --bin fulgorart-web
# or
make run-web
```

Then open `http://localhost:3000`.

HTML templates and the stylesheet are read from `crates/web/templates` and
`crates/web/assets/style.css` for each request, so changes are applied on the
next request without restarting the web server.

## CLI usage

```bash
# Register a local image into DB + R2 and queue a tag job
cargo run --bin fulgorart-cli -- upload-tool \
  --file photo.jpg \
  --source-type pixiv \
  --source-post-id 12345 \
  --source-post-url https://www.pixiv.net/artworks/12345

# Process pending tag jobs with local WD14
cargo run --bin fulgorart-cli -- tagger-tool --batch-size 20

# Import files downloaded by gallery-dl (metadata from --write-metadata sidecars)
cargo run --bin fulgorart-cli -- gallery-dl --remove-imported ./staging

# Tag pending jobs on the Cloud Run tagger job
cargo run --bin fulgorart-cli -- remote-tagger --max-batches 1
```

## Pixiv bookmarks pipeline (gallery-dl)

```
gallery-dl (fulgorart-gdl) ──▶ gdl/staging: x.png + x.png.json ──▶ cli gallery-dl ──▶ R2 + DB ──▶ cli remote-tagger
```

`deploy/ingest-pixiv.sh` runs the three steps with the services in
`deploy/compose.yaml`:

1. **download**: the `gdl` service (image `fulgorart-gdl`, `deploy/gdl/`) runs
   gallery-dl with `--download-archive gdl/state/pixiv.arc -A 20
   --write-metadata`. Bookmarks come newest first and `-A 20` stops after 20
   already-downloaded files in a row, so a routine run only looks at the newest
   few bookmarks. Each file gets a metadata sidecar, `x.png.json`.
2. **import**: `cli gallery-dl --remove-imported /staging` stores each file via
   the shared import path (also used by `cli ingestor x`): rejects files that
   don't decode (e.g. truncated downloads), dedups by sha256, uploads original +
   thumbnail to R2, queues a tag job. Stored and duplicate files are removed
   from staging; anything left there needs attention. If an upload fails, the
   image's DB row is released, so the next run retries it.
3. **tag**: `cli remote-tagger`.

The metadata reader only needs `category` and `id`; everything else is optional
and unknown fields are ignored, so gallery-dl updates don't break imports.
Pixiv is mapped exactly; other gallery-dl sites get a best-effort mapping.
`cli gallery-dl <dir> <dump.json>` still accepts a `gallery-dl -J` dump.

### Setup on a host

```bash
# next to compose.yaml (copy deploy/compose.yaml and deploy/ingest-pixiv.sh there)
mkdir -p gdl/state gdl/staging gdl/cache
# reuse an existing archive so already-downloaded bookmarks aren't fetched again
cp ~/gdl-archives/pixiv.arc gdl/state/pixiv.arc
echo 'PIXIV_BOOKMARKS_URL=https://www.pixiv.net/users/<id>/bookmarks/artworks' > ingest.env
docker compose pull
./ingest-pixiv.sh
```

The gallery-dl config (site tokens/cookies) is mounted read-only from
`~/.config/gallery-dl` into the `gdl` container only and is never baked into
the image. gallery-dl's version is pinned in `deploy/gdl/Dockerfile`; bump it
there to update (the image rebuilds on push to `main`).

## REST API

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/images` | List images (`?include=tag1,tag2&exclude=tag3&page=1&per_page=20`) |
| `GET` | `/api/images/:id` | Get one image with tags |
| `POST` | `/api/images/:id/tags` | Add tag `{"tag":"name"}` |
| `DELETE` | `/api/images/:id/tags/:tag_id` | Remove tag |
| `GET` | `/api/tags` | List all tags (`?q=search`) |
