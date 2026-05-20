use anyhow::Result;

use crate::{
    jobs::{
        image_grabber::{ImageGrabberJob, IngestorImageGrabberJob},
        job_delegate::{CloudRunJobDelegate, JobDelegate},
        storage::{R2StorageJob, StorageJob},
    },
    BridgeConfig,
};

pub struct BridgeService {
    image_grabber: Box<dyn ImageGrabberJob>,
    storage: Box<dyn StorageJob>,
    delegate: Box<dyn JobDelegate>,
}

impl BridgeService {
    pub async fn new(config: BridgeConfig) -> Result<Self> {
        let db = fulgorart_db::Db::connect(&config.db.path).await?;
        let r2 = fulgorart_storage::R2Client::new(&config.r2).await?;

        let image_grabber = Box::new(IngestorImageGrabberJob::from_config(&config));
        let storage = Box::new(R2StorageJob::new(db.clone(), r2));
        let delegate = Box::new(CloudRunJobDelegate::new(db, config.cloud_run).await?);

        Ok(Self {
            image_grabber,
            storage,
            delegate,
        })
    }

    pub async fn run_once(&self) -> Result<()> {
        let posts = self.image_grabber.grab_liked_posts().await?;
        if posts.is_empty() {
            tracing::info!("Image grabber returned no liked posts");
        } else {
            let stored = self.storage.store_posts(posts).await?;
            tracing::info!(stored, "Stored grabbed images");
        }

        self.delegate.dispatch_pending_jobs().await
    }
}
