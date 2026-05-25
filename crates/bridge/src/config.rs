use anyhow::{Context, Result};
use fulgorart_db::DbConfig;
use fulgorart_storage::R2Config;

#[derive(Debug, Clone)]
pub struct CloudRunConfig {
    pub project_id: String,
    pub region: String,
    pub job_name: String,
    pub tagger_batch_size: usize,
}

#[derive(Debug, Clone)]
pub struct BridgeConfig {
    pub db: DbConfig,
    pub r2: R2Config,
    pub tagger_batch_size: usize,
    pub pixiv_access_token: Option<String>,
    pub pixiv_user_id: Option<String>,
    pub twitter_bearer_token: Option<String>,
}

impl BridgeConfig {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();
        let tagger_batch_size: usize = std::env::var("TAGGER_BATCH_SIZE")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(20);

        Ok(Self {
            db: DbConfig::from_env(),
            r2: R2Config::from_env(),
            tagger_batch_size,
            pixiv_access_token: std::env::var("PIXIV_ACCESS_TOKEN").ok(),
            pixiv_user_id: std::env::var("PIXIV_USER_ID").ok(),
            twitter_bearer_token: std::env::var("TWITTER_BEARER_TOKEN").ok(),
        })
    }
}

pub fn cloud_run_config_from_env(tagger_batch_size: usize) -> Result<CloudRunConfig> {
    dotenvy::dotenv().ok();
    Ok(CloudRunConfig {
        project_id: std::env::var("GCP_PROJECT_ID").context("GCP_PROJECT_ID is required")?,
        region: std::env::var("GCP_REGION").context("GCP_REGION is required")?,
        job_name: std::env::var("CLOUD_RUN_JOB_NAME").context("CLOUD_RUN_JOB_NAME is required")?,
        tagger_batch_size,
    })
}
