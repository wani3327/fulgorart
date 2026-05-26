# Personal Like-Crawler + Image Library (R2) — Specification

Date: 2026-04-04  
Owner: `wani3327`  
Status: Draft v0.1 (MVP-first)

## 1. Goal

Build a **private** application that:
1. Polls social services (initially: **Pixiv** and/or **Twitter/X**).
2. Collects posts the user **liked**.
3. Downloads original images and stores them in **Cloudflare R2**.
4. Runs **WD14** to generate danbooru-style tags.
5. Lets the user review and manually edit tags later.
6. Provides a web UI for browsing and search.

MVP focus: **reliable image grabbing + bridge orchestration + storage + tagging + browse UI**.

## 2. High-level Architecture

### 2.1 Components

- **Ingestor** (`crates/ingestor`, Rust)
  - Independent app.
  - Library responsibility: grab liked images from SNS and return them as bytes plus metadata.
  - Binary responsibility: save grabbed images into a caller-provided directory and exit.
  - Must not own R2 upload, DB persistence, or tag-job dispatch.

- **Tagger** (`crates/tagger`, Rust)
  - Independent app.
  - Accepts local paths, URLs, or `r2://` object keys.
  - Emits JSON lines with tag predictions for each input.
  - Must not own DB reads/writes or queue management.

- **Bridge** (`crates/bridge`, Rust)
  - Orchestrator app.
  - Calls the ingestor library as its image-grabber.
  - Uploads grabbed images with `fulgorart-storage`.
  - Creates and updates DB rows with `fulgorart-db`.
  - Queues pending tag jobs.
  - Executes tagging behind a `TaggerJob` trait.
  - Supports at least two `TaggerJob` implementations:
    - local tagging by calling `fulgorart-tagger` library code
    - Cloud Run tagging by calling GCP Cloud Run Job
  - Stores returned tags and updates tag job status.
  - Internally separates image-grabber, storage, and tagger responsibilities behind traits.

- **Web app**
  - Serves pages for browsing images.
  - Filters by tags and sources.
  - Provides review/edit UI for tags and metadata.

### 2.2 Deployment assumptions

- Primary server: small VM.
- Object storage: Cloudflare R2 (S3-compatible).
- DB: SQLite for MVP.
- Tagger runtime can be local (`fulgorart-tagger` library call) or cloud (GCP Cloud Run job).

## 3. Data Model (MVP)

### 3.1 Entities

#### `post`
A liked post/item from a source service.

Fields:
- `id` (pk)
- `source_type`
- `source_post_id`
- `source_post_url`
- `liked_at` (nullable)
- `author_name` (nullable)
- `author_id` (nullable)
- `raw_json` (nullable)
- `created_at`, `updated_at`

Constraint:
- Unique (`source_type`, `source_post_id`)

#### `image_asset`
Represents an uploaded image file.

Fields:
- `id` (pk)
- `post_id` (fk)
- `group_id` (fk, nullable)
- `sha256`
- `r2_key`
- `r2_url`
- `width`, `height` (nullable)
- `file_size` (nullable)
- `content_type`
- `source_url` (nullable)
- `created_at`, `updated_at`

Constraint:
- Unique (`sha256`)

#### `tag`
Normalized tag dictionary.

Fields:
- `id` (pk)
- `name`
- `category` (nullable)
- `created_at`

Constraint:
- Unique (`name`)

#### `image_tag`
Many-to-many association between `image_asset` and `tag`.

Fields:
- `image_id`
- `tag_id`
- `source` (`wd14` or `manual`)
- `score` (nullable)
- `created_at`

Constraint:
- Unique (`image_id`, `tag_id`)

### 3.2 Queue tables

#### `tag_job`
Fields:
- `id` (pk)
- `image_id` (fk -> `image_asset`)
- `status` (`pending`, `running`, `done`, `failed`)
- `error` (nullable)
- `created_at`, `updated_at`

Behavior:
- Bridge owns queue creation and status updates.
- Only one effective tag job should exist per image for the normal bridge flow.

## 4. Tagging (WD14)

### 4.1 Tag generation
- For each image, run WD14 and produce `(tag_name, score)` values.
- Apply thresholds:
  - `general_threshold` (default `0.35`)
  - `character_threshold` (default `0.75`)

### 4.2 Storage rules
- Store WD14 tags with `source = wd14` and `score = predicted_score`.
- Store manual tags with `source = manual` and `score = NULL`.

### 4.3 Tagger interface
- Tagger input may be:
  - local file path
  - public URL
  - `r2://` object key
- Tagger output is one JSON object per processed input.
- Bridge consumes the JSON output and writes DB state.

## 5. Web UI (MVP)

### 5.1 Pages
1. **Gallery**
   - paginated image list
   - include / exclude tag filters
2. **Image detail**
   - full image view
   - metadata display
   - add/remove manual tags

### 5.2 Search semantics
- AND semantics for included tags.
- NOT semantics for excluded tags.

## 6. Image grabbing and bridge flow

### 6.1 Source support strategy
Because Twitter/X and Pixiv APIs can be restricted, image grabbing should be adapter-based.

- `SourceAdapter` trait:
  - fetch liked posts
  - download original images

### 6.2 End-to-end flow
1. Bridge calls ingestor adapters.
2. Ingestor returns grabbed posts and image bytes.
3. Bridge uploads images to R2.
4. Bridge persists `post`, `image_asset`, and `tag_job` rows.
5. Bridge invokes the configured `TaggerJob` strategy.
6. `TaggerJob` runs tagging (local or Cloud Run) and returns results.
7. Bridge stores tags and updates queue state.

### 6.3 Idempotency
- Bridge must be safe to run repeatedly.
- Existing `post` rows should be updated rather than duplicated.
- Existing `image_asset` rows keyed by `sha256` should be reused.
- Bridge should avoid spawning redundant tag jobs for the same image in the normal path.

### 6.4 Storage key format
Recommended canonical key:
- `images/{source_type}/{yyyy}/{mm}/{dd}/{sha256}.{ext}`

## 7. Security & Privacy

- Require authentication for web UI in private deployments.
- Do not expose R2 credentials to clients.
- Keep source credentials, local tagger config, and Cloud Run credentials outside `fulgorart-core` and scoped to the crates that use them.

## 8. Observability

- Structured logs for:
  - image grabbing
  - upload events
  - tagger job outcomes (local or cloud)
  - tagging result application

## 9. Rust implementation notes

- Runtime: `tokio`
- Web framework: `axum`
- DB: SQLite + `sqlx`
- R2: AWS SDK for Rust
- WD14:
  - ONNX Runtime via `ort`
  - input preprocessing: square pad → resize 448×448 → BGR float32 NHWC tensor

## 10. MVP milestones

### Milestone 1 — Storage + DB skeleton ✅
- Setup DB schema
- Implement R2 upload with metadata
- Add a CLI import path for local testing

### Milestone 2 — WD14 tagging pipeline ✅
- Make `fulgorart-tagger` process explicit inputs independently
- Store tags + scores through bridge-owned orchestration

### Milestone 3 — Web gallery + detail pages
- Browse images
- Filter by tags
- Manual tag editing

### Milestone 4 — Source image-grabber + bridge orchestration
- Implement at least one source adapter end-to-end
- Run `fulgorart-ingestor` independently for local dumps
- Run `fulgorart-bridge` for production orchestration
- Keep repeated bridge runs idempotent enough for scheduled execution

## 11. Acceptance criteria

- `fulgorart-ingestor` can grab at least one source and save images to a directory.
- `fulgorart-bridge` can ingest grabbed results into R2 and SQLite.
- `fulgorart-tagger` can tag provided local paths, URLs, and `r2://` keys.
- Bridge can apply returned WD14 tags and update tag-job status.
- Web UI can browse images and persist manual tag edits.
