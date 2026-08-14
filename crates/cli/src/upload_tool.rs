use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use fulgorart_db::Db;
use fulgorart_storage::R2Client;
use image::GenericImageView;
use sha2::Digest;

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    /// Path to local image file
    #[arg(long)]
    pub file: String,
    /// Source type (pixiv, twitter, etc.)
    #[arg(long)]
    pub source_type: String,
    /// Source post ID
    #[arg(long)]
    pub source_post_id: String,
    /// Source post URL
    #[arg(long)]
    pub source_post_url: String,
    /// Author name (optional)
    #[arg(long)]
    pub author_name: Option<String>,
    /// Author ID (optional)
    #[arg(long)]
    pub author_id: Option<String>,
}

fn content_type_for_ext(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "image/jpeg",
    }
}

pub async fn run(args: Args, db: &Db, r2: &R2Client) -> Result<()> {
    let path = std::path::Path::new(&args.file);
    let data = tokio::fs::read(path)
        .await
        .with_context(|| format!("Failed to read local file '{}'", args.file))?;
    let bytes = bytes::Bytes::from(data.clone());

    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("jpg")
        .to_lowercase();
    let content_type = content_type_for_ext(&ext);

    let mut hasher = sha2::Sha256::new();
    hasher.update(&data);
    let sha256 = hex::encode(hasher.finalize());

    let (width, height) = match image::load_from_memory(&data) {
        Ok(image) => {
            let (w, h) = image.dimensions();
            (Some(w as i64), Some(h as i64))
        }
        Err(_) => (None, None),
    };

    let key = ""; //R2Client::canonical_key(&args.source_type, &sha256, &ext);
    let r2_url = r2.object_url(&key);
    r2.upload(&key, bytes, content_type)
        .await
        .with_context(|| format!("Failed to upload '{}' to key '{}'", args.file, key))?;

    let post = db
        .insert_post(
            &args.source_type,
            &args.source_post_id,
            &args.source_post_url,
            None,
            args.author_id.as_deref(),
            args.author_name.as_deref(),
            None,
            None,
        )
        .await?;

    let asset = db
        .insert_image_asset(
            Some(post.id),
            &sha256,
            &key,
            width,
            height,
            Some(data.len() as i64),
            content_type,
            None,
        )
        .await?;

    let job = db.ensure_tag_job(asset.id).await?;

    println!("uploaded={} key={}", args.file, key);
    println!("image_id={} sha256={}", asset.id, sha256);
    println!("r2_url={}", r2_url);
    println!("tag_job_id={}", job.id);
    Ok(())
}
