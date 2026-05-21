mod config;
mod jobs;

pub use config::{BridgeConfig, CloudRunConfig, TaggerJobMode};
pub use jobs::{
    image_grabber::MyImageGrabJob,
    storage::R2StorageJob,
    tagger_job::{CloudRunTaggerJob, LocalTaggerJob},
};

use anyhow::Result;

use crate::jobs::{image_grabber::ImageGrabJob, storage::StorageJob, tagger_job::TaggerJob};

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
