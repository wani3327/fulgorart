mod pixiv;

use std::collections::HashMap;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use clap::Args as ClapArgs;
use fulgorart_db::Db;
use fulgorart_storage::R2Client;
use image::GenericImageView;
use sha2::Digest;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use pixiv::PixivGalleryDlJson3;

const MAX_CONCURRENT_UPLOADS: usize = 8;

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    /// Directory containing files downloaded by gallery-dl
    pub image_dir: PathBuf,
    /// gallery-dl JSON output that deserializes to PixivGalleryDlJson3. As file or from stdin
    pub json_file: Option<PathBuf>,
}

pub async fn run(args: Args, db: &Db, r2: &R2Client) -> Result<()> {
    let image_paths = index_image_paths(&args.image_dir).await.with_context(|| {
        format!(
            "Failed to index image directory '{}'",
            args.image_dir.display()
        )
    })?;
    let json = match &args.json_file {
        Some(f) => tokio::fs::read_to_string(f)
            .await
            .with_context(|| format!("Failed to read JSON file '{}'", f.display()))?,
        None => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            input
        }
    };
    let entries: PixivGalleryDlJson3 =
        serde_json::from_str(&json).with_context(|| format!("Failed to parse JSON"))?;

    let mut imported_posts = 0usize;
    let mut inserted_assets = 0usize;
    let mut skipped_assets = 0usize;
    let upload_slots = std::sync::Arc::new(Semaphore::new(MAX_CONCURRENT_UPLOADS));
    let mut upload_tasks = JoinSet::new();

    // for entry in entries {
    for (_i, entry) in entries.into_iter().enumerate() {
        let post_info = entry.clone().post();
        let item_info = entry.item();
        let source_post_id = post_info.id.to_string();

        // find post from DB
        let post_id = match db
            .get_post_by_source(&post_info.category, &source_post_id)
            .await?
        {
            Some(post) => post.id,
            None => {
                // if not found, insert post
                let raw_json = String::from_utf8(post_info.compressed.clone())
                    .context("Compressed Pixiv metadata was not valid UTF-8 JSON")?;
                let post = db
                    .insert_post_with_details(
                        &post_info.category,
                        &source_post_id,
                        &format!("https://www.pixiv.net/artworks/{}", post_info.id),
                        Some(&post_info.date),
                        Some(&post_info.user.id.to_string()),
                        Some(&post_info.user.name),
                        Some(&post_info.user.url),
                        Some(&post_info.user.profile_url),
                        Some(&post_info.title),
                        Some(&post_info.caption),
                        Some(&raw_json),
                    )
                    .await?;
                imported_posts += 1;
                post.id
            }
        };

        // read image
        let Some(image_path) =
            image_paths.get(&format!("{}.{}", item_info.filename, item_info.extension))
        else {
            println!(
                "JSON references '{}', but it was not found under '{}'",
                item_info.filename,
                args.image_dir.display()
            );
            continue;
        };

        let Ok(data) = tokio::fs::read(image_path).await else {
            println!("Failed to read image '{}'", image_path.display());
            continue;
        };

        // find metadata
        let (sha256, width, height) = image_metadata(&data);
        let content_type = content_type_for_ext(&item_info.extension);
        let now = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let s3_key = format!(
            "{}/{}.{}.{}",
            post_info.category, item_info.filename, now, item_info.extension
        );
        let file_size = data.len() as i64;

        // check duplication to DB
        let Some(claimed_upload) = db
            .claim_image_asset_upload_with_filename(
                Some(post_id),
                &sha256,
                &s3_key,
                Some(&item_info.filename),
                width,
                height,
                Some(file_size),
                content_type,
                Some(&item_info.url),
            )
            .await?
        else {
            // duplicated. skip uploading.
            let existing_asset = db.get_image_asset_by_sha256(&sha256).await?;
            match existing_asset {
                Some(existing_asset) => println!(
                    "skipped_duplicate filename={} path={} sha256={} existing_image_id={} existing_s3_key={}",
                    item_info.filename,
                    image_path.display(),
                    sha256,
                    existing_asset.id,
                    existing_asset.s3_key
                ),
                None => println!(
                    "skipped_duplicate filename={} path={} sha256={} reason=claim_lost",
                    item_info.filename,
                    image_path.display(),
                    sha256
                ),
            }
            skipped_assets += 1;
            continue;
        };

        let permit = upload_slots.clone().acquire_owned().await?;
        let db = db.clone();
        let r2 = r2.clone();
        let filename = item_info.filename;
        let image_path = image_path.clone();
        let job_id = claimed_upload.job.id;
        let image_id = claimed_upload.asset.id;

        upload_tasks.spawn(async move {
            let _permit = permit;
            let upload_result = r2
                .upload(&s3_key, bytes::Bytes::from(data), content_type)
                .await
                .with_context(|| {
                    format!(
                        "Failed to upload '{}' to key '{}'",
                        image_path.display(),
                        s3_key
                    )
                });

            match upload_result {
                Ok(()) => {
                    db.update_tag_job_status(job_id, "uploaded", None).await?;
                    println!(
                        "uploaded filename={} image_id={} key={}",
                        filename, image_id, s3_key
                    );
                }
                Err(error) => {
                    let message = format!("{error:#}");
                    db.update_tag_job_status(job_id, "failed", Some(&message))
                        .await?;
                    return Err(error);
                }
            }

            Result::<usize>::Ok(1)
        });
    }

    while let Some(result) = upload_tasks.join_next().await {
        inserted_assets += result.context("gallery-dl upload task panicked")??;
    }

    println!(
        "imported_posts={} imported_images={} skipped_images={}",
        imported_posts, inserted_assets, skipped_assets
    );
    Ok(())
}

/// Make HashMap(filename -> full path)
async fn index_image_paths(image_dir: &Path) -> Result<HashMap<String, PathBuf>> {
    let mut result = HashMap::new();
    let mut pending = vec![image_dir.to_path_buf()];

    while let Some(dir) = pending.pop() {
        let mut entries = tokio::fs::read_dir(&dir)
            .await
            .with_context(|| format!("Failed to read directory '{}'", dir.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let file_type = entry.file_type().await?;
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file() {
                if let Some(filename) = path.file_name().and_then(|value| value.to_str()) {
                    result.insert(filename.to_string(), path);
                }
            }
        }
    }

    Ok(result)
}

fn image_metadata(data: &[u8]) -> (String, Option<i64>, Option<i64>) {
    let mut hasher = sha2::Sha256::new();
    hasher.update(data);
    let sha256 = hex::encode(hasher.finalize());
    let (width, height) = match image::load_from_memory(data) {
        Ok(image) => {
            let (width, height) = image.dimensions();
            (Some(width as i64), Some(height as i64))
        }
        Err(_) => (None, None),
    };

    (sha256, width, height)
}

fn content_type_for_ext(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        _ => "else",
    }
}

pub struct PostInterested {
    category: String,
    date: String,
    user: UserInterested,
    id: i64,
    caption: String,
    title: String,
    compressed: Vec<u8>,
}

struct UserInterested {
    id: i64,
    name: String,
    url: String,
    profile_url: String,
}

pub trait PostInterest {
    fn post(self) -> PostInterested;
}

pub struct ItemInterested {
    extension: String,
    filename: String,
    url: String,
}

pub trait ItemInterest {
    fn item(self) -> ItemInterested;
}
