CREATE TABLE IF NOT EXISTS image_group (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    post_id INTEGER REFERENCES post(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(post_id)
);

ALTER TABLE image_asset ADD COLUMN group_id INTEGER REFERENCES image_group(id);

CREATE INDEX IF NOT EXISTS idx_image_group_post_id ON image_group(post_id);
CREATE INDEX IF NOT EXISTS idx_image_asset_group_id ON image_asset(group_id);
