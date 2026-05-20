use anyhow::Result;
use async_trait::async_trait;
use fulgorart_ingestor::{
    GrabbedPost, ImageGrabberService, PixivAdapter, SourceAdapter, TwitterAdapter,
};

use crate::BridgeConfig;

#[async_trait]
pub trait ImageGrabberJob: Send + Sync {
    async fn grab_liked_posts(&self) -> Result<Vec<GrabbedPost>>;
}

pub struct IngestorImageGrabberJob {
    service: ImageGrabberService,
}

impl IngestorImageGrabberJob {
    pub fn from_config(config: &BridgeConfig) -> Self {
        let mut adapters: Vec<Box<dyn SourceAdapter>> = Vec::new();

        if let Some(token) = &config.pixiv_access_token {
            adapters.push(Box::new(PixivAdapter::new(
                token,
                config.pixiv_user_id.clone(),
            )));
        }

        if let Some(token) = &config.twitter_bearer_token {
            adapters.push(Box::new(TwitterAdapter::new(token)));
        }

        Self {
            service: ImageGrabberService::new(adapters),
        }
    }
}

#[async_trait]
impl ImageGrabberJob for IngestorImageGrabberJob {
    async fn grab_liked_posts(&self) -> Result<Vec<GrabbedPost>> {
        self.service.grab_all().await
    }
}
