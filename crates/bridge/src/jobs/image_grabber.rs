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
}

impl MyImageGrabJob {
    pub async fn grab_liked_posts(&self) -> Result<Vec<GrabbedPost>> {
        let mut posts = Vec::new();

        let pixiv_posts = grab(&self.pixiv).await?;
        posts.extend(pixiv_posts);

        let twitter_posts = grab(&self.twitter).await?;
        posts.extend(twitter_posts);

        Ok(posts)
    }
}
