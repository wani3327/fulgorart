use anyhow::{Context, Result};
use async_trait::async_trait;
use fulgorart_db::{Db, TagJobWithKey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::CloudRunConfig;

#[async_trait]
pub trait JobDelegate: Send + Sync {
    async fn dispatch_pending_jobs(&self) -> Result<()>;
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
struct R2TagResult {
    pub key: String,
    pub tags: Vec<TagPrediction>,
}

#[derive(Debug, Deserialize)]
struct TagPrediction {
    pub name: String,
    pub category: Option<String>,
    pub score: f32,
}

pub struct CloudRunJobDelegate {
    db: Db,
    config: CloudRunConfig,
    http: reqwest::Client,
    auth: Arc<dyn gcp_auth::TokenProvider>,
}

impl CloudRunJobDelegate {
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

    async fn run_batch(&self, keys: &[String]) -> Result<Vec<R2TagResult>> {
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

    async fn fetch_logs(&self, execution_name: &str) -> Result<Vec<R2TagResult>> {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let execution_id = execution_name
            .rsplit('/')
            .next()
            .unwrap_or(execution_name);
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
                        match serde_json::from_str::<R2TagResult>(&payload) {
                            Ok(result) => results.push(result),
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

    async fn apply_results(&self, jobs: &[TagJobWithKey], results: &[R2TagResult]) -> Result<()> {
        let result_map: HashMap<&str, &R2TagResult> = results
            .iter()
            .map(|result| (result.key.as_str(), result))
            .collect();

        for job in jobs {
            match result_map.get(job.r2_key.as_str()) {
                Some(result) => {
                    for prediction in &result.tags {
                        let tag = self
                            .db
                            .get_or_create_tag(&prediction.name, prediction.category.as_deref())
                            .await?;
                        self.db
                            .insert_image_tag(
                                job.image_id,
                                tag.id,
                                "wd14",
                                Some(prediction.score as f64),
                            )
                            .await?;
                    }
                    self.db
                        .update_tag_job_status(job.job_id, "done", None)
                        .await?;
                }
                None => {
                    let message = format!("No log output found for R2 key: {}", job.r2_key);
                    self.db
                        .update_tag_job_status(job.job_id, "failed", Some(&message))
                        .await?;
                }
            }
        }

        Ok(())
    }
}

#[async_trait]
impl JobDelegate for CloudRunJobDelegate {
    async fn dispatch_pending_jobs(&self) -> Result<()> {
        let batch_size = self.config.tagger_batch_size as i64;

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
                Ok(results) => self.apply_results(&jobs, &results).await?,
                Err(error) => {
                    let message = format!("{error:#}");
                    for job in &jobs {
                        self.db
                            .update_tag_job_status(job.job_id, "failed", Some(&message))
                            .await?;
                    }
                }
            }

            if (jobs.len() as i64) < batch_size {
                break;
            }
        }

        Ok(())
    }
}
