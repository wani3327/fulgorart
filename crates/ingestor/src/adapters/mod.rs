mod pixiv;

pub use pixiv::PixivAdapter;

use anyhow::Result;
use async_trait::async_trait;
use bytes::Bytes;

use crate::SourcePost;

/// A trait for adapters that can fetch liked posts and download images from different sources.
/// Basically a crawler for a specific service.
#[async_trait]
pub trait SourceAdapter: Send + Sync {
    fn source_type(&self) -> &str;
    async fn fetch_liked_posts(&self, since: Option<&str>) -> Result<Vec<SourcePost>>;
    async fn download_image(&self, url: &str) -> Result<(Bytes, String)>;
}

pub struct TwitterAdapter {
    bearer_token: String,
    client: reqwest::Client,
}

impl TwitterAdapter {
    pub fn new(bearer_token: &str) -> Self {
        Self {
            bearer_token: bearer_token.to_string(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl SourceAdapter for TwitterAdapter {
    fn source_type(&self) -> &str {
        "twitter"
    }

    async fn fetch_liked_posts(&self, _since: Option<&str>) -> Result<Vec<SourcePost>> {
        tracing::warn!("TwitterAdapter::fetch_liked_posts is a stub");
        Ok(vec![])
    }

    async fn download_image(&self, url: &str) -> Result<(Bytes, String)> {
        let resp = self
            .client
            .get(url)
            .bearer_auth(&self.bearer_token)
            .send()
            .await?
            .error_for_status()?;
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();
        let data = resp.bytes().await?;
        Ok((data, content_type))
    }
}
