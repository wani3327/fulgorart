mod config;

use anyhow::{Context, Result};
use async_trait::async_trait;
pub use config::TaggerConfig;
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
            9 => "rating",
            _ => "general",
        }
    }
}

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

    pub fn from_env() -> Result<Self> {
        let config = config::TaggerConfig::from_env();
        Self::new(
            &config.model_path,
            &config.labels_path,
            config.general_threshold,
            config.character_threshold,
        )
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
        let rating_count = 4; /* all
                              .iter()
                              .take_while(|e| e.name.starts_with("rating:"))
                              .count(); */

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

    fn run_inference(&self, image_bytes: &[u8]) -> Result<Vec<TagPrediction>> {
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

        if outputs.len() == 0 {
            anyhow::bail!("ONNX model produced no outputs");
        }
        let (_, scores) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("Failed to extract output tensor: {e}"))?;

        let mut predictions = Vec::new();
        for (i, label) in self.labels.iter().enumerate() {
            let score = *scores.get(i).unwrap_or(&0.0_f32);
            if i < self.rating_count {
                // Always include all rating tags regardless of threshold.
                predictions.push(TagPrediction {
                    name: label.name.clone(),
                    category: Some("rating".to_string()),
                    score,
                });
                continue;
            }
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

#[async_trait]
impl Tagger for OnnxTagger {
    async fn tag_image(&self, image_bytes: &[u8]) -> Result<Vec<TagPrediction>> {
        self.run_inference(image_bytes)
    }
}
