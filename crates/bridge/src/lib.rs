mod config;

use anyhow::Result;
use async_trait::async_trait;

pub use config::{BridgeConfig, CloudRunConfig, TaggerJobMode};

#[async_trait]
pub trait ImageGrabJob: Send + Sync {
    async fn grab_liked_posts(&self) -> Result<Vec<fulgorart_ingestor::GrabbedPost>>;
}

#[async_trait]
pub trait StorageJob: Send + Sync {
    async fn store_posts(&self, posts: Vec<fulgorart_ingestor::GrabbedPost>) -> Result<usize>;
}

#[async_trait]
pub trait TaggerJob: Send + Sync {
    async fn tag(&self) -> Result<()>;
}

pub async fn run_once<IG: ImageGrabJob, S: StorageJob, T: TaggerJob>(
    image_grabber: &IG,
    storage: &S,
    tagger_job: &T,
) -> Result<()> {
    let posts = image_grabber.grab_liked_posts().await?;
    if posts.is_empty() {
        tracing::info!("Image grabber returned no liked posts");
    } else {
        let stored = storage.store_posts(posts).await?;
        tracing::info!(stored, "Stored grabbed images");
    }

    tagger_job.tag().await
}
