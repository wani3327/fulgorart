use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagPrediction {
    pub name: String,
    pub category: Option<String>,
    pub score: f32,
}

#[async_trait]
pub trait Tagger: Send + Sync {
    async fn tag_image(&self, image_bytes: &[u8]) -> Result<Vec<TagPrediction>>;
}

/// Stub tagger using ONNX WD14 model.
/// TODO: Implement using the `ort` crate (ONNX Runtime) once system dependencies are available.
pub struct OnnxTagger {
    pub model_path: String,
    pub general_threshold: f32,
    pub character_threshold: f32,
}

impl OnnxTagger {
    pub fn new(model_path: &str, general_threshold: f32, character_threshold: f32) -> Result<Self> {
        tracing::warn!(
            "OnnxTagger is stubbed; ONNX runtime not yet integrated. model_path={}",
            model_path
        );
        Ok(OnnxTagger {
            model_path: model_path.to_string(),
            general_threshold,
            character_threshold,
        })
    }
}

#[async_trait]
impl Tagger for OnnxTagger {
    async fn tag_image(&self, _image_bytes: &[u8]) -> Result<Vec<TagPrediction>> {
        // TODO: Implement WD14 ONNX inference using the `ort` crate.
        // Steps:
        //   1. Load ONNX model from self.model_path
        //   2. Pre-process image to 448x448 RGB tensor
        //   3. Run inference
        //   4. Map output logits to tag names using labels CSV
        //   5. Filter by self.general_threshold / self.character_threshold
        tracing::warn!("OnnxTagger::tag_image is a stub; returning empty tags");
        Ok(vec![])
    }
}

/// Background worker that processes pending tag jobs from the database.
pub struct TaggerWorker {
    pub db: fulgorart_db::Db,
    pub tagger: Box<dyn Tagger>,
}

impl TaggerWorker {
    pub fn new(db: fulgorart_db::Db, tagger: Box<dyn Tagger>) -> Self {
        TaggerWorker { db, tagger }
    }

    pub async fn run_once(&self) -> Result<usize> {
        let jobs = self.db.get_pending_tag_jobs(10).await?;
        let count = jobs.len();
        for job in jobs {
            self.db
                .update_tag_job_status(job.id, "running", None)
                .await?;
            let asset = self.db.get_image_asset_by_id(job.image_id).await?;
            match asset {
                None => {
                    self.db
                        .update_tag_job_status(job.id, "failed", Some("image not found"))
                        .await?;
                }
                Some(_asset) => {
                    // TODO: Download image bytes from R2 or local cache, then tag
                    let predictions: Vec<TagPrediction> = vec![];
                    for pred in predictions {
                        let tag = self
                            .db
                            .get_or_create_tag(&pred.name, pred.category.as_deref())
                            .await?;
                        self.db
                            .insert_image_tag(
                                job.image_id,
                                tag.id,
                                "wd14",
                                Some(pred.score as f64),
                            )
                            .await?;
                    }
                    self.db.update_tag_job_status(job.id, "done", None).await?;
                }
            }
        }
        Ok(count)
    }
}
