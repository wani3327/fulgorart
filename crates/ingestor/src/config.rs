use crate::{PixivAdapter, SourceAdapter, TwitterAdapter};

#[derive(Debug, Clone)]
pub struct IngestorConfig {
    pub default_output_dir: String,
    pub pixiv_access_token: Option<String>,
    pub pixiv_user_id: Option<String>,
    pub twitter_bearer_token: Option<String>,
}

impl IngestorConfig {
    pub fn from_env() -> Self {
        dotenvy::dotenv().ok();
        Self {
            default_output_dir: std::env::var("FULGORART_INGESTOR_OUTPUT_DIR")
                .unwrap_or_else(|_| "./data/ingestor".to_string()),
            pixiv_access_token: std::env::var("PIXIV_ACCESS_TOKEN").ok(),
            pixiv_user_id: std::env::var("PIXIV_USER_ID").ok(),
            twitter_bearer_token: std::env::var("TWITTER_BEARER_TOKEN").ok(),
        }
    }

    pub fn build_adapters(&self) -> Vec<Box<dyn SourceAdapter>> {
        let mut adapters: Vec<Box<dyn SourceAdapter>> = Vec::new();

        if let Some(token) = &self.pixiv_access_token {
            adapters.push(Box::new(PixivAdapter::new(
                token,
                self.pixiv_user_id.clone(),
            )));
        }

        if let Some(token) = &self.twitter_bearer_token {
            adapters.push(Box::new(TwitterAdapter::new(token)));
        }

        adapters
    }
}
