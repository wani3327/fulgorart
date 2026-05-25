use anyhow::Result;
use fulgorart_db::Db;
use fulgorart_ingestor::GrabbedPost;
use fulgorart_storage::R2Client;
use sha2::Digest;

pub struct R2StorageJob {
    db: Db,
    r2: R2Client,
}

impl R2StorageJob {
    pub fn new(db: Db, r2: R2Client) -> Self {
        Self { db, r2 }
    }
}

impl R2StorageJob {
    pub async fn store_posts(&self, posts: Vec<GrabbedPost>) -> Result<usize> {
        let mut stored_images = 0usize;

        for post in posts {
            let post_row = self
                .db
                .insert_post(
                    &post.source_type,
                    &post.source_post_id,
                    &post.source_post_url,
                    post.liked_at.as_deref(),
                    post.author_name.as_deref(),
                    post.author_id.as_deref(),
                    post.raw_json.as_deref(),
                )
                .await?;
            let group_row = self.db.insert_image_group(Some(post_row.id)).await?;

            for image in post.images {
                let sha256 = {
                    let mut hasher = sha2::Sha256::new();
                    hasher.update(&image.bytes);
                    hex::encode(hasher.finalize())
                };
                let ext = match image.content_type.as_str() {
                    "image/png" => "png",
                    "image/gif" => "gif",
                    "image/webp" => "webp",
                    _ => "jpg",
                };
                let key = R2Client::canonical_key(&post.source_type, &sha256, ext);
                let r2_url = self.r2.object_url(&key);

                self.r2
                    .upload(&key, image.bytes.clone(), &image.content_type)
                    .await?;

                let asset = self
                    .db
                    .insert_image_asset(
                        Some(post_row.id),
                        Some(group_row.id),
                        &sha256,
                        &key,
                        &r2_url,
                        None,
                        None,
                        Some(image.bytes.len() as i64),
                        &image.content_type,
                        Some(&image.source_url),
                    )
                    .await?;
                self.db.ensure_tag_job(asset.id).await?;
                stored_images += 1;
            }
        }

        Ok(stored_images)
    }
}
