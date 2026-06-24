use anyhow::Result;
use fulgorart_ingestor::GrabbedPost;
use fulgorart_storage::R2Client;
use sha2::Digest;

#[derive(Debug, Clone)]
pub struct StoredImage {
    pub sha256: String,
    pub s3_key: String,
    pub file_size: i64,
    pub content_type: String,
    pub source_url: String,
}

pub struct R2StorageJob {
    r2: R2Client,
}

impl R2StorageJob {
    pub fn new(r2: R2Client) -> Self {
        Self { r2 }
    }

    pub async fn store_post(&self, post: GrabbedPost) -> Result<Vec<StoredImage>> {
        let mut stored = Vec::new();

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
            let content_type = image.content_type.clone();
            let source_url = image.source_url;
            let file_size = image.bytes.len() as i64;

            self.r2.upload(&key, image.bytes, &content_type).await?;

            stored.push(StoredImage {
                sha256,
                s3_key: key,
                file_size,
                content_type,
                source_url,
            });
        }

        Ok(stored)
    }
}
