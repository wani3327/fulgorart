# fulgorart

A personal art collection manager that ingests liked posts from Pixiv and Twitter/X, stores images in Cloudflare R2, tags them with WD14 (ONNX), and exposes a dark-themed web UI for browsing and tagging.

## Architecture

Rust workspace with these crates:

| Crate | Description |
|-------|-------------|
| `crates/core` | Shared types, enums, `AppConfig` |
| `crates/db` | SQLite via `sqlx` — all DB queries |
| `crates/storage` | Cloudflare R2 / S3 upload client |
| `crates/tagger` | WD14 ONNX tagger worker (stub; `ort` integration TODO) |
| `crates/ingestor` | Pixiv & Twitter source adapters |
| `crates/web` | Axum web application + REST API |
| `bin/cli` | `fulgorart-cli` command-line tool |

## Requirements

- Rust 1.75+
- SQLite (linked via `sqlx`)
- A Cloudflare R2 bucket (or any S3-compatible object store)

## Setup

1. Copy `.env.example` to `.env` and fill in your credentials:

```bash
cp .env.example .env
```

2. Edit `.env`:

```env
FULGORART_DB_PATH=./data/fulgorart.db
FULGORART_R2_BUCKET=my-bucket
FULGORART_R2_ENDPOINT=https://<account_id>.r2.cloudflarestorage.com
FULGORART_R2_ACCESS_KEY_ID=...
FULGORART_R2_SECRET_ACCESS_KEY=...
FULGORART_PASSWORD=mysecret        # optional HTTP Basic Auth
FULGORART_PORT=3000
```

3. Build:

```bash
cargo build --release
```

## Running the web server

```bash
cargo run --bin fulgorart-web
# or
make run-web
```

Then open `http://localhost:3000` in your browser.

## CLI usage

```bash
# Import a local image
cargo run --bin fulgorart-cli -- import-image \
  --file photo.jpg \
  --source-type pixiv \
  --source-post-id 12345 \
  --source-post-url https://www.pixiv.net/artworks/12345

# List images
cargo run --bin fulgorart-cli -- list-images

# Search tags
cargo run --bin fulgorart-cli -- search-tags "blue_hair"

# Manually add a tag to an image
cargo run --bin fulgorart-cli -- add-tag --image-id 1 --tag "blue_hair"
```

## REST API

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/images` | List images (supports `?include=tag1,tag2&exclude=tag3&page=1&per_page=20`) |
| `GET` | `/api/images/:id` | Get single image with tags |
| `POST` | `/api/images/:id/tags` | Add tag `{"tag": "name"}` |
| `DELETE` | `/api/images/:id/tags/:tag_id` | Remove tag |
| `GET` | `/api/tags` | List all tags (supports `?q=search`) |

## Make targets

```
make check   # cargo check
make build   # cargo build --release
make test    # cargo test
make fmt     # cargo fmt --all
make lint    # cargo clippy
make clean   # cargo clean
```
