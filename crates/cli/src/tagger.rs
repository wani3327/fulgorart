use anyhow::Result;

use fulgorart_db::Db;

pub struct RunTaggerJobTool {
    pub project_id: String,
    pub location: String,
    pub job_name: String,
}

impl RunTaggerJobTool {
    async fn retrieve_log<T: AsRef<str>>(
        &self,
        execution_id: T,
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
            execution_id.as_ref()
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

    async fn trigger_job(&self) -> Result<google_cloud_run_v2::model::Execution> {
        use google_cloud_lro::Poller;
        use google_cloud_run_v2::{
            client::Jobs,
            model::run_job_request::{overrides, Overrides},
        };

        // Initialize the Cloud Run Admin API client
        let client = Jobs::builder().build().await?;

        let name = format!(
            "projects/{}/locations/{}/jobs/{}",
            self.project_id, self.location, self.job_name
        );

        // let co = google_cloud_run_v2::model::run_job_request::overrides::ContainerOverride::new()
        //     .set_args(pendings.iter().map(|job| format!("r2://{}", job.s3_key)));

        let co = overrides::ContainerOverride::new().set_args(["r2://119053188_p0.jpg"]);

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
        let pendings = db_connection
            .get_pending_tag_jobs_with_keys(i64::MAX)
            .await?;

        if pendings.is_empty() {
            // return Ok(())
        }

        // let execution = self.trigger_job().await?;

        // println!("trigger done; {execution:?}");

        // self.retrieve_log(execution.uid).await?;
        let entries = self.retrieve_log("fulgorart-tagger-bgtlc").await?;

        for entry in entries {
            if let Some(s) = entry.text_payload() {
                println!("T {s}")
            }
            if let Some(j) = entry.json_payload() {
                println!("J {j:?}")
            }
        }

        todo!()
    }
}
