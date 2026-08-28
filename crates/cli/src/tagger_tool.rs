use std::collections::HashMap;

use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use fulgorart_db::{Db, TagJobWithKey};
use fulgorart_storage::R2Client;
use fulgorart_tagger::Wd14Tagger;

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    /// Number of pending jobs to process per fetch
    #[arg(long, default_value_t = 20)]
    pub batch_size: i64,
}

#[derive(Debug, Clone)]
struct Wd14Label {
    name: String,
    category: String,
}

#[derive(Debug, Clone)]
struct Wd14LabelIndex {
    by_tag_id: HashMap<i64, Wd14Label>,
}

#[derive(Debug, serde::Deserialize)]
struct Wd14CsvRow {
    tag_id: i64,
    name: String,
    category: i32,
}

impl Wd14LabelIndex {
    fn from_env() -> Result<Self> {
        let labels_path = std::env::var("WD14_LABELS_PATH")
            .unwrap_or_else(|_| "./models/selected_tags.csv".to_string());
        let mut reader = csv::Reader::from_path(&labels_path)
            .with_context(|| format!("Cannot open WD14 labels CSV '{labels_path}'"))?;
        let mut by_tag_id = HashMap::new();

        for row in reader.deserialize::<Wd14CsvRow>() {
            let row = row.context("Failed to parse WD14 labels row")?;
            by_tag_id.insert(
                row.tag_id,
                Wd14Label {
                    name: row.name,
                    category: wd14_category_name(row.category).to_string(),
                },
            );
        }

        Ok(Self { by_tag_id })
    }

    fn get(&self, tag_id: i64) -> Option<&Wd14Label> {
        self.by_tag_id.get(&tag_id)
    }
}

fn wd14_category_name(category_id: i32) -> &'static str {
    match category_id {
        0 => "general",
        1 => "artist",
        4 => "character",
        5 => "meta",
        9 => "rating",
        _ => "general",
    }
}

async fn apply_job_tags(
    db: &Db,
    labels: &Wd14LabelIndex,
    tagger: &Wd14Tagger,
    job: &TagJobWithKey,
    bytes: &[u8],
) -> Result<()> {
    let predictions = tagger.tag(bytes)?;

    for prediction in predictions {
        let label = labels
            .get(prediction.tag_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown WD14 tag id {}", prediction.tag_id))?;
        let tag_row = db
            .get_or_create_tag(&label.name, Some(&label.category))
            .await?;
        db.insert_image_tag(
            job.image_id,
            tag_row.id,
            "wd14",
            Some(prediction.score as f64),
        )
        .await?;
    }

    Ok(())
}

pub async fn run(args: Args, db: &Db, r2: &R2Client) -> Result<()> {
    let batch_size = args.batch_size.max(1);
    let labels = Wd14LabelIndex::from_env()?;
    let tagger = Wd14Tagger::from_env()?;

    loop {
        let jobs = db.get_pending_tag_jobs_with_keys(batch_size).await?;
        if jobs.is_empty() {
            println!("no_uploaded_tag_jobs");
            break;
        }

        for job in &jobs {
            db.update_tag_job_status(job.job_id, "running", None)
                .await?;
        }

        for job in &jobs {
            let result = async {
                let key = job.s3_key.strip_prefix("r2://").unwrap_or(&job.s3_key);
                let bytes = r2
                    .download(key)
                    .await
                    .with_context(|| format!("Failed to download '{}'", key))?;
                apply_job_tags(db, &labels, &tagger, job, &bytes).await
            }
            .await;

            match result {
                Ok(()) => {
                    db.update_tag_job_status(job.job_id, "tagged", None).await?;
                    println!("tagged image_id={} key={}", job.image_id, job.s3_key);
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

        if (jobs.len() as i64) < batch_size {
            break;
        }
    }

    Ok(())
}
