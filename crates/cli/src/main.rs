use anyhow::Result;
use clap::{Parser, Subcommand};
use fulgorart_core::AppConfig;
use fulgorart_db::Db;
use fulgorart_storage::R2Client;
use image::GenericImageView;
use sha2::Digest;

#[derive(Parser)]
#[command(name = "fulgorart-cli", about = "FulgorArt command-line tool")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Import an image file into the database and R2 storage
    ImportImage {
        /// Path to the image file
        #[arg(long)]
        file: String,
        /// Source type (pixiv, twitter)
        #[arg(long)]
        source_type: String,
        /// Source post ID
        #[arg(long)]
        source_post_id: String,
        /// Source post URL
        #[arg(long)]
        source_post_url: String,
        /// Author name (optional)
        #[arg(long)]
        author_name: Option<String>,
        /// Author ID (optional)
        #[arg(long)]
        author_id: Option<String>,
    },
    /// List images in the database
    ListImages {
        #[arg(long, default_value = "1")]
        page: i64,
        #[arg(long, default_value = "20")]
        per_page: i64,
    },
    /// Search tags
    SearchTags { query: String },
    /// Add a tag to an image
    AddTag {
        #[arg(long)]
        image_id: i64,
        #[arg(long)]
        tag: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let cli = Cli::parse();
    let config = AppConfig::from_env()?;
    let db = Db::connect(&config.db_path).await?;

    match cli.command {
        Commands::ImportImage {
            file,
            source_type,
            source_post_id,
            source_post_url,
            author_name,
            author_id,
        } => {
            let path = std::path::Path::new(&file);
            let data = tokio::fs::read(path).await?;
            let bytes = bytes::Bytes::from(data.clone());

            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("jpg")
                .to_lowercase();
            let content_type = match ext.as_str() {
                "png" => "image/png",
                "gif" => "image/gif",
                "webp" => "image/webp",
                _ => "image/jpeg",
            };

            let mut hasher = sha2::Sha256::new();
            hasher.update(&data);
            let sha256 = hex::encode(hasher.finalize());

            let (width, height) = match image::load_from_memory(&data) {
                Ok(img) => {
                    let (w, h) = img.dimensions();
                    (Some(w as i64), Some(h as i64))
                }
                Err(_) => (None, None),
            };

            let r2 = R2Client::new(&config).await?;
            let key = R2Client::canonical_key(&source_type, &sha256, &ext);
            let r2_url = r2.object_url(&config.r2_bucket, &key);

            println!("Uploading {} -> {}", file, key);
            r2.upload(&config.r2_bucket, &key, bytes, content_type)
                .await?;

            let post = db
                .insert_post(
                    &source_type,
                    &source_post_id,
                    &source_post_url,
                    None,
                    author_name.as_deref(),
                    author_id.as_deref(),
                    None,
                )
                .await?;

            let asset = db
                .insert_image_asset(
                    Some(post.id),
                    &sha256,
                    &key,
                    &r2_url,
                    width,
                    height,
                    Some(data.len() as i64),
                    content_type,
                    None,
                )
                .await?;

            println!("Imported image id={} sha256={}", asset.id, sha256);
            println!("R2 URL: {}", r2_url);

            let job = db.insert_tag_job(asset.id).await?;
            println!("Tag job queued: id={}", job.id);
        }

        Commands::ListImages { page, per_page } => {
            let images = db.list_image_assets(page, per_page).await?;
            println!("{} images (page {})", images.len(), page);
            for img in images {
                println!(
                    "  [{}] {} {}x{} {}",
                    img.id,
                    img.sha256.chars().take(12).collect::<String>(),
                    img.width.unwrap_or(0),
                    img.height.unwrap_or(0),
                    img.r2_url
                );
            }
        }

        Commands::SearchTags { query } => {
            let tags = db.search_tags(&query).await?;
            for tag in tags {
                println!(
                    "[{}] {} ({})",
                    tag.id,
                    tag.name,
                    tag.category.as_deref().unwrap_or("uncategorized")
                );
            }
        }

        Commands::AddTag { image_id, tag } => {
            let tag_row = db.get_or_create_tag(&tag, None).await?;
            db.insert_image_tag(image_id, tag_row.id, "manual", None)
                .await?;
            println!(
                "Added tag '{}' (id={}) to image {}",
                tag, tag_row.id, image_id
            );
        }
    }

    Ok(())
}
