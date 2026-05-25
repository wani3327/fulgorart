# fulgorart

A personal art collection manager that grabs liked images from SNS, stores them in Cloudflare R2, tags them with WD14, and exposes a web UI for browsing and manual tag edits.

## Architecture

| Crate | Responsibility |
|---|---|
| `crates/core` | Shared enums and small cross-crate types |
| `crates/db` | SQLite access layer and DB config |
| `crates/storage` | Cloudflare R2 / S3 client and R2 config |
| `crates/ingestor` | Independent image-grabber app; lib returns liked images as bytes, bin saves them to a directory |
| `crates/tagger` | Independent WD14 tagger app for local files, URLs, or `r2://` keys |
| `crates/bridge` | Orchestrator app; combines grabber + storage + tagger jobs and persists/applies tags |
| `crates/web` | Axum web UI and REST API |
| `crates/cli` | Local maintenance CLI |

## Common environment variables

### Shared DB / storage

```env
FULGORART_DB_PATH=./data/fulgorart.db
FULGORART_R2_BUCKET=my-bucket
FULGORART_R2_ENDPOINT=https://<account_id>.r2.cloudflarestorage.com
FULGORART_R2_ACCESS_KEY_ID=...
FULGORART_R2_SECRET_ACCESS_KEY=...
```

### Ingestor / bridge source credentials

```env
PIXIV_ACCESS_TOKEN=...
PIXIV_USER_ID=...                # optional; auto-resolved when omitted
TWITTER_BEARER_TOKEN=...
```

### Bridge tagging

```env
TAGGER_BATCH_SIZE=20
```

### Bridge Cloud Run mode

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

## Run the bridge app

Grab liked images, upload to R2, persist DB rows, queue tag jobs, and execute tagger jobs:

```bash
cargo run --bin fulgorart-bridge -- --tagger-mode local
# or
cargo run --bin fulgorart-bridge -- --tagger-mode cloud_run
# or
make run-bridge
```

## Run the tagger app

```bash
# local files
cargo run --bin fulgorart-tagger -- ./image.jpg ./other.png

# URLs
cargo run --bin fulgorart-tagger -- https://example.com/a.jpg

# R2 keys
cargo run --bin fulgorart-tagger -- r2://images/pixiv/2026/05/20/hash.jpg
```

## Run the web server

```bash
cargo run --bin fulgorart-web
# or
make run-web
```

Then open `http://localhost:3000`.

## CLI usage

```bash
# Import a local image directly into DB + R2
cargo run --bin fulgorart-cli -- import-image \
  --file photo.jpg \
  --source-type pixiv \
  --source-post-id 12345 \
  --source-post-url https://www.pixiv.net/artworks/12345

# List images
cargo run --bin fulgorart-cli -- list-images

# Search tags
cargo run --bin fulgorart-cli -- search-tags blue_hair

# Manually add a tag
cargo run --bin fulgorart-cli -- add-tag --image-id 1 --tag blue_hair
```

## REST API

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/images` | List images (`?include=tag1,tag2&exclude=tag3&page=1&per_page=20`) |
| `GET` | `/api/images/:id` | Get one image with tags |
| `POST` | `/api/images/:id/tags` | Add tag `{"tag":"name"}` |
| `DELETE` | `/api/images/:id/tags/:tag_id` | Remove tag |
| `GET` | `/api/tags` | List all tags (`?q=search`) |
