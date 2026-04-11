use anyhow::{Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use ndarray::Array4;
use ort::{
    session::{builder::GraphOptimizationLevel, Session},
    value::Tensor,
};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Category IDs used in the WD14 `selected_tags.csv`.
const CAT_CHARACTER: i32 = 4;
const WD14_INPUT_SIZE: u32 = 448;

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

// ─── Label entry loaded from selected_tags.csv ────────────────────────────────

#[derive(Debug, Clone)]
struct LabelEntry {
    name: String,
    /// Raw category integer from the CSV (0=general, 4=character, 9=copyright,
    /// 1=artist, 5=meta).  The first four rows are ratings and are skipped.
    category: i32,
}

impl LabelEntry {
    fn category_str(&self) -> &'static str {
        match self.category {
            0 => "general",
            1 => "artist",
            4 => "character",
            5 => "meta",
            9 => "copyright",
            _ => "general",
        }
    }
}

// ─── OnnxTagger ───────────────────────────────────────────────────────────────

/// WD14 tagger backed by an ONNX model via `ort`.
///
/// At runtime, `libonnxruntime.so` (or `onnxruntime.dll` on Windows) must be
/// on `LD_LIBRARY_PATH` / `PATH`.  Download the appropriate ONNX Runtime
/// release from <https://github.com/microsoft/onnxruntime/releases> and place
/// it next to the binary or in a system library path.
pub struct OnnxTagger {
    session: Mutex<Session>,
    /// All tag labels including the leading rating rows.
    labels: Vec<LabelEntry>,
    /// Minimum score for general / meta tags to be included.
    general_threshold: f32,
    /// Minimum score for character / copyright tags to be included.
    character_threshold: f32,
    /// How many leading rows in the CSV are rating pseudo-tags (skipped in output).
    rating_count: usize,
}

impl OnnxTagger {
    /// Load the ONNX model and the labels CSV.
    ///
    /// `labels_path` must point to a WD14 `selected_tags.csv` with columns
    /// `tag_id,name,category[,count]`.  The first N rows whose `name` starts
    /// with `"rating:"` are rating tags and are excluded from output.
    pub fn new(
        model_path: &str,
        labels_path: &str,
        general_threshold: f32,
        character_threshold: f32,
    ) -> Result<Self> {
        tracing::info!(model_path, labels_path, "Loading WD14 ONNX model");

        let session = Session::builder()
            .map_err(|e| anyhow::anyhow!("Failed to create ONNX session builder: {e}"))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow::anyhow!("Failed to set optimization level: {e}"))?
            .commit_from_file(model_path)
            .map_err(|e| anyhow::anyhow!("Failed to load ONNX model from '{model_path}': {e}"))?;

        let (labels, rating_count) = Self::load_labels(labels_path)
            .with_context(|| format!("Failed to load labels from '{labels_path}'"))?;

        tracing::info!(
            label_count = labels.len(),
            rating_count,
            "WD14 model loaded"
        );

        Ok(OnnxTagger {
            session: Mutex::new(session),
            labels,
            general_threshold,
            character_threshold,
            rating_count,
        })
    }

    /// Parse `selected_tags.csv`.  Returns `(labels, rating_count)`.
    fn load_labels(path: &str) -> Result<(Vec<LabelEntry>, usize)> {
        #[derive(Deserialize)]
        struct Row {
            #[allow(dead_code)]
            tag_id: Option<i64>,
            name: String,
            category: i32,
            // `count` column is ignored
        }

        let mut reader = csv::Reader::from_path(path)
            .with_context(|| format!("Cannot open labels CSV '{path}'"))?;

        let mut all: Vec<LabelEntry> = Vec::new();
        for result in reader.deserialize::<Row>() {
            let row = result.context("Failed to parse labels CSV row")?;
            all.push(LabelEntry {
                name: row.name,
                category: row.category,
            });
        }

        // The first N rows where the name starts with "rating:" are the rating
        // pseudo-tags that have no useful threshold semantics.
        let rating_count = all
            .iter()
            .take_while(|e| e.name.starts_with("rating:"))
            .count();

        Ok((all, rating_count))
    }

    /// Decode, pad to square, resize to 448×448, and convert to a BGR
    /// float32 tensor with shape `[1, 448, 448, 3]` (NHWC layout).
    fn preprocess(image_bytes: &[u8]) -> Result<Array4<f32>> {
        use image::{imageops, DynamicImage, Rgb, RgbImage};

        let img: DynamicImage =
            image::load_from_memory(image_bytes).context("Failed to decode image")?;
        let img = img.to_rgb8();
        let (w, h) = img.dimensions();

        // Pad to square with a white background.
        let max_dim = w.max(h);
        let mut canvas = RgbImage::from_pixel(max_dim, max_dim, Rgb([255u8, 255, 255]));
        let x_off = (max_dim - w) / 2;
        let y_off = (max_dim - h) / 2;
        imageops::overlay(&mut canvas, &img, i64::from(x_off), i64::from(y_off));

        // Resize to the model's expected input size.
        let resized = imageops::resize(
            &canvas,
            WD14_INPUT_SIZE,
            WD14_INPUT_SIZE,
            imageops::FilterType::Lanczos3,
        );

        // Build NHWC tensor in BGR order (matching the original WD14 pipeline).
        let size = WD14_INPUT_SIZE as usize;
        let mut tensor = Array4::<f32>::zeros((1, size, size, 3));
        for y in 0..size {
            for x in 0..size {
                let px = resized.get_pixel(x as u32, y as u32);
                tensor[[0, y, x, 0]] = f32::from(px[2]); // B
                tensor[[0, y, x, 1]] = f32::from(px[1]); // G
                tensor[[0, y, x, 2]] = f32::from(px[0]); // R
            }
        }
        Ok(tensor)
    }
}

#[async_trait]
impl Tagger for OnnxTagger {
    async fn tag_image(&self, image_bytes: &[u8]) -> Result<Vec<TagPrediction>> {
        let array = Self::preprocess(image_bytes)?;

        let mut session = self
            .session
            .lock()
            .map_err(|_| anyhow::anyhow!("ONNX session mutex poisoned"))?;

        let input_name = session
            .inputs()
            .first()
            .context("ONNX model has no inputs")?
            .name()
            .to_string();

        let ort_tensor = Tensor::from_array(array)
            .map_err(|e| anyhow::anyhow!("Failed to create ONNX input tensor: {e}"))?;

        let outputs = session
            .run(ort::inputs![input_name.as_str() => ort_tensor])
            .map_err(|e| anyhow::anyhow!("ONNX inference failed: {e}"))?;

        if outputs.is_empty() {
            anyhow::bail!("ONNX model produced no outputs");
        }
        let (_, scores) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("Failed to extract output tensor: {e}"))?;

        let mut predictions = Vec::new();
        for (i, label) in self.labels.iter().enumerate() {
            if i < self.rating_count {
                continue;
            }
            let score = *scores.get(i).unwrap_or(&0.0_f32);
            let threshold = if label.category == CAT_CHARACTER {
                self.character_threshold
            } else {
                self.general_threshold
            };
            if score >= threshold {
                predictions.push(TagPrediction {
                    name: label.name.clone(),
                    category: Some(label.category_str().to_string()),
                    score,
                });
            }
        }

        // Sort descending by score for easier inspection.
        predictions.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(predictions)
    }
}

// ─── TaggerWorker ─────────────────────────────────────────────────────────────

/// One-shot worker: drains all pending tag jobs from the database and exits.
pub struct TaggerWorker {
    pub db: fulgorart_db::Db,
    pub tagger: Box<dyn Tagger>,
    pub http: reqwest::Client,
}

impl TaggerWorker {
    pub fn new(db: fulgorart_db::Db, tagger: Box<dyn Tagger>) -> Self {
        TaggerWorker {
            db,
            tagger,
            http: reqwest::Client::new(),
        }
    }

    /// Process every pending tag job and return the number of jobs handled.
    pub async fn run_once(&self) -> Result<usize> {
        let mut total = 0usize;
        loop {
            let jobs = self.db.get_pending_tag_jobs(50).await?;
            if jobs.is_empty() {
                break;
            }
            for job in &jobs {
                self.process_job(job).await?;
                total += 1;
            }
        }
        Ok(total)
    }

    async fn process_job(&self, job: &fulgorart_db::TagJobRow) -> Result<()> {
        self.db
            .update_tag_job_status(job.id, "running", None)
            .await?;

        let asset = match self.db.get_image_asset_by_id(job.image_id).await? {
            None => {
                self.db
                    .update_tag_job_status(job.id, "failed", Some("image not found"))
                    .await?;
                return Ok(());
            }
            Some(a) => a,
        };

        match self.download_and_tag(&asset).await {
            Ok(predictions) => {
                for pred in &predictions {
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
                    tags = predictions.len(),
                    "Tagged image"
                );
                self.db.update_tag_job_status(job.id, "done", None).await?;
            }
            Err(e) => {
                let msg = format!("{e:#}");
                tracing::error!(image_id = job.image_id, error = %msg, "Failed to tag image");
                self.db
                    .update_tag_job_status(job.id, "failed", Some(&msg))
                    .await?;
            }
        }
        Ok(())
    }

    async fn download_and_tag(
        &self,
        asset: &fulgorart_db::ImageAssetRow,
    ) -> Result<Vec<TagPrediction>> {
        tracing::debug!(url = %asset.r2_url, "Downloading image for tagging");
        let response = self
            .http
            .get(&asset.r2_url)
            .send()
            .await
            .context("HTTP request failed")?
            .error_for_status()
            .context("HTTP error status")?;
        let bytes: Bytes = response
            .bytes()
            .await
            .context("Failed to read image body")?;
        self.tagger.tag_image(&bytes).await
    }
}

// ─── Model file helpers ───────────────────────────────────────────────────────

/// Git LFS pointer files start with this header line.
const LFS_POINTER_HEADER: &[u8] = b"version https://git-lfs.github.com/spec/v1";

/// Return `true` if the first bytes of `path` look like a Git LFS pointer.
fn is_lfs_pointer(path: &std::path::Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = vec![0u8; LFS_POINTER_HEADER.len()];
    matches!(f.read_exact(&mut buf), Ok(())) && buf == LFS_POINTER_HEADER
}

/// Ensure that the file at `dest_path` exists and is a valid (non-LFS) file.
///
/// * If the file is missing or is a Git LFS pointer, and `url` is `Some`,
///   it will be downloaded from `url` and written to `dest_path` (creating
///   parent directories as needed).
/// * If the file is a Git LFS pointer and no URL is provided, an error with a
///   helpful message is returned so the user knows they need the real file.
async fn ensure_model_file(
    dest_path: &str,
    url: Option<&str>,
    http: &reqwest::Client,
) -> Result<()> {
    let path = std::path::Path::new(dest_path);

    let needs_download = if !path.exists() {
        tracing::info!(path = dest_path, "Model file not found; will download");
        true
    } else if is_lfs_pointer(path) {
        tracing::warn!(
            path = dest_path,
            "Model file appears to be a Git LFS pointer (not the real binary); will re-download"
        );
        true
    } else {
        false
    };

    if !needs_download {
        return Ok(());
    }

    let url = url.ok_or_else(|| {
        anyhow::anyhow!(
            "Model file '{}' is missing or is a Git LFS pointer, \
             and no download URL is configured. \
             Set WD14_MODEL_URL / WD14_LABELS_URL or place the files manually.",
            dest_path
        )
    })?;

    tracing::info!(url, dest = dest_path, "Downloading model file");

    // Create parent directories if needed.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory '{}'", parent.display()))?;
    }

    let response = http
        .get(url)
        .send()
        .await
        .with_context(|| format!("HTTP request failed for '{url}'"))?
        .error_for_status()
        .with_context(|| format!("HTTP error status for '{url}'"))?;

    let bytes = response
        .bytes()
        .await
        .with_context(|| format!("Failed to read response body from '{url}'"))?;

    std::fs::write(path, &bytes)
        .with_context(|| format!("Failed to write model file to '{dest_path}'"))?;

    tracing::info!(
        dest = dest_path,
        bytes = bytes.len(),
        "Model file downloaded successfully"
    );
    Ok(())
}

// ─── Entry point ──────────────────────────────────────────────────────────────

pub async fn run() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = fulgorart_core::AppConfig::from_env()?;
    let db = fulgorart_db::Db::connect(&config.db_path).await?;

    // Ensure the model and labels files are present (download if needed).
    let http = reqwest::Client::new();
    ensure_model_file(
        &config.wd14_model_path,
        config.wd14_model_url.as_deref(),
        &http,
    )
    .await
    .context("Failed to ensure WD14 model file")?;
    ensure_model_file(
        &config.wd14_labels_path,
        config.wd14_labels_url.as_deref(),
        &http,
    )
    .await
    .context("Failed to ensure WD14 labels file")?;

    let tagger = OnnxTagger::new(
        &config.wd14_model_path,
        &config.wd14_labels_path,
        config.wd14_general_threshold,
        config.wd14_character_threshold,
    )?;

    let worker = TaggerWorker::new(db, Box::new(tagger));
    let n = worker.run_once().await?;
    tracing::info!("Tagger: processed {} job(s)", n);

    Ok(())
}
