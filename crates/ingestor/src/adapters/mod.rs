mod pixiv;
pub mod x;

pub use pixiv::PixivAdapter;
pub use x::XAdapter;

use anyhow::Result;
use async_trait::async_trait;
use bytes::Bytes;

use crate::LikedPage;

/// A source of liked posts (one per service). Adding a service means
/// implementing this trait; the CLI handles paging, dedup and storage.
#[async_trait]
pub trait SourceAdapter: Send + Sync {
    /// Value stored as `post.source_type`, also used as the R2 key prefix.
    fn source_type(&self) -> &str;
    /// Fetch one page of liked posts, newest first. `cursor` is the previous
    /// page's `next_cursor`, or `None` for the first page.
    async fn fetch_liked_page(&self, cursor: Option<&str>) -> Result<LikedPage>;
    /// Download one image URL from a `SourcePost`, returning bytes and content type.
    async fn download_image(&self, url: &str) -> Result<(Bytes, String)>;
}
