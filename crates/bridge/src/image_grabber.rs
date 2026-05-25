use anyhow::Result;
use fulgorart_ingestor::{grab, GrabbedPost, PixivAdapter, TwitterAdapter};

use crate::config::BridgeConfig;

pub struct MyImageGrabJob {
    pixiv: PixivAdapter,
    twitter: TwitterAdapter,
}

impl MyImageGrabJob {
    pub fn from_config(config: &BridgeConfig) -> Self {
        Self {
            pixiv: PixivAdapter::new(
                config.pixiv_access_token.as_deref().unwrap_or(""),
                config.pixiv_user_id.clone(),
            ),
            twitter: TwitterAdapter::new(config.twitter_bearer_token.as_deref().unwrap_or("")),
        }
    }

    pub async fn grab_liked_posts(&self) -> Result<Vec<GrabbedPost>> {
        let mut posts = Vec::new();
        posts.extend(grab(&self.pixiv).await?);
        posts.extend(grab(&self.twitter).await?);
        Ok(posts)
    }
}
