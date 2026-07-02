mod config;
mod image_grabber;
mod storage;
mod tagger_job;

use anyhow::Result;
use std::collections::HashMap;

use crate::config::{cloud_run_config_from_env, BridgeConfig};
use crate::image_grabber::MyImageGrabJob;
use crate::storage::R2StorageJob;
use crate::tagger_job::{BridgeTagPrediction, BridgeTagResult, CloudRunTaggerJob, LocalTaggerJob};

use fulgorart_db::{Db, TagJobWithKey};
use fulgorart_ingestor::GrabbedPost;
use fulgorart_storage::R2Client;
use fulgorart_tagger::Wd14Labels;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaggerMode {
    Local,
    CloudRun,
}

fn print_usage() {
    eprintln!("Usage:");
    eprintln!("  fulgorart-bridge --tagger-mode <local|cloud_run>");
}

fn parse_mode_arg() -> Result<TaggerMode> {
    let mut args = std::env::args().skip(1);
    let mut mode: Option<TaggerMode> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            "--tagger-mode" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("Missing value for --tagger-mode"))?;
                mode = Some(match value.as_str() {
                    "local" => TaggerMode::Local,
                    "cloud_run" => TaggerMode::CloudRun,
                    _ => anyhow::bail!("--tagger-mode must be either 'local' or 'cloud_run'"),
                });
            }
            other => anyhow::bail!("Unknown argument: {other}"),
        }
    }

    mode.ok_or_else(|| anyhow::anyhow!("--tagger-mode is required"))
}

async fn ingest_post(db: &Db, storage: &R2StorageJob, post: GrabbedPost) -> Result<usize> {
    let post_row = db
        .insert_post(
            &post.source_type,
            &post.source_post_id,
            &post.source_post_url,
            post.liked_at.as_deref(),
            post.author_source_id.as_deref(),
            post.author_name.as_deref(),
            post.author_url.as_deref(),
            post.raw_json.as_deref(),
        )
        .await?;
    let stored_images = storage.store_post(post).await?;

    for image in &stored_images {
        let asset = db
            .insert_image_asset(
                Some(post_row.id),
                &image.sha256,
                &image.s3_key,
                None,
                None,
                Some(image.file_size),
                &image.content_type,
                Some(&image.source_url),
            )
            .await?;
        db.ensure_tag_job(asset.id).await?;
    }

    Ok(stored_images.len())
}

async fn run_ingest_once(
    db: &Db,
    image_grabber: &MyImageGrabJob,
    storage: &R2StorageJob,
) -> Result<()> {
    let posts = image_grabber.grab_liked_posts().await?;
    if posts.is_empty() {
        tracing::info!("Image grabber returned no liked posts");
        return Ok(());
    }

    let mut stored = 0usize;
    for post in posts {
        stored += ingest_post(db, storage, post).await?;
    }

    tracing::info!(stored, "Stored grabbed images");
    Ok(())
}

async fn apply_job_tags(
    db: &Db,
    labels: &Wd14Labels,
    job: &TagJobWithKey,
    tags: &[BridgeTagPrediction],
) -> Result<()> {
    for prediction in tags {
        let label = labels
            .label_for_tag_id(prediction.tag_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown WD14 tag id: {}", prediction.tag_id))?;
        let tag = db
            .get_or_create_tag(&label.name, Some(&label.category))
            .await?;
        db.insert_image_tag(job.image_id, tag.id, "wd14", Some(prediction.score as f64))
            .await?;
    }
    db.update_tag_job_status(job.job_id, "done", None).await?;
    Ok(())
}

async fn mark_job_failed(db: &Db, job: &TagJobWithKey, message: &str) -> Result<()> {
    db.update_tag_job_status(job.job_id, "failed", Some(message))
        .await?;
    Ok(())
}

async fn run_local_tagger(db: &Db, tagger: &LocalTaggerJob, batch_size: usize) -> Result<()> {
    let batch_size = batch_size.max(1) as i64;

    loop {
        let jobs = db.get_pending_tag_jobs_with_keys(batch_size).await?;
        if jobs.is_empty() {
            tracing::info!("No pending tag jobs");
            break;
        }

        for job in &jobs {
            db.update_tag_job_status(job.job_id, "running", None)
                .await?;
        }

        for job in &jobs {
            match tagger.tag_r2_key(&job.s3_key).await {
                Ok(tags) => apply_job_tags(db, tagger.labels(), job, &tags).await?,
                Err(error) => {
                    let message = format!("{error:#}");
                    mark_job_failed(db, job, &message).await?;
                }
            }
        }

        if (jobs.len() as i64) < batch_size {
            break;
        }
    }

    Ok(())
}

async fn run_cloud_tagger(db: &Db, tagger: &CloudRunTaggerJob, batch_size: usize) -> Result<()> {
    let batch_size = batch_size.max(1) as i64;

    loop {
        let jobs = db.get_pending_tag_jobs_with_keys(batch_size).await?;
        if jobs.is_empty() {
            tracing::info!("No pending tag jobs");
            break;
        }

        for job in &jobs {
            db.update_tag_job_status(job.job_id, "running", None)
                .await?;
        }

        let keys: Vec<String> = jobs.iter().map(|job| job.s3_key.clone()).collect();
        match tagger.tag(&keys).await {
            Ok(results) => {
                let result_map: HashMap<&str, &BridgeTagResult> = results
                    .iter()
                    .map(|result| (result.key.as_str(), result))
                    .collect();
                for job in &jobs {
                    match result_map.get(job.s3_key.as_str()) {
                        Some(result) => apply_job_tags(db, tagger.labels(), job, &result.tags).await?,
                        None => {
                            let message =
                                format!("No tag output found for storage key: {}", job.s3_key);
                            mark_job_failed(db, job, &message).await?;
                        }
                    }
                }
            }
            Err(error) => {
                let message = format!("{error:#}");
                for job in &jobs {
                    mark_job_failed(db, job, &message).await?;
                }
            }
        }

        if (jobs.len() as i64) < batch_size {
            break;
        }
    }

    Ok(())
}

fn install_rustls_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    install_rustls_crypto_provider();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let tagger_mode = parse_mode_arg()?;
    let config = BridgeConfig::from_env()?;
    let db = Db::connect(&config.db.path).await?;
    let r2 = R2Client::new(&config.r2).await?;

    let image_grabber = MyImageGrabJob::from_config(&config);
    let storage = R2StorageJob::new(r2);
    run_ingest_once(&db, &image_grabber, &storage).await?;

    match tagger_mode {
        TaggerMode::CloudRun => {
            let cloud_run_config = cloud_run_config_from_env()?;
            let tagger = CloudRunTaggerJob::new(cloud_run_config).await?;
            run_cloud_tagger(&db, &tagger, config.tagger_batch_size).await?;
        }
        TaggerMode::Local => {
            let tagger = LocalTaggerJob::new(config.r2.clone()).await?;
            run_local_tagger(&db, &tagger, config.tagger_batch_size).await?;
        }
    }

    Ok(())
}
