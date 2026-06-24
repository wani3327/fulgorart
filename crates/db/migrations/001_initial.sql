CREATE TABLE IF NOT EXISTS author (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_type TEXT NOT NULL,
    source_author_id TEXT NOT NULL,
    name TEXT,
    url TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(source_type, source_author_id)
);

CREATE TABLE IF NOT EXISTS post (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_type TEXT NOT NULL,
    source_post_id TEXT NOT NULL,
    source_post_url TEXT NOT NULL,
    liked_at TEXT,
    author_id INTEGER REFERENCES author(id),
    raw_json TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(source_type, source_post_id)
);

CREATE TABLE IF NOT EXISTS image_asset (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    post_id INTEGER REFERENCES post(id),
    sha256 TEXT NOT NULL UNIQUE,
    s3_key TEXT NOT NULL,
    width INTEGER,
    height INTEGER,
    file_size INTEGER,
    content_type TEXT NOT NULL DEFAULT 'image/jpeg',
    source_url TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS tag (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    category TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS image_tag (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    image_id INTEGER NOT NULL REFERENCES image_asset(id) ON DELETE CASCADE,
    tag_id INTEGER NOT NULL REFERENCES tag(id) ON DELETE CASCADE,
    source TEXT NOT NULL DEFAULT 'manual',
    score REAL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(image_id, tag_id)
);

CREATE TABLE IF NOT EXISTS tag_job (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    image_id INTEGER NOT NULL REFERENCES image_asset(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'pending',
    error TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_post_source ON post(source_type, source_post_id);
CREATE INDEX IF NOT EXISTS idx_image_asset_sha256 ON image_asset(sha256);
CREATE INDEX IF NOT EXISTS idx_image_tag_image_id ON image_tag(image_id);
CREATE INDEX IF NOT EXISTS idx_image_tag_tag_id ON image_tag(tag_id);
CREATE INDEX IF NOT EXISTS idx_tag_job_status ON tag_job(status);
CREATE INDEX IF NOT EXISTS idx_author_source ON author(source_type, source_author_id);
CREATE INDEX IF NOT EXISTS idx_post_author_id ON post(author_id);
CREATE INDEX IF NOT EXISTS idx_image_asset_s3_key ON image_asset(s3_key);
