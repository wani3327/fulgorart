use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ─── Config ───────────────────────────────────────────────────────────────────

/// Configuration loaded from environment variables.
#[derive(Debug, Clone)]
pub struct BridgeConfig {
    /// GCP project ID (e.g. `my-project`).
    pub gcp_project_id: String,
    /// GCP region where the Cloud Run Job is deployed (e.g. `asia-northeast1`).
    pub gcp_region: String,
    /// Cloud Run Job name (e.g. `fulgorart-tagger`).
    pub cloud_run_job_name: String,
    /// Maximum number of URLs dispatched per job execution.
    pub tagger_batch_size: usize,
    /// Path to the SQLite database.
    pub db_path: String,
}

impl BridgeConfig {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();
        Ok(BridgeConfig {
            gcp_project_id: std::env::var("GCP_PROJECT_ID")
                .context("GCP_PROJECT_ID is required")?,
            gcp_region: std::env::var("GCP_REGION").context("GCP_REGION is required")?,
            cloud_run_job_name: std::env::var("CLOUD_RUN_JOB_NAME")
                .context("CLOUD_RUN_JOB_NAME is required")?,
            tagger_batch_size: std::env::var("TAGGER_BATCH_SIZE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(20),
            db_path: std::env::var("FULGORART_DB_PATH")
                .unwrap_or_else(|_| "./data/fulgorart.db".to_string()),
        })
    }
}

// ─── GCP API types ────────────────────────────────────────────────────────────

/// Subset of the Cloud Run Jobs v2 `RunJobRequest` body.
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

/// Subset of the long-running operation returned by `jobs:run`.
#[derive(Debug, Deserialize)]
struct Operation {
    /// Resource name, e.g. `projects/.../locations/.../operations/...`
    pub name: String,
    pub done: Option<bool>,
    pub error: Option<OperationError>,
    /// When done and successful, contains a serialized `Execution` resource.
    pub response: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct OperationError {
    pub code: i32,
    pub message: String,
}

/// Minimal view of a Cloud Run `Execution` resource.
#[derive(Debug, Deserialize)]
struct Execution {
    /// Execution resource name.
    #[allow(dead_code)]
    pub name: String,
    /// Terminal conditions / completion status.
    pub conditions: Option<Vec<ExecutionCondition>>,
}

#[derive(Debug, Deserialize)]
struct ExecutionCondition {
    #[serde(rename = "type")]
    pub condition_type: String,
    pub state: String,
    pub message: Option<String>,
}

/// One log entry from Cloud Logging.
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

/// The JSON line emitted by `fulgorart-tagger` for each processed URL.
#[derive(Debug, Deserialize)]
pub struct UrlTagResult {
    pub url: String,
    pub tags: Vec<TagPrediction>,
}

#[derive(Debug, Deserialize)]
pub struct TagPrediction {
    pub name: String,
    pub category: Option<String>,
    pub score: f32,
}

// ─── BridgeService ────────────────────────────────────────────────────────────

pub struct BridgeService {
    pub db: fulgorart_db::Db,
    pub config: BridgeConfig,
    pub http: reqwest::Client,
    auth: Arc<dyn gcp_auth::TokenProvider>,
}

impl BridgeService {
    pub async fn new(config: BridgeConfig) -> Result<Self> {
        let db = fulgorart_db::Db::connect(&config.db_path).await?;
        let http = reqwest::Client::new();
        let auth = gcp_auth::provider()
            .await
            .context("Failed to initialise GCP authentication")?;
        Ok(BridgeService {
            db,
            config,
            http,
            auth,
        })
    }

    /// Obtain a Bearer token for the given scopes.
    async fn bearer_token(&self) -> Result<String> {
        let scopes = &["https://www.googleapis.com/auth/cloud-platform"];
        let token = self
            .auth
            .token(scopes)
            .await
            .context("Failed to get GCP access token")?;
        Ok(format!("Bearer {}", token.as_str()))
    }

    /// Main entry point: process all pending tag jobs in batches.
    pub async fn run_once(&self) -> Result<()> {
        let batch_size = self.config.tagger_batch_size as i64;

        loop {
            let jobs = self
                .db
                .get_pending_tag_jobs_with_urls(batch_size)
                .await?;

            if jobs.is_empty() {
                tracing::info!("No pending tag jobs");
                break;
            }

            let job_ids: Vec<i64> = jobs.iter().map(|j| j.job_id).collect();
            let urls: Vec<String> = jobs.iter().map(|j| j.r2_url.clone()).collect();

            tracing::info!(count = jobs.len(), "Dispatching batch to Cloud Run Job");

            // Mark all jobs in this batch as running.
            for job in &jobs {
                self.db
                    .update_tag_job_status(job.job_id, "running", None)
                    .await?;
            }

            match self.run_batch(&urls).await {
                Ok(results) => {
                    self.apply_results(&jobs, &results).await?;
                }
                Err(e) => {
                    let msg = format!("{e:#}");
                    tracing::error!(error = %msg, "Batch execution failed");
                    for job_id in &job_ids {
                        self.db
                            .update_tag_job_status(*job_id, "failed", Some(&msg))
                            .await?;
                    }
                }
            }

            // If fewer jobs were returned than the batch size we have drained the queue.
            if (jobs.len() as i64) < batch_size {
                break;
            }
        }

        Ok(())
    }

    /// Submit one Cloud Run Job execution for the given URLs, poll until
    /// terminal, then fetch and return its log output.
    async fn run_batch(&self, urls: &[String]) -> Result<Vec<UrlTagResult>> {
        let execution_name = self.submit_execution(urls).await?;
        tracing::info!(execution = %execution_name, "Execution submitted, polling…");

        self.wait_for_execution(&execution_name).await?;
        tracing::info!(execution = %execution_name, "Execution finished, fetching logs");

        self.fetch_logs(&execution_name).await
    }

    /// Call `jobs:run` with an args override and return the execution resource name.
    async fn submit_execution(&self, urls: &[String]) -> Result<String> {
        let bearer = self.bearer_token().await?;
        let project = &self.config.gcp_project_id;
        let region = &self.config.gcp_region;
        let job = &self.config.cloud_run_job_name;

        let url = format!(
            "https://run.googleapis.com/v2/projects/{project}/locations/{region}/jobs/{job}:run"
        );

        let body = RunJobRequest {
            overrides: JobOverrides {
                container_overrides: vec![ContainerOverride {
                    args: urls.to_vec(),
                }],
            },
        };

        let resp = self
            .http
            .post(&url)
            .header("Authorization", bearer)
            .json(&body)
            .send()
            .await
            .context("Failed to call Cloud Run Jobs API")?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            anyhow::bail!("Cloud Run Jobs API returned {status}: {text}");
        }

        let op: Operation =
            serde_json::from_str(&text).context("Failed to parse jobs:run response")?;

        // The operation name is: projects/.../locations/.../operations/<id>
        // The eventual execution name will be:
        //   projects/.../locations/.../jobs/<job>/executions/<execution>
        // We derive it from the operation once it completes.
        self.poll_operation_for_execution_name(&op.name).await
    }

    /// Poll the long-running operation until done, returning the execution name.
    async fn poll_operation_for_execution_name(&self, op_name: &str) -> Result<String> {
        let bearer_base = "https://run.googleapis.com/v2/";
        // op_name is already an absolute resource path like
        // "projects/.../locations/.../operations/..."
        let url = format!("{bearer_base}{op_name}");

        loop {
            let bearer = self.bearer_token().await?;
            let resp = self
                .http
                .get(&url)
                .header("Authorization", bearer)
                .send()
                .await
                .context("Failed to poll operation")?;

            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();

            if !status.is_success() {
                anyhow::bail!("Operation poll returned {status}: {text}");
            }

            let op: Operation =
                serde_json::from_str(&text).context("Failed to parse operation response")?;

            if op.done.unwrap_or(false) {
                if let Some(err) = op.error {
                    anyhow::bail!("Operation failed ({}): {}", err.code, err.message);
                }
                // Extract execution name from the response metadata.
                if let Some(response) = op.response {
                    if let Some(name) = response.get("name").and_then(|v| v.as_str()) {
                        return Ok(name.to_string());
                    }
                }
                anyhow::bail!("Operation succeeded but response contained no execution name");
            }

            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }

    /// Poll the execution resource until it reaches a terminal state.
    async fn wait_for_execution(&self, execution_name: &str) -> Result<()> {
        let url = format!("https://run.googleapis.com/v2/{execution_name}");

        loop {
            let bearer = self.bearer_token().await?;
            let resp = self
                .http
                .get(&url)
                .header("Authorization", bearer)
                .send()
                .await
                .context("Failed to poll execution")?;

            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();

            if !status.is_success() {
                anyhow::bail!("Execution poll returned {status}: {text}");
            }

            let exec: Execution =
                serde_json::from_str(&text).context("Failed to parse execution response")?;

            if let Some(conditions) = &exec.conditions {
                for cond in conditions {
                    if cond.condition_type == "Completed" {
                        match cond.state.as_str() {
                            "CONDITION_SUCCEEDED" => return Ok(()),
                            "CONDITION_FAILED" => {
                                let msg =
                                    cond.message.as_deref().unwrap_or("unknown reason");
                                anyhow::bail!("Execution failed: {msg}");
                            }
                            _ => {}
                        }
                    }
                }
            }

            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }

    /// Fetch all log entries for the given execution from Cloud Logging.
    async fn fetch_logs(&self, execution_name: &str) -> Result<Vec<UrlTagResult>> {
        // Give Cloud Logging a moment to index the final log lines.
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let project = &self.config.gcp_project_id;
        let url = "https://logging.googleapis.com/v2/entries:list".to_string();

        // Extract just the last path segment as the execution id for the label filter.
        let exec_id = execution_name
            .split('/')
            .last()
            .unwrap_or(execution_name);

        let filter = format!(
            "resource.type=\"cloud_run_job\" labels.\"run.googleapis.com/execution_name\"=\"{exec_id}\""
        );

        let mut results: Vec<UrlTagResult> = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let bearer = self.bearer_token().await?;

            let mut body = serde_json::json!({
                "resourceNames": [format!("projects/{project}")],
                "filter": filter,
                "orderBy": "timestamp asc",
                "pageSize": 1000,
            });

            if let Some(token) = &page_token {
                body["pageToken"] = serde_json::Value::String(token.clone());
            }

            let resp = self
                .http
                .post(&url)
                .header("Authorization", bearer)
                .json(&body)
                .send()
                .await
                .context("Failed to call Cloud Logging API")?;

            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();

            if !status.is_success() {
                anyhow::bail!("Cloud Logging API returned {status}: {text}");
            }

            let log_resp: ListLogEntriesResponse =
                serde_json::from_str(&text).context("Failed to parse log entries response")?;

            if let Some(entries) = log_resp.entries {
                for entry in entries {
                    if let Some(payload) = entry.text_payload {
                        match serde_json::from_str::<UrlTagResult>(&payload) {
                            Ok(result) => results.push(result),
                            Err(e) => {
                                tracing::warn!(payload = %payload, error = %e, "Skipping non-JSON log line");
                            }
                        }
                    }
                }
            }

            match log_resp.next_page_token {
                Some(token) if !token.is_empty() => page_token = Some(token),
                _ => break,
            }
        }

        tracing::info!(count = results.len(), "Parsed tag results from logs");
        Ok(results)
    }

    /// Write parsed tag results to the DB and update job statuses.
    async fn apply_results(
        &self,
        jobs: &[fulgorart_db::TagJobWithUrl],
        results: &[UrlTagResult],
    ) -> Result<()> {
        // Build a URL → result index for quick lookup.
        let result_map: std::collections::HashMap<&str, &UrlTagResult> =
            results.iter().map(|r| (r.url.as_str(), r)).collect();

        for job in jobs {
            match result_map.get(job.r2_url.as_str()) {
                Some(result) => {
                    for pred in &result.tags {
                        let tag = self
                            .db
                            .get_or_create_tag(&pred.name, pred.category.as_deref())
                            .await?;
                        self.db
                            .insert_image_tag(job.image_id, tag.id, "wd14", Some(pred.score as f64))
                            .await?;
                    }
                    tracing::info!(
                        image_id = job.image_id,
                        tags = result.tags.len(),
                        "Tags applied"
                    );
                    self.db
                        .update_tag_job_status(job.job_id, "done", None)
                        .await?;
                }
                None => {
                    let msg = format!("No log output found for URL: {}", job.r2_url);
                    tracing::warn!(image_id = job.image_id, %msg, "Missing result");
                    self.db
                        .update_tag_job_status(job.job_id, "failed", Some(&msg))
                        .await?;
                }
            }
        }

        Ok(())
    }
}
