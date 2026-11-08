use bytes::Bytes;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourcePost {
    pub source_type: String,
    pub source_post_id: String,
    pub source_post_url: String,
    pub liked_at: Option<String>,
    /// When the post itself was published (RFC 3339), if the source says.
    pub posted_at: Option<String>,
    /// Post text / caption.
    pub caption: Option<String>,
    pub author_name: Option<String>,
    pub author_source_id: Option<String>,
    pub author_url: Option<String>,
    pub image_urls: Vec<String>,
    pub raw_json: Option<String>,
}

/// One page of liked posts, newest like first.
#[derive(Debug, Clone, Default)]
pub struct LikedPage {
    pub posts: Vec<SourcePost>,
    /// Pass to the next `fetch_liked_page` call; `None` means this was the last page.
    pub next_cursor: Option<String>,
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
    pub posted_at: Option<String>,
    pub caption: Option<String>,
    pub author_name: Option<String>,
    pub author_source_id: Option<String>,
    pub author_url: Option<String>,
    pub raw_json: Option<String>,
    pub images: Vec<GrabbedImage>,
}
