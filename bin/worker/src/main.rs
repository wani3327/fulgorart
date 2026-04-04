use anyhow::Result;
use fulgorart_core::AppConfig;
use fulgorart_db::Db;
use fulgorart_ingestor::{IngestorService, PixivAdapter, TwitterAdapter};
use fulgorart_storage::R2Client;
use fulgorart_tagger::{OnnxTagger, TaggerWorker};
use std::sync::Arc;

/// Interval between ingestor polling runs.
const INGESTOR_INTERVAL_SECS: u64 = 300; // 5 minutes

/// Interval between tagger polling runs.
const TAGGER_INTERVAL_SECS: u64 = 30;

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
    let config = Arc::new(config);

    tracing::info!("FulgorArt worker starting");

    // --- Tagger polling task ---
    let tagger_task = {
        let db = db.clone();
        let config = Arc::clone(&config);
        tokio::spawn(async move {
            let tagger = match OnnxTagger::new(
                &config.wd14_model_path,
                config.wd14_general_threshold,
                config.wd14_character_threshold,
            ) {
                Ok(t) => t,
                Err(e) => {
                    tracing::error!("Failed to initialize tagger: {}", e);
                    return;
                }
            };
            let worker = TaggerWorker::new(db, Box::new(tagger));
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(TAGGER_INTERVAL_SECS));
            tracing::info!("Tagger polling every {}s", TAGGER_INTERVAL_SECS);
            loop {
                interval.tick().await;
                match worker.run_once().await {
                    Ok(n) if n > 0 => tracing::info!("Tagger processed {} job(s)", n),
                    Ok(_) => tracing::debug!("Tagger: no pending jobs"),
                    Err(e) => tracing::error!("Tagger poll error: {}", e),
                }
            }
        })
    };

    // --- Ingestor polling task ---
    let ingestor_task = {
        let db = db.clone();
        let config = Arc::clone(&config);
        tokio::spawn(async move {
            let service = IngestorService::new(db, r2, (*config).clone());

            // Build adapters based on available environment credentials.
            let pixiv_token = std::env::var("PIXIV_ACCESS_TOKEN").ok();
            let twitter_token = std::env::var("TWITTER_BEARER_TOKEN").ok();

            if pixiv_token.is_none() && twitter_token.is_none() {
                tracing::warn!(
                    "No source adapter credentials found \
                     (PIXIV_ACCESS_TOKEN / TWITTER_BEARER_TOKEN). \
                     Ingestor will not fetch new posts."
                );
            }

            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(INGESTOR_INTERVAL_SECS));
            tracing::info!("Ingestor polling every {}s", INGESTOR_INTERVAL_SECS);
            loop {
                interval.tick().await;
                tracing::info!("Ingestor poll started");

                if let Some(ref token) = pixiv_token {
                    let adapter = PixivAdapter::new(token);
                    match service.run_adapter(&adapter).await {
                        Ok(n) => tracing::info!("Pixiv: ingested {} image(s)", n),
                        Err(e) => tracing::error!("Pixiv ingestor error: {}", e),
                    }
                }

                if let Some(ref token) = twitter_token {
                    let adapter = TwitterAdapter::new(token);
                    match service.run_adapter(&adapter).await {
                        Ok(n) => tracing::info!("Twitter: ingested {} image(s)", n),
                        Err(e) => tracing::error!("Twitter ingestor error: {}", e),
                    }
                }
            }
        })
    };

    // Run both tasks concurrently; if either exits (panic), abort.
    tokio::select! {
        res = tagger_task => {
            if let Err(e) = res {
                tracing::error!("Tagger task panicked: {}", e);
            }
        }
        res = ingestor_task => {
            if let Err(e) = res {
                tracing::error!("Ingestor task panicked: {}", e);
            }
        }
    }

    Ok(())
}
