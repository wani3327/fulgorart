use anyhow::{anyhow, Result};
use serde::Deserialize;

use fulgorart_db::{Db, TagJobWithKey};

#[derive(Deserialize)]
struct TagResult {
    key: String,
    tags: Vec<fulgorart_tagger::TagPrediction>,
}

pub struct RunTaggerJobTool {
    pub project_id: String,
    pub location: String,
    pub job_name: String,
}

impl RunTaggerJobTool {
    async fn retrieve_log<T: AsRef<str>>(
        &self,
        task_id: T,
    ) -> Result<Vec<google_cloud_logging_v2::model::LogEntry>> {
        let client = google_cloud_logging_v2::client::LoggingServiceV2::builder()
            .build()
            .await?;

        // 2. Build the Advanced Log Filter query
        // This tells Cloud Logging exactly which Job Execution logs to fetch
        let filter_query = format!(
            "resource.type=\"cloud_run_job\" \
            AND resource.labels.project_id=\"{}\" \
            AND resource.labels.location=\"{}\" \
            AND resource.labels.job_name=\"{}\" \
            AND labels.\"run.googleapis.com/execution_name\"=\"{}\"
            AND severity = NOTICE",
            self.project_id,
            self.location,
            self.job_name,
            task_id.as_ref()
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
            println!("entries: {} -> {}", response.entries.len(), entries.len());

            if response.next_page_token.is_empty() {
                return Ok(entries);
            }

            page_token = response.next_page_token;
        }
    }

    async fn trigger_job(
        &self,
        pending_jobs: &Vec<fulgorart_db::TagJobWithKey>,
    ) -> Result<google_cloud_run_v2::model::Execution> {
        use google_cloud_lro::Poller;
        use google_cloud_run_v2::{client::Jobs, model::run_job_request::Overrides};

        // Initialize the Cloud Run Admin API client
        let client = Jobs::builder().build().await?;

        let name = format!(
            "projects/{}/locations/{}/jobs/{}",
            self.project_id, self.location, self.job_name
        );

        let co = google_cloud_run_v2::model::run_job_request::overrides::ContainerOverride::new()
            .set_args(
                pending_jobs
                    .iter()
                    .map(|job| format!("r2://{}", job.s3_key)),
            );

        // let co = overrides::ContainerOverride::new().set_args(Vec::<String>::new());

        let or = Overrides::new()
            .set_container_overrides([co])
            .set_task_count(1);

        Ok(client
            .run_job()
            .set_name(name)
            .set_overrides(or)
            .poller()
            .until_done()
            .await?)
    }

    pub async fn run(&self, db_connection: Db) -> Result<()> {
        let pending_jobs = db_connection
            .get_pending_tag_jobs_with_keys(i64::MAX)
            .await?;

        if pending_jobs.is_empty() {
            // return Ok(())
        }

        let pending_jobs = vec![TagJobWithKey {
            job_id: 0,
            image_id: 0,
            s3_key: "119053188_p0.jpg".to_string(),
        }];

        let execution = self.trigger_job(&pending_jobs).await?;
        let task_id = execution
            .name
            .split("/")
            .last()
            .ok_or(anyhow::anyhow!("Invalid task name"))?;
        println!("task id: {task_id}");

        let entries = self.retrieve_log(task_id).await?;
        // let entries = self.retrieve_log("fulgorart-tagger-78tkn").await?;

        let find_ids = |key: &str| {
            for job in &pending_jobs {
                if key == job.s3_key {
                    return Some((job.job_id, job.image_id));
                }
            }
            None
        };

        for entry in entries {
            let (job_id, image_id, tags) = || -> _ {
                let message = entry.json_payload()?.get("message")?.as_str()?;
                let res: TagResult = serde_json::from_str(message).ok()?;
                let (job_id, image_id) = find_ids(&res.key)?;
                Some((job_id, image_id, res.tags))
            }()
            .ok_or(anyhow!("json parse failed {entry:?}"))?;

            for prediction in tags {
                db_connection
                    .insert_image_tag(
                        image_id,
                        prediction.tag_id,
                        "wd-swinv2",
                        Some(prediction.score as f64),
                    )
                    .await?; // TODO: model info should be fetched
            }

            db_connection
                .update_tag_job_status(job_id, "tagged", None)
                .await? // TODO: update tag parse failure error msg to DB
        }

        Ok(())
    }
}
