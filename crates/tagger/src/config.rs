#[derive(Debug, Clone)]
pub struct TaggerConfig {
    pub model_path: String,
    pub labels_path: String,
    pub general_threshold: f32,
    pub character_threshold: f32,
}

impl TaggerConfig {
    pub fn from_env() -> Self {
        dotenvy::dotenv().ok();
        Self {
            model_path: std::env::var("WD14_MODEL_PATH")
                .unwrap_or_else(|_| "./models/wd14-convnext.onnx".to_string()),
            labels_path: std::env::var("WD14_LABELS_PATH")
                .unwrap_or_else(|_| "./models/selected_tags.csv".to_string()),
            general_threshold: std::env::var("WD14_GENERAL_THRESHOLD")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0.35),
            character_threshold: std::env::var("WD14_CHARACTER_THRESHOLD")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0.75),
        }
    }
}
