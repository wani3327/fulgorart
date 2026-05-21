use anyhow::Result;

use fulgorart_bridge::{
    run_once, BridgeConfig, CloudRunTaggerJob, LocalTaggerJob, MyImageGrabJob, R2StorageJob,
    TaggerJobMode,
};

use fulgorart_db::Db;
use fulgorart_storage::R2Client;

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

    let config = BridgeConfig::from_env()?;
    let db = Db::connect(&config.db.path).await?;
    let r2 = R2Client::new(&config.r2).await?;

    let image_grabber = MyImageGrabJob::from_config(&config);
    let storage = R2StorageJob::new(db.clone(), r2);
    match config.tagger_job_mode {
        TaggerJobMode::CloudRun => {
            let cloud_run_config = config
                .cloud_run
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Missing Cloud Run config"))?;
            let tagger_job = CloudRunTaggerJob::new(db, cloud_run_config).await?;
            run_once(&image_grabber, &storage, &tagger_job).await?;
        }
        TaggerJobMode::Local => {
            let tagger_job = LocalTaggerJob::new(db, config.tagger_batch_size);
            run_once(&image_grabber, &storage, &tagger_job).await?;
        }
    }
    Ok(())
}
