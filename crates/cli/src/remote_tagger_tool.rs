use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use serde::Deserialize;

use fulgorart_db::{Db, TagJobWithKey};
use fulgorart_storage as storage;
use fulgorart_tagger::TagPrediction;

use crate::tagger_tool::{save_predictions, Wd14LabelIndex};

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    /// Number of pending jobs sent to one Cloud Run execution
    #[arg(long, default_value_t = 50)]
    pub batch_size: i64,

    /// How long to keep polling Cloud Logging for an execution's results
    #[arg(long, default_value_t = 180)]
    pub log_wait_secs: u64,

    /// Stop after this many executions (default: until no pending jobs remain)
    #[arg(long)]
    pub max_batches: Option<u64>,
}

/// One line logged by fulgorart-tagger-s3, either a result or a per-image error.
#[derive(Debug, Deserialize)]
struct LoggedTagResult {
    key: String,
    #[serde(default)]
    tags: Option<Vec<TagPrediction>>,
    #[serde(default)]
    error: Option<String>,
}

pub struct CloudRunJob {
    pub project_id: String,
    pub location: String,
    pub job_name: String,
}

impl CloudRunJob {
    pub fn from_env() -> Result<Self> {
        let var = |name: &str| {
            std::env::var(name)
                .ok()
                .filter(|v| !v.is_empty())
                .with_context(|| format!("{name} must be set"))
        };
        Ok(Self {
            project_id: var("FULGORART_GCP_PROJECT_ID")?,
            location: var("FULGORART_GCP_REGION")?,
            job_name: var("FULGORART_CLOUD_RUN_JOB_NAME")?,
        })
    }

    async fn retrieve_log(
        &self,
        client: &google_cloud_logging_v2::client::LoggingServiceV2,
        execution_name: &str,
    ) -> Result<Vec<google_cloud_logging_v2::model::LogEntry>> {
        // Only this execution's NOTICE lines, which is where tagger-s3 writes results.
        let filter_query = format!(
            "resource.type=\"cloud_run_job\" \
             AND resource.labels.project_id=\"{}\" \
             AND resource.labels.location=\"{}\" \
             AND resource.labels.job_name=\"{}\" \
             AND labels.\"run.googleapis.com/execution_name\"=\"{}\" \
             AND severity = NOTICE",
            self.project_id, self.location, self.job_name, execution_name
        );

        let mut entries = Vec::new();
        let mut page_token = String::new();

        loop {
            let response = client
                .list_log_entries()
                .set_resource_names(vec![format!("projects/{}", self.project_id)])
                .set_filter(filter_query.clone())
                .set_order_by("timestamp asc")
                .set_page_token(page_token)
                .send()
                .await?;

            entries.extend(response.entries.iter().cloned());

            if response.next_page_token.is_empty() {
                return Ok(entries);
            }

            page_token = response.next_page_token;
        }
    }

    /// Run the job for these images and wait for it to finish; returns the execution id.
    async fn trigger(&self, jobs: &[TagJobWithKey]) -> Result<String> {
        use google_cloud_lro::Poller;
        use google_cloud_run_v2::{
            client::Jobs,
            model::run_job_request::{overrides::ContainerOverride, Overrides},
        };

        let client = Jobs::builder().build().await?;

        let name = format!(
            "projects/{}/locations/{}/jobs/{}",
            self.project_id, self.location, self.job_name
        );

        let co = ContainerOverride::new().set_args(
            jobs.iter()
                .map(|job| format!("r2://{}", storage::original_key(&job.s3_key_base))),
        );
        let overrides = Overrides::new()
            .set_container_overrides([co])
            .set_task_count(1);

        let execution = client
            .run_job()
            .set_name(name)
            .set_overrides(overrides)
            .poller()
            .until_done()
            .await
            .context("Cloud Run job execution failed")?;

        execution
            .name
            .rsplit('/')
            .next()
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .context("Cloud Run returned an execution without a name")
    }
}

/// Find the job a logged key belongs to. tagger-s3 logs the R2 key it read
/// (`<key_base>/original`); also accept an `r2://` prefix or a bare key base.
fn job_for_key<'a>(jobs: &'a [TagJobWithKey], logged_key: &str) -> Option<&'a TagJobWithKey> {
    let key = logged_key.strip_prefix("r2://").unwrap_or(logged_key);
    jobs.iter()
        .find(|job| key == storage::original_key(&job.s3_key_base) || key == job.s3_key_base)
}

/// Parse one log entry's message into a tag result.
fn parse_message(message: &str) -> Result<LoggedTagResult> {
    serde_json::from_str(message).with_context(|| format!("not a tag result: {message}"))
}

/// Collect this execution's results, polling until every job has one or the wait runs out
/// (Cloud Logging can lag behind the execution by a while).
async fn collect_results(
    cloud_run: &CloudRunJob,
    execution: &str,
    jobs: &[TagJobWithKey],
    wait: Duration,
) -> Result<HashMap<i64, LoggedTagResult>> {
    let client = google_cloud_logging_v2::client::LoggingServiceV2::builder()
        .build()
        .await?;
    let deadline = Instant::now() + wait;
    let mut results = HashMap::new();

    loop {
        for entry in cloud_run.retrieve_log(&client, execution).await? {
            let Some(message) = entry
                .json_payload()
                .and_then(|p| p.get("message"))
                .and_then(|m| m.as_str())
            else {
                continue;
            };
            let result = match parse_message(message) {
                Ok(result) => result,
                Err(error) => {
                    tracing::warn!(%execution, "skipping log entry: {error:#}");
                    continue;
                }
            };
            match job_for_key(jobs, &result.key) {
                Some(job) => {
                    results.entry(job.job_id).or_insert(result);
                }
                None => tracing::warn!(%execution, key = %result.key, "result for unknown key"),
            }
        }

        if results.len() >= jobs.len() || Instant::now() >= deadline {
            return Ok(results);
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

async fn run_batch(
    db: &Db,
    labels: &Wd14LabelIndex,
    cloud_run: &CloudRunJob,
    jobs: &[TagJobWithKey],
    log_wait: Duration,
) -> Result<()> {
    for job in jobs {
        db.update_tag_job_status(job.job_id, "running", None).await?;
    }

    let execution = match cloud_run.trigger(jobs).await {
        Ok(execution) => execution,
        Err(error) => {
            // Don't leave the batch stuck in "running".
            let message = format!("{error:#}");
            for job in jobs {
                db.update_tag_job_status(job.job_id, "failed", Some(&message))
                    .await?;
            }
            return Err(error);
        }
    };
    println!("execution={execution} jobs={}", jobs.len());

    let mut results = collect_results(cloud_run, &execution, jobs, log_wait).await?;

    for job in jobs {
        let outcome = match results.remove(&job.job_id) {
            Some(LoggedTagResult { tags: Some(tags), .. }) => {
                save_predictions(db, labels, job.image_id, tags).await
            }
            Some(LoggedTagResult { error: Some(error), .. }) => {
                Err(anyhow::anyhow!("tagger error: {error}"))
            }
            Some(_) => Err(anyhow::anyhow!("tagger result had neither tags nor error")),
            None => Err(anyhow::anyhow!(
                "no result in Cloud Run logs for execution {execution}"
            )),
        };

        match outcome {
            Ok(()) => {
                db.update_tag_job_status(job.job_id, "tagged", None).await?;
                println!("tagged image_id={} key={}", job.image_id, job.s3_key_base);
            }
            Err(error) => {
                let message = format!("{error:#}");
                db.update_tag_job_status(job.job_id, "failed", Some(&message))
                    .await?;
                eprintln!(
                    "tag_job_failed job_id={} image_id={} error={}",
                    job.job_id, job.image_id, message
                );
            }
        }
    }

    Ok(())
}

pub async fn run(args: Args, db: &Db, cloud_run: &CloudRunJob) -> Result<()> {
    let batch_size = args.batch_size.max(1);
    let log_wait = Duration::from_secs(args.log_wait_secs);
    let labels = Wd14LabelIndex::from_env()?;
    let mut batches = 0;

    while args.max_batches.is_none_or(|max| batches < max) {
        let jobs = db.get_pending_tag_jobs_with_keys(batch_size).await?;
        if jobs.is_empty() {
            println!("no_uploaded_tag_jobs");
            break;
        }

        run_batch(db, &labels, cloud_run, &jobs, log_wait).await?;
        batches += 1;

        if (jobs.len() as i64) < batch_size {
            break;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(job_id: i64, s3_key_base: &str) -> TagJobWithKey {
        TagJobWithKey {
            job_id,
            image_id: job_id * 10,
            s3_key_base: s3_key_base.to_owned(),
        }
    }

    #[test]
    fn logged_original_key_matches_job() {
        let jobs = [job(1, "pixiv/123_p0.jpg"), job(2, "pixiv/456_p0.jpg")];
        // what tagger-s3 actually logs: the key it downloaded, without r2://
        assert_eq!(job_for_key(&jobs, "pixiv/456_p0.jpg/original").map(|j| j.job_id), Some(2));
        assert_eq!(job_for_key(&jobs, "r2://pixiv/123_p0.jpg/original").map(|j| j.job_id), Some(1));
        assert_eq!(job_for_key(&jobs, "pixiv/123_p0.jpg").map(|j| j.job_id), Some(1));
        assert!(job_for_key(&jobs, "pixiv/789_p0.jpg/original").is_none());
        assert!(job_for_key(&jobs, "pixiv/123_p0.jpg/thumbnail").is_none());
    }

    #[test]
    fn parses_result_and_error_lines() {
        let ok = parse_message(
            r#"{"key":"a/original","tags":[{"tag_id":5,"category_id":0,"score":0.9}]}"#,
        )
        .unwrap();
        assert_eq!(ok.key, "a/original");
        assert_eq!(ok.tags.unwrap()[0].tag_id, 5);

        let err = parse_message(r#"{"key":"a/original","error":"decode failed"}"#).unwrap();
        assert_eq!(err.error.as_deref(), Some("decode failed"));
        assert!(err.tags.is_none());

        assert!(parse_message("Tagger processed file(s)").is_err());
    }
}
