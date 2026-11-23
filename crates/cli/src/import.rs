//! Storing one image: the shared path for every importer (`gallery-dl`,
//! `ingestor x`, ...): decode check, dedup by sha256, post/author rows, R2
//! upload with thumbnail, tag job.

use anyhow::{Context, Result};
use async_trait::async_trait;
use fulgorart_db::{Db, GalleryImportAuthorArgs, GalleryImportImageArgs, GalleryImportPostArgs};
use fulgorart_storage::R2Client;
use image::GenericImageView;
use sha2::Digest;

/// Where image bytes go. R2 in production; tests use a fake.
#[async_trait]
pub(crate) trait ImageStore: Send + Sync {
    async fn upload_with_thumbnail(&self, key_base: &str, data: Vec<u8>, content_type: &str) -> Result<()>;
}

#[async_trait]
impl ImageStore for R2Client {
    async fn upload_with_thumbnail(&self, key_base: &str, data: Vec<u8>, content_type: &str) -> Result<()> {
        R2Client::upload_with_thumbnail(self, key_base, data, content_type).await
    }
}

/// The post an image belongs to.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ImportPost {
    /// `post.source_type`, also the R2 key prefix (e.g. "pixiv", "x").
    pub source_type: String,
    pub source_post_id: String,
    pub source_post_url: String,
    pub posted_at: Option<String>,
    pub title: Option<String>,
    pub caption: Option<String>,
    pub raw_json: Option<String>,
    pub author: Option<ImportAuthor>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ImportAuthor {
    pub source_author_id: String,
    pub name: Option<String>,
    pub url: Option<String>,
    pub profile_url: Option<String>,
}

/// One image file to store.
pub(crate) struct ImportImage {
    pub data: Vec<u8>,
    /// Name without extension, stored on the row and used in the R2 key.
    pub filename: String,
    /// Where it was downloaded from, if known.
    pub source_url: Option<String>,
}

#[derive(Debug, PartialEq)]
pub(crate) enum ImportOutcome {
    Uploaded { image_id: i64, key: String },
    /// Same bytes are already stored (sha256), possibly under another post.
    Duplicate { sha256: String },
    /// Not stored: the file isn't a decodable image (e.g. truncated download).
    /// Nothing was written; fix the file and import again.
    Rejected(String),
}

/// Store one image. An `Err` means the upload (or the DB) failed; the claim is
/// released first, so importing the same file again retries it.
pub(crate) async fn import_image(
    db: &Db,
    store: &dyn ImageStore,
    post: &ImportPost,
    image: ImportImage,
) -> Result<ImportOutcome> {
    // Decode before touching the DB: a truncated or non-image file would
    // otherwise be uploaded and only fail later at thumbnail or tagging time.
    let decoded = match image::load_from_memory(&image.data) {
        Ok(decoded) => decoded,
        Err(error) => return Ok(ImportOutcome::Rejected(format!("not a decodable image: {error}"))),
    };
    let (width, height) = decoded.dimensions();
    drop(decoded);
    let content_type = image::guess_format(&image.data)
        .map(|format| format.to_mime_type())
        .unwrap_or("application/octet-stream")
        .to_owned();
    let sha256 = hex::encode(sha2::Sha256::digest(&image.data));
    let key = R2Client::canonical_key_base(&post.source_type, &image.filename);

    let author = post.author.as_ref().map(|a| GalleryImportAuthorArgs {
        source_author_id: &a.source_author_id,
        name: a.name.as_deref(),
        url: a.url.as_deref(),
        profile_url: a.profile_url.as_deref(),
    });
    let claimed = db
        .claim_gallery_image_upload(
            &GalleryImportPostArgs {
                source_type: &post.source_type,
                source_post_id: &post.source_post_id,
                source_post_url: &post.source_post_url,
                uploaded_at: post.posted_at.as_deref(),
                title: post.title.as_deref(),
                caption: post.caption.as_deref(),
                raw_json: post.raw_json.as_deref(),
            },
            author.as_ref(),
            &GalleryImportImageArgs {
                sha256: &sha256,
                s3_key_base: &key,
                filename: Some(&image.filename),
                width: Some(width as i64),
                height: Some(height as i64),
                file_size: Some(image.data.len() as i64),
                content_type: &content_type,
                source_url: image.source_url.as_deref(),
            },
        )
        .await?;

    let Some(claimed) = claimed.claimed_upload else {
        return Ok(ImportOutcome::Duplicate { sha256 });
    };

    match store.upload_with_thumbnail(&key, image.data, &content_type).await {
        Ok(()) => {
            db.update_tag_job_status(claimed.job.id, "uploaded", None).await?;
            Ok(ImportOutcome::Uploaded {
                image_id: claimed.asset.id,
                key,
            })
        }
        Err(error) => {
            db.release_image_asset_claim(claimed.asset.id)
                .await
                .context("upload failed and releasing the claim failed too")?;
            Err(error.context(format!("upload to {key}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records uploads; fails them while `fail` is set.
    #[derive(Default)]
    struct FakeStore {
        fail: Mutex<bool>,
        uploads: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ImageStore for FakeStore {
        async fn upload_with_thumbnail(&self, key_base: &str, _data: Vec<u8>, _content_type: &str) -> Result<()> {
            if *self.fail.lock().unwrap() {
                anyhow::bail!("simulated R2 outage");
            }
            self.uploads.lock().unwrap().push(key_base.to_owned());
            Ok(())
        }
    }

    async fn temp_db(name: &str) -> Db {
        let path = std::env::temp_dir().join(format!("fulgorart-import-{name}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Db::connect(&path).await.unwrap()
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbImage::new(w, h).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn post() -> ImportPost {
        ImportPost {
            source_type: "pixiv".into(),
            source_post_id: "1".into(),
            source_post_url: "https://www.pixiv.net/artworks/1".into(),
            author: Some(ImportAuthor {
                source_author_id: "9".into(),
                name: Some("artist".into()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn image(data: Vec<u8>) -> ImportImage {
        ImportImage {
            data,
            filename: "1_p0".into(),
            source_url: None,
        }
    }

    #[tokio::test]
    async fn uploads_new_image_and_skips_duplicate() {
        let (db, store) = (temp_db("new").await, FakeStore::default());
        let first = import_image(&db, &store, &post(), image(png(4, 3))).await.unwrap();
        let ImportOutcome::Uploaded { image_id, .. } = first else { panic!("{first:?}") };
        let asset = db.get_image_asset_by_id(image_id).await.unwrap().unwrap();
        assert_eq!((asset.width, asset.height, asset.content_type.as_str()), (Some(4), Some(3), "image/png"));
        assert_eq!(db.get_pending_tag_jobs(10).await.unwrap().len(), 1, "queued for tagging");

        let again = import_image(&db, &store, &post(), image(png(4, 3))).await.unwrap();
        assert!(matches!(again, ImportOutcome::Duplicate { .. }), "{again:?}");
        assert_eq!(store.uploads.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn rejects_truncated_image_without_writing() {
        let (db, store) = (temp_db("truncated").await, FakeStore::default());
        let mut data = png(64, 64);
        data.truncate(data.len() / 2);
        let outcome = import_image(&db, &store, &post(), image(data)).await.unwrap();
        assert!(matches!(outcome, ImportOutcome::Rejected(_)), "{outcome:?}");
        assert!(db.list_image_assets(1, 10).await.unwrap().is_empty());
        assert!(store.uploads.lock().unwrap().is_empty());
    }

    // Regression: a failed upload used to leave the row behind, so every later
    // import skipped the image as a duplicate and it was never uploaded.
    #[tokio::test]
    async fn failed_upload_is_retried_on_next_import() {
        let (db, store) = (temp_db("retry").await, FakeStore::default());
        *store.fail.lock().unwrap() = true;
        assert!(import_image(&db, &store, &post(), image(png(5, 5))).await.is_err());
        assert!(db.list_image_assets(1, 10).await.unwrap().is_empty(), "claim released");

        *store.fail.lock().unwrap() = false;
        let retry = import_image(&db, &store, &post(), image(png(5, 5))).await.unwrap();
        assert!(matches!(retry, ImportOutcome::Uploaded { .. }), "{retry:?}");
    }
}
