# Personal Like-Crawler + Image Library (R2) — Specification

Date: 2026-04-04  
Owner: `wani3327`  
Status: Draft v0.1 (MVP-first)

## 1. Goal

Build a **private** application that:
1. Polls social services (initially: **Pixiv** and/or **Twitter/X**; exact APIs depend on availability).
2. Collects posts the user **liked**.
3. Downloads original images and stores them in **Cloudflare R2**.
4. Runs **WD14 tagger** locally to generate tags (danbooru-style).
5. Lets the user **review and manually edit tags** later.
6. Provides a **web UI** (and later Android client) to browse and search the collection.

MVP focus: **reliable ingestion + storage + tagging + browse UI**.

Non-goals (for now):
- Perfect character recognition for new series/characters.
- Full automatic dedup across all sources (we can add later).
- Multi-user / public hosting.

## 2. High-level Architecture

### 2.1 Components

- **Ingestor** (`crates/ingestor`, Rust):
  - One-shot binary: run once and exit.
  - Polls source services for “liked” posts.
  - Extracts media URLs and metadata.
  - Downloads images (or fetches original file if possible).
  - Uploads images to Cloudflare R2.
  - Creates DB records for posts and images.
  - Enqueues images for tagging.
  - Scheduled via **cron** (e.g. `*/5 * * * *`).

- **Tagger** (`crates/tagger`, Rust):
  - One-shot binary: run once and exit.
  - Pulls all pending images from the tag job queue.
  - Runs **WD14** (prefer ONNX runtime if feasible, otherwise shell-out to Python as a separate process).
  - Stores predicted tags and scores.
  - Marks image as “tagged (auto)” but still “needs review”.
  - Scheduled via **cron** (e.g. `* * * * *`).

- **Web app**:
  - Serves pages for browsing images.
  - Filters by tags and sources.
  - Provides review/edit UI for tags and metadata.

### 2.2 Deployment assumptions

- Primary server: small AWS Lightsail instance (1GB RAM).
- Object storage: Cloudflare R2 (S3-compatible).
- DB: SQLite (MVP) or Postgres (later). Prefer **SQLite** for simplicity on Lightsail.

## 3. Data Model (MVP)

### 3.1 Entities

#### `source_account`
Represents a configured account + credentials per source.

Fields:
- `id` (pk)
- `source_type` (enum: `pixiv`, `twitter`)
- `account_handle` (string)
- `created_at`, `updated_at`
- `config_json` (text) — tokens, cookies, settings (encrypted at rest if possible)

#### `post`
A liked post/item from a source service.

Fields:
- `id` (pk)
- `source_type` (enum)
- `source_post_id` (string) — id from Pixiv/Twitter
- `source_post_url` (string)
- `liked_at` (datetime, nullable if unknown)
- `author_name` (string, nullable)
- `author_id` (string, nullable)
- `raw_json` (text, nullable) — store raw API payload when available
- `created_at`, `updated_at`

Constraints:
- Unique (`source_type`, `source_post_id`)

#### `image_asset`
Represents a downloaded image file.

Fields:
- `id` (pk)
- `post_id` (fk -> post)
- `source_media_url` (string) — where it came from
- `original_filename` (string, nullable)
- `content_type` (string, e.g. `image/jpeg`)
- `byte_size` (int)
- `width`, `height` (int, nullable)
- `r2_bucket` (string)
- `r2_object_key` (string) — canonical storage key
- `sha256` (string) — for dedup and integrity
- `phash` (string, nullable) — optional for near-duplicate later
- `downloaded_at` (datetime)
- `created_at`, `updated_at`

Constraints:
- Unique (`sha256`) optional in MVP (enable later if desired)

#### `tag`
Normalized tag dictionary (optional in MVP; can also store tags as plain strings).

Fields:
- `id` (pk)
- `name` (string) — e.g. `1girl`, `long_hair`, `cosplay`
- `category` (enum/string, optional): `general`, `character`, `artist`, `copyright`, etc.
- `created_at`, `updated_at`

Constraints:
- Unique (`name`)

#### `image_tag`
Many-to-many association between `image_asset` and `tag`.

Fields:
- `image_id` (fk)
- `tag_id` (fk)
- `source` (enum: `wd14`, `manual`)
- `score` (float, nullable) — WD14 confidence
- `created_at`

Constraints:
- Unique (`image_id`, `tag_id`, `source`) (or just (`image_id`, `tag_id`) if you want one row per tag and record origin differently)

### 3.2 Queue tables (MVP)

#### `tag_job`
Fields:
- `id` (pk)
- `image_id` (fk -> image_asset)
- `status` (enum: `pending`, `running`, `done`, `failed`)
- `attempts` (int)
- `last_error` (text, nullable)
- `created_at`, `updated_at`

Constraints:
- Unique (`image_id`) (one job per image)

## 4. Tagging (WD14)

### 4.1 Tag generation
- For each image, run WD14 tagger and produce:
  - list of `(tag_name, score)`
- Apply thresholds:
  - `general_threshold` (e.g. 0.35)
  - `character_threshold` (e.g. 0.75)
  - thresholds configurable per category (if category is available) or just a single threshold initially.

### 4.2 Storage rules
- Store WD14 tags with `source = wd14` and `score = predicted_score`.
- Manual tags stored with `source = manual` and `score = NULL`.
- Manual tags override behavior:
  - If a user removes an auto-tag, record that removal (optional future feature).
  - MVP: just delete the tag association.

### 4.3 Tag normalization
- Normalize tag strings to:
  - lowercase
  - spaces -> underscores
  - trim
- Keep danbooru-style tags as-is (e.g. `1girl`, `blue_eyes`, `looking_at_viewer`).

## 5. Web UI (MVP)

### 5.1 Pages

1. **Gallery**
   - Infinite scroll or paginated.
   - Shows thumbnails.
   - Filters:
     - include tags
     - exclude tags
     - source_type
     - date range (liked_at / downloaded_at)
     - “needs review” toggle (images with only auto-tags or with failed tagging)

2. **Image detail**
   - Full image view
   - Display metadata: source, author, post link
   - Tag list grouped by:
     - manual tags
     - wd14 tags (with scores)
   - Actions:
     - add tag
     - remove tag
     - quick-apply common tags

3. **Tag management** (optional MVP)
   - Search tags
   - Rename tag (careful: impacts associations)

### 5.2 Search semantics
- Basic tag filtering:
  - AND semantics for included tags (MVP).
  - Excluded tags (NOT).
- Later: OR / parentheses.

## 6. Ingestion (MVP scope)

### 6.1 Source support strategy
Because Twitter/X and Pixiv APIs can be restricted, ingestion should be designed with adapters:

- `SourceAdapter` trait:
  - `list_likes(since_cursor) -> Vec<PostRef>`
  - `fetch_post(post_ref) -> PostWithMedia`
  - `extract_media_urls(post) -> Vec<MediaUrl>`

MVP can start with only 1 adapter (whichever is easiest to implement reliably).

### 6.2 Idempotency
- Ingestor must be safe to run repeatedly.
- If (`source_type`, `source_post_id`) already exists, skip or update metadata.
- If image `sha256` already exists, avoid re-upload (optional MVP).

### 6.3 Storage key format (R2)
Recommended canonical key:
- `images/{source_type}/{yyyy}/{mm}/{dd}/{sha256}.{ext}`

Optionally keep original file name in metadata.

## 7. Security & Privacy

- App is private; require authentication for web UI:
  - MVP: basic auth (nginx) or simple password login.
  - Later: OAuth / session cookies.
- Do not expose R2 credentials to client.
- Store source tokens securely:
  - MVP: env vars or config file with restricted permissions.
  - Later: encryption-at-rest.

## 8. Observability (MVP)

- Structured logs:
  - ingestion events
  - download/upload events
  - tagging outcomes
- Basic metrics (optional):
  - number of images downloaded/day
  - tagger success/failure count
- Admin page (optional):
  - show recent failures

## 9. Rust Implementation Notes (MVP)

- Runtime: `tokio`
- Web framework: `axum` (recommended) or `actix-web`
- DB:
  - SQLite + `sqlx` recommended
- R2:
  - S3-compatible client (AWS SDK for Rust or `s3` crate)
- WD14:
  - ONNX runtime via the `ort` crate (`download-binaries` feature builds ONNX Runtime automatically).
  - Image preprocessing: pad to square with white background → resize 448×448 → BGR float32 NHWC tensor.
  - Labels loaded from `selected_tags.csv` (the WD14 model release artifact from SmilingWolf on HuggingFace).
  - Env vars: `WD14_MODEL_PATH`, `WD14_LABELS_PATH`, `WD14_GENERAL_THRESHOLD`, `WD14_CHARACTER_THRESHOLD`.

## 10. MVP Milestones

### Milestone 1 — Storage + DB skeleton ✅
- Setup DB schema
- Implement R2 upload with metadata
- CLI to import local images (for testing) into R2 and DB

### Milestone 2 — WD14 tagging pipeline ✅
- Tag job queue
- `fulgorart-tagger` drains all pending jobs via ONNX WD14 inference (`ort` crate)
- Store tags + scores; mark failures with error message

### Milestone 3 — Web gallery + detail pages
- Thumbnail generation strategy (store smaller images or generate on demand)
- Filter by tags
- Manual tag editing

### Milestone 4 — Source ingestion adapter
- Implement one source adapter end-to-end
- Schedule `fulgorart-ingestor` and `fulgorart-tagger` via cron (each is a one-shot binary)
- Idempotency

## 11. Open Questions (to decide before coding)

1. Which source to implement first: Pixiv or Twitter/X?
2. WD14 execution strategy: ONNX in Rust vs Python subprocess?
3. Thumbnail generation:
   - generate at ingest time and store in R2, or
   - generate on-demand and cache?
4. DB choice: SQLite (simple) vs Postgres (future scaling)?
5. How strict should dedup be: `sha256` only or also perceptual hash?

## 12. Acceptance Criteria (MVP)

- Can ingest at least one source reliably and store images in R2.
- WD14 tags are generated and stored with scores.
- Web UI can browse images and filter by tags.
- User can add/remove tags manually and changes persist.
- System is stable under tens of images per day.
