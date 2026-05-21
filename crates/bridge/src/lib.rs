mod config;
mod jobs;

pub use config::{BridgeConfig, CloudRunConfig};
pub use jobs::{image_grabber::MyImageGrabJob, job_delegate::CloudRunJobDelegate, storage::R2StorageJob};

use anyhow::Result;

use crate::jobs::{image_grabber::ImageGrabJob, job_delegate::JobDelegate, storage::StorageJob};

pub async fn run_once<IG: ImageGrabJob, S: StorageJob, D: JobDelegate>(
    image_grabber: &IG,
    storage: &S,
    delegate: &D,
) -> Result<()> {
    let posts = image_grabber.grab_liked_posts().await?;
    if posts.is_empty() {
        tracing::info!("Image grabber returned no liked posts");
    } else {
        let stored = storage.store_posts(posts).await?;
        tracing::info!(stored, "Stored grabbed images");
    }

    delegate.dispatch_pending_jobs().await
}
