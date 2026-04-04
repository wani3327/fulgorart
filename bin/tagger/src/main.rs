use anyhow::Result;
use fulgorart_core::AppConfig;
use fulgorart_db::Db;
use fulgorart_tagger::{OnnxTagger, TaggerWorker};

/// Process every pending tag job in the database, then exit.
/// Intended to be invoked by cron (e.g. every minute).
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

    let tagger = OnnxTagger::new(
        &config.wd14_model_path,
        config.wd14_general_threshold,
        config.wd14_character_threshold,
    )?;

    let worker = TaggerWorker::new(db, Box::new(tagger));
    let n = worker.run_once().await?;
    tracing::info!("Tagger: processed {} job(s)", n);

    Ok(())
}
