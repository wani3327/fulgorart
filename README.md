# fulgorart

A personal art collection manager that ingests liked posts from Pixiv and Twitter/X, stores images in Cloudflare R2, tags them with WD14 (ONNX), and exposes a dark-themed web UI for browsing and tagging.

## Architecture

Rust workspace with these crates:

| Crate | Description |
|-------|-------------|
| `crates/core` | Shared types, enums, `AppConfig` |
| `crates/db` | SQLite via `sqlx` — all DB queries |
| `crates/storage` | Cloudflare R2 / S3 upload client |
| `crates/tagger` | WD14 ONNX tagger worker (`fulgorart-tagger`) |
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

3. Build the workspace:

```bash
cargo build --release
```

## Building the tagger binary

The tagger is a separate release binary. Build it with:

```bash
cargo build --release -p fulgorart-tagger
# or
make build-tagger
```

The resulting executable is written to `target/release/fulgorart-tagger`.

## Shipping to another computer

The tagger is easiest to move as a small folder that contains the binary plus its runtime assets:

1. Copy `target/release/fulgorart-tagger` to the destination machine.
2. Copy the WD14 model files and point the binary at them with `WD14_MODEL_PATH` and `WD14_LABELS_PATH`, or place them at the default `./models/` paths.
3. Make sure the ONNX Runtime shared library is available on the target machine. With the current `load-dynamic` setup, placing `libonnxruntime.so` next to the executable is the simplest option on Linux. On Windows and macOS, use the platform-appropriate library name in the same folder as the binary, or set `ORT_DYLIB_PATH` explicitly.
4. Copy `.env` or set the required environment variables on the destination machine, especially `FULGORART_DB_PATH`, `FULGORART_R2_*`, and any tagger/model paths you need.

If you want a repeatable handoff, build the binary with `make build-tagger`, then archive the executable together with the model directory and the ONNX Runtime library that the executable will load at startup.

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
make build-tagger # cargo build --release -p fulgorart-tagger
make test    # cargo test
make fmt     # cargo fmt --all
make lint    # cargo clippy
make clean   # cargo clean
```
