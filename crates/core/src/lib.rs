use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    Pixiv,
    Twitter,
}

impl std::fmt::Display for SourceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceType::Pixiv => write!(f, "pixiv"),
            SourceType::Twitter => write!(f, "twitter"),
        }
    }
}

impl std::str::FromStr for SourceType {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pixiv" => Ok(SourceType::Pixiv),
            "twitter" => Ok(SourceType::Twitter),
            _ => Err(anyhow::anyhow!("Unknown source type: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagSource {
    Wd14,
    Manual,
}

impl std::fmt::Display for TagSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TagSource::Wd14 => write!(f, "wd14"),
            TagSource::Manual => write!(f, "manual"),
        }
    }
}

impl std::str::FromStr for TagSource {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "wd14" => Ok(TagSource::Wd14),
            "manual" => Ok(TagSource::Manual),
            _ => Err(anyhow::anyhow!("Unknown tag source: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Pending,
    Running,
    Done,
    Failed,
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobStatus::Pending => write!(f, "pending"),
            JobStatus::Running => write!(f, "running"),
            JobStatus::Done => write!(f, "done"),
            JobStatus::Failed => write!(f, "failed"),
        }
    }
}

impl std::str::FromStr for JobStatus {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(JobStatus::Pending),
            "running" => Ok(JobStatus::Running),
            "done" => Ok(JobStatus::Done),
            "failed" => Ok(JobStatus::Failed),
            _ => Err(anyhow::anyhow!("Unknown job status: {}", s)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub db_path: String,
    pub r2_bucket: String,
    pub r2_endpoint: String,
    pub r2_access_key_id: String,
    pub r2_secret_access_key: String,
    pub password: Option<String>,
    pub port: u16,
    pub wd14_model_path: String,
    pub wd14_labels_path: String,
    pub wd14_general_threshold: f32,
    pub wd14_character_threshold: f32,
}

impl AppConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        dotenvy::dotenv().ok();
        Ok(AppConfig {
            db_path: std::env::var("FULGORART_DB_PATH")
                .unwrap_or_else(|_| "./data/fulgorart.db".to_string()),
            r2_bucket: std::env::var("FULGORART_R2_BUCKET")
                .unwrap_or_else(|_| "fulgorart-images".to_string()),
            r2_endpoint: std::env::var("FULGORART_R2_ENDPOINT")
                .unwrap_or_else(|_| "https://example.r2.cloudflarestorage.com".to_string()),
            r2_access_key_id: std::env::var("FULGORART_R2_ACCESS_KEY_ID")
                .unwrap_or_default(),
            r2_secret_access_key: std::env::var("FULGORART_R2_SECRET_ACCESS_KEY")
                .unwrap_or_default(),
            password: std::env::var("FULGORART_PASSWORD").ok(),
            port: std::env::var("FULGORART_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(3000),
            wd14_model_path: std::env::var("WD14_MODEL_PATH")
                .unwrap_or_else(|_| "./models/wd14-convnext.onnx".to_string()),
            wd14_labels_path: std::env::var("WD14_LABELS_PATH")
                .unwrap_or_else(|_| "./models/selected_tags.csv".to_string()),
            wd14_general_threshold: std::env::var("WD14_GENERAL_THRESHOLD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.35),
            wd14_character_threshold: std::env::var("WD14_CHARACTER_THRESHOLD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.75),
        })
    }
}
