use bytes::Bytes;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourcePost {
    pub source_type: String,
    pub source_post_id: String,
    pub source_post_url: String,
    pub liked_at: Option<String>,
    pub author_name: Option<String>,
    pub author_source_id: Option<String>,
    pub author_url: Option<String>,
    pub image_urls: Vec<String>,
    pub raw_json: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GrabbedImage {
    pub source_url: String,
    pub content_type: String,
    pub bytes: Bytes,
}

#[derive(Debug, Clone)]
pub struct GrabbedPost {
    pub source_type: String,
    pub source_post_id: String,
    pub source_post_url: String,
    pub liked_at: Option<String>,
    pub author_name: Option<String>,
    pub author_source_id: Option<String>,
    pub author_url: Option<String>,
    pub raw_json: Option<String>,
    pub images: Vec<GrabbedImage>,
}
