mod config;
mod jobs;

use anyhow::Result;

use crate::config::{cloud_run_config_from_env, BridgeConfig};
use crate::jobs::{
    image_grabber::MyImageGrabJob,
    storage::R2StorageJob,
    tagger_job::{CloudRunTaggerJob, LocalTaggerJob},
};

use fulgorart_db::Db;
use fulgorart_storage::R2Client;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaggerMode {
    Local,
    CloudRun,
}

fn print_usage() {
    eprintln!("Usage:");
    eprintln!("  fulgorart-bridge --tagger-mode <local|cloud_run>");
}

fn parse_mode_arg() -> Result<TaggerMode> {
    let mut args = std::env::args().skip(1);
    let mut mode: Option<TaggerMode> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            "--tagger-mode" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("Missing value for --tagger-mode"))?;
                mode = Some(match value.as_str() {
                    "local" => TaggerMode::Local,
                    "cloud_run" => TaggerMode::CloudRun,
                    _ => anyhow::bail!("--tagger-mode must be either 'local' or 'cloud_run'"),
                });
            }
            other => anyhow::bail!("Unknown argument: {other}"),
        }
    }

    mode.ok_or_else(|| anyhow::anyhow!("--tagger-mode is required"))
}

async fn run_once(image_grabber: &MyImageGrabJob, storage: &R2StorageJob) -> Result<()> {
    let posts = image_grabber.grab_liked_posts().await?;
    if posts.is_empty() {
        tracing::info!("Image grabber returned no liked posts");
        return Ok(());
    }

    let stored = storage.store_posts(posts).await?;
    tracing::info!(stored, "Stored grabbed images");
    Ok(())
}

fn install_rustls_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    install_rustls_crypto_provider();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let tagger_mode = parse_mode_arg()?;
    let config = BridgeConfig::from_env()?;
    let db = Db::connect(&config.db.path).await?;
    let r2 = R2Client::new(&config.r2).await?;

    let image_grabber = MyImageGrabJob::from_config(&config);
    let storage = R2StorageJob::new(db.clone(), r2);
    run_once(&image_grabber, &storage).await?;

    match tagger_mode {
        TaggerMode::CloudRun => {
            let cloud_run_config = cloud_run_config_from_env(config.tagger_batch_size)?;
            let tagger_job = CloudRunTaggerJob::new(db, cloud_run_config).await?;
            tagger_job.tag().await?;
        }
        TaggerMode::Local => {
            let tagger_job =
                LocalTaggerJob::new(db, config.r2.clone(), config.tagger_batch_size).await?;
            tagger_job.tag().await?;
        }
    }

    Ok(())
}
