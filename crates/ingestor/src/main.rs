use anyhow::Result;
use std::path::PathBuf;

use fulgorart_ingestor::{grab, save_grabbed_posts, PixivAdapter, TwitterAdapter};

fn output_dir_from_args(default_output_dir: &str) -> PathBuf {
    let mut args = std::env::args().skip(1);
    if let Some(arg) = args.next() {
        return PathBuf::from(arg);
    }
    PathBuf::from(default_output_dir)
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .try_init()
        .ok();

    let output_dir = output_dir_from_args(
        std::env::var("FULGORART_INGESTOR_OUTPUT_DIR")
            .unwrap_or_else(|_| "./data/ingestor".to_string())
            .as_str(),
    );

    let mut posts = Vec::new();
    let pixiv = PixivAdapter::new(
        std::env::var("PIXIV_ACCESS_TOKEN")?.as_str(),
        std::env::var("PIXIV_USER_ID").ok(),
    );
    let twitter = TwitterAdapter::new(std::env::var("TWITTER_BEARER_TOKEN")?.as_str());

    posts.extend(grab(&pixiv).await?);
    posts.extend(grab(&twitter).await?);

    let saved = save_grabbed_posts(&output_dir, &posts).await?;

    tracing::info!(saved, output_dir = %output_dir.display(), "Ingestor finished");
    Ok(())
}
