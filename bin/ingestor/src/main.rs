use anyhow::Result;
use fulgorart_core::AppConfig;
use fulgorart_db::Db;
use fulgorart_ingestor::{IngestorService, PixivAdapter, TwitterAdapter};
use fulgorart_storage::R2Client;

/// Fetch liked posts from all configured source adapters, download images, and exit.
/// Intended to be invoked by cron (e.g. every 5 minutes).
#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = AppConfig::from_env()?;
    let db = Db::connect(&config.db_path).await?;
    let r2 = R2Client::new(&config).await?;

    let service = IngestorService::new(db, r2, config);

    let pixiv_token = std::env::var("PIXIV_ACCESS_TOKEN").ok();
    let twitter_token = std::env::var("TWITTER_BEARER_TOKEN").ok();

    if pixiv_token.is_none() && twitter_token.is_none() {
        tracing::warn!(
            "No adapter credentials found \
             (PIXIV_ACCESS_TOKEN / TWITTER_BEARER_TOKEN). Nothing to do."
        );
        return Ok(());
    }

    if let Some(ref token) = pixiv_token {
        let adapter = PixivAdapter::new(token);
        let n = service.run_adapter(&adapter).await?;
        tracing::info!("Pixiv: ingested {} image(s)", n);
    }

    if let Some(ref token) = twitter_token {
        let adapter = TwitterAdapter::new(token);
        let n = service.run_adapter(&adapter).await?;
        tracing::info!("Twitter: ingested {} image(s)", n);
    }

    Ok(())
}
