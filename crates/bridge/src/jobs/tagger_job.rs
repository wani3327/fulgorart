use anyhow::{Context, Result};
use async_trait::async_trait;
use fulgorart_db::{Db, TagJobWithKey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::CloudRunConfig;

#[async_trait]
pub trait TaggerJob: Send + Sync {
    async fn tag(&self) -> Result<TaggerJobSummary>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TaggerJobSummary {
    pub succeeded: usize,
    pub failed: usize,
}

impl TaggerJobSummary {
    pub fn total(self) -> usize {
        self.succeeded + self.failed
    }

    fn merge(&mut self, other: Self) {
        self.succeeded += other.succeeded;
        self.failed += other.failed;
    }
}

#[derive(Debug, Clone)]
struct BridgeTagResult {
    key: String,
    tags: Vec<BridgeTagPrediction>,
}

#[derive(Debug, Clone)]
struct BridgeTagPrediction {
    name: String,
    category: Option<String>,
    score: f32,
}

async fn apply_results(
    db: &Db,
    jobs: &[TagJobWithKey],
    results: &[BridgeTagResult],
) -> Result<TaggerJobSummary> {
    let result_map: HashMap<&str, &BridgeTagResult> = results
        .iter()
        .map(|result| (result.key.as_str(), result))
        .collect();
    let mut summary = TaggerJobSummary::default();

    for job in jobs {
        match result_map.get(job.r2_key.as_str()) {
            Some(result) => {
                for prediction in &result.tags {
                    let tag = db
                        .get_or_create_tag(&prediction.name, prediction.category.as_deref())
                        .await?;
                    db.insert_image_tag(
                        job.image_id,
                        tag.id,
                        "wd14",
                        Some(prediction.score as f64),
                    )
                    .await?;
                }
                db.update_tag_job_status(job.job_id, "done", None).await?;
                summary.succeeded += 1;
            }
            None => {
                let message = format!("No tag output found for R2 key: {}", job.r2_key);
                db.update_tag_job_status(job.job_id, "failed", Some(&message))
                    .await?;
                summary.failed += 1;
            }
        }
    }

    Ok(summary)
}

async fn mark_batch_failed(db: &Db, jobs: &[TagJobWithKey], message: &str) -> Result<usize> {
    for job in jobs {
        db.update_tag_job_status(job.job_id, "failed", Some(message))
            .await?;
    }
    Ok(jobs.len())
}

pub struct LocalTaggerJob {
    db: Db,
    batch_size: usize,
}

impl LocalTaggerJob {
    pub fn new(db: Db, batch_size: usize) -> Self {
        Self {
            db,
            batch_size: batch_size.max(1),
        }
    }

    async fn run_batch(&self, keys: &[String]) -> Result<Vec<BridgeTagResult>> {
        let outputs = fulgorart_tagger::tag_r2_keys(keys).await?;
        Ok(outputs
            .into_iter()
            .map(|result| BridgeTagResult {
                key: result.key,
                tags: result
                    .tags
                    .into_iter()
                    .map(|prediction| BridgeTagPrediction {
                        name: prediction.name,
                        category: prediction.category,
                        score: prediction.score,
                    })
                    .collect(),
            })
            .collect())
    }
}

#[async_trait]
impl TaggerJob for LocalTaggerJob {
    async fn tag(&self) -> Result<TaggerJobSummary> {
        let batch_size = self.batch_size as i64;
        let mut summary = TaggerJobSummary::default();

        loop {
            let jobs = self.db.get_pending_tag_jobs_with_keys(batch_size).await?;
            if jobs.is_empty() {
                tracing::info!("No pending tag jobs");
                break;
            }

            for job in &jobs {
                self.db
                    .update_tag_job_status(job.job_id, "running", None)
                    .await?;
            }

            let keys: Vec<String> = jobs.iter().map(|job| job.r2_key.clone()).collect();
            match self.run_batch(&keys).await {
                Ok(results) => summary.merge(apply_results(&self.db, &jobs, &results).await?),
                Err(error) => {
                    let message = format!("{error:#}");
                    summary.failed += mark_batch_failed(&self.db, &jobs, &message).await?;
                }
            }

            if (jobs.len() as i64) < batch_size {
                break;
            }
        }

        Ok(summary)
    }
}

#[derive(Debug, Serialize)]
struct RunJobRequest {
    overrides: JobOverrides,
}

#[derive(Debug, Serialize)]
struct JobOverrides {
    #[serde(rename = "containerOverrides")]
    container_overrides: Vec<ContainerOverride>,
}

#[derive(Debug, Serialize)]
struct ContainerOverride {
    args: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Operation {
    pub name: String,
    pub done: Option<bool>,
    pub error: Option<OperationError>,
    pub response: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct OperationError {
    pub code: i32,
    pub message: String,
}

#[derive(Debug, Deserialize)]
struct Execution {
    pub conditions: Option<Vec<ExecutionCondition>>,
}

#[derive(Debug, Deserialize)]
struct ExecutionCondition {
    #[serde(rename = "type")]
    pub condition_type: String,
    pub state: String,
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LogEntry {
    #[serde(rename = "textPayload")]
    pub text_payload: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListLogEntriesResponse {
    pub entries: Option<Vec<LogEntry>>,
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CloudRunTagResult {
    pub key: String,
    pub tags: Vec<CloudRunTagPrediction>,
}

#[derive(Debug, Deserialize)]
struct CloudRunTagPrediction {
    pub name: String,
    pub category: Option<String>,
    pub score: f32,
}

pub struct CloudRunTaggerJob {
    db: Db,
    config: CloudRunConfig,
    http: reqwest::Client,
    auth: Arc<dyn gcp_auth::TokenProvider>,
}

impl CloudRunTaggerJob {
    pub async fn new(db: Db, config: CloudRunConfig) -> Result<Self> {
        let http = reqwest::Client::new();
        let auth = gcp_auth::provider()
            .await
            .context("Failed to initialise GCP authentication")?;
        Ok(Self {
            db,
            config,
            http,
            auth,
        })
    }

    async fn bearer_token(&self) -> Result<String> {
        let token = self
            .auth
            .token(&["https://www.googleapis.com/auth/cloud-platform"])
            .await
            .context("Failed to get GCP access token")?;
        Ok(format!("Bearer {}", token.as_str()))
    }

    async fn run_batch(&self, keys: &[String]) -> Result<Vec<BridgeTagResult>> {
        let execution_name = self.submit_execution(keys).await?;
        self.wait_for_execution(&execution_name).await?;
        self.fetch_logs(&execution_name).await
    }

    async fn submit_execution(&self, keys: &[String]) -> Result<String> {
        let bearer = self.bearer_token().await?;
        let url = format!(
            "https://run.googleapis.com/v2/projects/{}/locations/{}/jobs/{}:run",
            self.config.project_id, self.config.region, self.config.job_name
        );
        let body = RunJobRequest {
            overrides: JobOverrides {
                container_overrides: vec![ContainerOverride {
                    args: keys.iter().map(|key| format!("r2://{key}")).collect(),
                }],
            },
        };

        let response = self
            .http
            .post(&url)
            .header("Authorization", bearer)
            .json(&body)
            .send()
            .await
            .context("Failed to call Cloud Run Jobs API")?;

        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("Cloud Run Jobs API returned {status}: {text}");
        }

        let operation: Operation =
            serde_json::from_str(&text).context("Failed to parse jobs:run response")?;
        self.poll_operation_for_execution_name(&operation.name)
            .await
    }

    async fn poll_operation_for_execution_name(&self, operation_name: &str) -> Result<String> {
        let url = format!("https://run.googleapis.com/v2/{operation_name}");

        loop {
            let bearer = self.bearer_token().await?;
            let response = self
                .http
                .get(&url)
                .header("Authorization", bearer)
                .send()
                .await
                .context("Failed to poll Cloud Run operation")?;
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            if !status.is_success() {
                anyhow::bail!("Operation poll returned {status}: {text}");
            }

            let operation: Operation =
                serde_json::from_str(&text).context("Failed to parse operation response")?;
            if operation.done.unwrap_or(false) {
                if let Some(error) = operation.error {
                    anyhow::bail!("Operation failed ({}): {}", error.code, error.message);
                }
                if let Some(response) = operation.response {
                    if let Some(name) = response.get("name").and_then(|value| value.as_str()) {
                        return Ok(name.to_string());
                    }
                }
                anyhow::bail!("Operation succeeded but response contained no execution name");
            }

            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }

    async fn wait_for_execution(&self, execution_name: &str) -> Result<()> {
        let url = format!("https://run.googleapis.com/v2/{execution_name}");

        loop {
            let bearer = self.bearer_token().await?;
            let response = self
                .http
                .get(&url)
                .header("Authorization", bearer)
                .send()
                .await
                .context("Failed to poll execution")?;
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            if !status.is_success() {
                anyhow::bail!("Execution poll returned {status}: {text}");
            }

            let execution: Execution =
                serde_json::from_str(&text).context("Failed to parse execution response")?;
            if let Some(conditions) = &execution.conditions {
                for condition in conditions {
                    if condition.condition_type == "Completed" {
                        match condition.state.as_str() {
                            "CONDITION_SUCCEEDED" => return Ok(()),
                            "CONDITION_FAILED" => {
                                let message =
                                    condition.message.as_deref().unwrap_or("unknown reason");
                                anyhow::bail!("Execution failed: {message}");
                            }
                            _ => {}
                        }
                    }
                }
            }

            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }

    async fn fetch_logs(&self, execution_name: &str) -> Result<Vec<BridgeTagResult>> {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let execution_id = execution_name.rsplit('/').next().unwrap_or(execution_name);
        let filter = format!(
            "resource.type=\"cloud_run_job\" labels.\"run.googleapis.com/execution_name\"=\"{execution_id}\""
        );
        let mut page_token: Option<String> = None;
        let mut results = Vec::new();

        loop {
            let bearer = self.bearer_token().await?;
            let mut body = serde_json::json!({
                "resourceNames": [format!("projects/{}", self.config.project_id)],
                "filter": filter,
                "orderBy": "timestamp asc",
                "pageSize": 1000,
            });
            if let Some(token) = &page_token {
                body["pageToken"] = serde_json::Value::String(token.clone());
            }

            let response = self
                .http
                .post("https://logging.googleapis.com/v2/entries:list")
                .header("Authorization", bearer)
                .json(&body)
                .send()
                .await
                .context("Failed to call Cloud Logging API")?;
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            if !status.is_success() {
                anyhow::bail!("Cloud Logging API returned {status}: {text}");
            }

            let log_response: ListLogEntriesResponse =
                serde_json::from_str(&text).context("Failed to parse log entries response")?;
            if let Some(entries) = log_response.entries {
                for entry in entries {
                    if let Some(payload) = entry.text_payload {
                        match serde_json::from_str::<CloudRunTagResult>(&payload) {
                            Ok(result) => results.push(BridgeTagResult {
                                key: result.key,
                                tags: result
                                    .tags
                                    .into_iter()
                                    .map(|tag| BridgeTagPrediction {
                                        name: tag.name,
                                        category: tag.category,
                                        score: tag.score,
                                    })
                                    .collect(),
                            }),
                            Err(error) => {
                                tracing::warn!(payload = %payload, error = %error, "Skipping non-result log line")
                            }
                        }
                    }
                }
            }

            match log_response.next_page_token {
                Some(token) if !token.is_empty() => page_token = Some(token),
                _ => break,
            }
        }

        Ok(results)
    }
}

#[async_trait]
impl TaggerJob for CloudRunTaggerJob {
    async fn tag(&self) -> Result<TaggerJobSummary> {
        let batch_size = self.config.tagger_batch_size as i64;
        let mut summary = TaggerJobSummary::default();

        loop {
            let jobs = self.db.get_pending_tag_jobs_with_keys(batch_size).await?;
            if jobs.is_empty() {
                tracing::info!("No pending tag jobs");
                break;
            }

            for job in &jobs {
                self.db
                    .update_tag_job_status(job.job_id, "running", None)
                    .await?;
            }

            let keys: Vec<String> = jobs.iter().map(|job| job.r2_key.clone()).collect();
            match self.run_batch(&keys).await {
                Ok(results) => summary.merge(apply_results(&self.db, &jobs, &results).await?),
                Err(error) => {
                    let message = format!("{error:#}");
                    summary.failed += mark_batch_failed(&self.db, &jobs, &message).await?;
                }
            }

            if (jobs.len() as i64) < batch_size {
                break;
            }
        }

        Ok(summary)
    }
}
