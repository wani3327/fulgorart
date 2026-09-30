ALTER TABLE image_asset RENAME COLUMN s3_key TO s3_key_base;
DROP INDEX IF EXISTS idx_image_asset_s3_key;
CREATE INDEX IF NOT EXISTS idx_image_asset_s3_key_base ON image_asset(s3_key_base);
