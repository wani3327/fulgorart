use anyhow::Result;

use fulgorart_db::Db;

pub struct TaggerTool {}

impl TaggerTool {
    pub async fn run(db_connection: Db) -> Result<()> {
        use google_cloud_lro::Poller;

        let pendings = db_connection
            .get_pending_tag_jobs_with_keys(i64::MAX)
            .await?;

        if pendings.is_empty() {
            // return Ok(())
        }

        // remote call GCP
        // Initialize the Cloud Run Admin API client
        // It automatically discovers credentials from your environment
        let client = google_cloud_run_v2::client::Jobs::builder().build().await?;

        // let co = google_cloud_run_v2::model::run_job_request::overrides::ContainerOverride::new()
        //     .set_args(pendings.iter().map(|job| format!("r2://{}", job.s3_key)));
        
        let co = google_cloud_run_v2::model::run_job_request::overrides::ContainerOverride::new()
            .set_args(["r2://119053188_p0.jpg"]);

        let or = google_cloud_run_v2::model::run_job_request::Overrides::new()
            .set_container_overrides([co])
            .set_task_count(1);
        
        let response = client
            .run_job()
            .set_name("projects/development-493004/locations/us-west1/jobs/fulgorart-tagger")
            .set_overrides(or)
            .poller()
            .until_done()
            .await?;

        println!("done; {response:?}");

        todo!()
    }
}
