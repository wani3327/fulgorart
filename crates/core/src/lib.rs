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
            _ => Err(anyhow::anyhow!("Unknown source type: {s}")),
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
            _ => Err(anyhow::anyhow!("Unknown tag source: {s}")),
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
            _ => Err(anyhow::anyhow!("Unknown job status: {s}")),
        }
    }
}
