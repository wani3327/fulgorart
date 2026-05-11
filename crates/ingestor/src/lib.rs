use anyhow::{Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use fulgorart_core::AppConfig;
use fulgorart_db::{Db, ImageAssetRow};
use fulgorart_storage::R2Client;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tracing::instrument;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourcePost {
    pub source_type: String,
    pub source_post_id: String,
    pub source_post_url: String,
    pub liked_at: Option<String>,
    pub author_name: Option<String>,
    pub author_id: Option<String>,
    pub image_urls: Vec<String>,
    pub raw_json: Option<String>,
}

#[async_trait]
pub trait SourceAdapter: Send + Sync {
    fn source_type(&self) -> &str;
    async fn fetch_liked_posts(&self, since: Option<&str>) -> Result<Vec<SourcePost>>;
    async fn download_image(&self, url: &str) -> Result<(Bytes, String)>;
}

/// Pixiv adapter stub.
/// TODO: Implement Pixiv API calls using OAuth + pixiv API endpoints.
pub struct PixivAdapter {
    pub access_token: String,
    client: reqwest::Client,
}

#[derive(Debug, Deserialize, Serialize)]
struct PixivUserMeResponse {
    user: PixivUser,
}

#[derive(Debug, Deserialize, Serialize)]
struct PixivUser {
    id: PixivId,
    name: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum PixivId {
    String(String),
    Number(u64),
}

impl std::fmt::Display for PixivId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PixivId::String(v) => f.write_str(v),
            PixivId::Number(v) => write!(f, "{v}"),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct PixivBookmarksResponse {
    #[serde(default)]
    illusts: Vec<PixivIllust>,
    next_url: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PixivIllust {
    id: u64,
    user: Option<PixivUser>,
    create_date: Option<String>,
    bookmark_date: Option<String>,
    bookmark_data: Option<PixivBookmarkData>,
    meta_single_page: Option<PixivMetaSinglePage>,
    #[serde(default)]
    meta_pages: Vec<PixivMetaPage>,
    image_urls: Option<PixivImageUrls>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PixivBookmarkData {
    date: Option<String>,
    created_at: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PixivMetaSinglePage {
    original_image_url: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PixivMetaPage {
    image_urls: Option<PixivImageUrls>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PixivImageUrls {
    original: Option<String>,
    large: Option<String>,
    medium: Option<String>,
}

impl PixivAdapter {
    pub fn new(access_token: &str) -> Self {
        PixivAdapter {
            access_token: access_token.to_string(),
            client: reqwest::Client::new(),
        }
    }

    fn request_builder(&self, url: &str) -> reqwest::RequestBuilder {
        self.client
            .get(url)
            .bearer_auth(&self.access_token)
            .header("User-Agent", "fulgorart-ingestor/0.1")
    }

    async fn resolve_user_id(&self) -> Result<String> {
        if let Ok(user_id) = std::env::var("PIXIV_USER_ID") {
            let user_id = user_id.trim();
            if !user_id.is_empty() {
                return Ok(user_id.to_string());
            }
        }

        let me: PixivUserMeResponse = self
            .request_builder("https://app-api.pixiv.net/v2/user/me")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("Failed to parse Pixiv /v2/user/me response")?;

        Ok(me.user.id.to_string())
    }

    fn parse_since(since: Option<&str>) -> Option<chrono::DateTime<chrono::FixedOffset>> {
        since.and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
    }

    fn is_since_included(
        since: Option<&chrono::DateTime<chrono::FixedOffset>>,
        liked_at: Option<&str>,
    ) -> bool {
        let Some(since_ts) = since else {
            return true;
        };
        let Some(liked_at) = liked_at else {
            return true;
        };
        chrono::DateTime::parse_from_rfc3339(liked_at)
            .map(|ts| ts >= *since_ts)
            .unwrap_or(true)
    }

    fn extract_image_urls(illust: &PixivIllust) -> Vec<String> {
        let mut urls = Vec::new();

        if !illust.meta_pages.is_empty() {
            for page in &illust.meta_pages {
                if let Some(image_urls) = &page.image_urls {
                    if let Some(url) = image_urls
                        .original
                        .as_deref()
                        .or(image_urls.large.as_deref())
                        .or(image_urls.medium.as_deref())
                    {
                        urls.push(url.to_string());
                    }
                }
            }
        } else if let Some(url) = illust
            .meta_single_page
            .as_ref()
            .and_then(|single| single.original_image_url.as_deref())
        {
            urls.push(url.to_string());
        } else if let Some(image_urls) = &illust.image_urls {
            if let Some(url) = image_urls
                .original
                .as_deref()
                .or(image_urls.large.as_deref())
                .or(image_urls.medium.as_deref())
            {
                urls.push(url.to_string());
            }
        }

        let mut dedup = HashSet::new();
        urls.into_iter()
            .filter(|u| dedup.insert(u.clone()))
            .collect()
    }
}

#[async_trait]
impl SourceAdapter for PixivAdapter {
    fn source_type(&self) -> &str {
        "pixiv"
    }

    async fn fetch_liked_posts(&self, since: Option<&str>) -> Result<Vec<SourcePost>> {
        let user_id = self.resolve_user_id().await?;
        let mut next_url = Some(format!(
            "https://app-api.pixiv.net/v1/user/bookmarks/illust?user_id={user_id}&restrict=public"
        ));
        let mut posts = Vec::new();
        let mut seen_post_ids = HashSet::new();
        let since_ts = Self::parse_since(since);

        while let Some(url) = next_url.take() {
            let response: PixivBookmarksResponse = self
                .request_builder(&url)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await
                .with_context(|| format!("Failed to parse Pixiv bookmarks response: {url}"))?;

            let PixivBookmarksResponse {
                illusts,
                next_url: new_next_url,
            } = response;

            for illust in illusts {
                let source_post_id = illust.id.to_string();
                if !seen_post_ids.insert(source_post_id.clone()) {
                    continue;
                }

                let liked_at = illust
                    .bookmark_date
                    .clone()
                    .or_else(|| illust.bookmark_data.as_ref().and_then(|d| d.date.clone()))
                    .or_else(|| {
                        illust
                            .bookmark_data
                            .as_ref()
                            .and_then(|d| d.created_at.clone())
                    })
                    .or_else(|| illust.create_date.clone());
                if !Self::is_since_included(since_ts.as_ref(), liked_at.as_deref()) {
                    continue;
                }

                let image_urls = Self::extract_image_urls(&illust);
                if image_urls.is_empty() {
                    continue;
                }

                let raw_json = serde_json::to_string(&illust).ok();
                let (author_name, author_id) = illust
                    .user
                    .as_ref()
                    .map(|u| (u.name.clone(), Some(u.id.to_string())))
                    .unwrap_or((None, None));

                posts.push(SourcePost {
                    source_type: self.source_type().to_string(),
                    source_post_id: source_post_id.clone(),
                    source_post_url: format!("https://www.pixiv.net/artworks/{source_post_id}"),
                    liked_at,
                    author_name,
                    author_id,
                    image_urls,
                    raw_json,
                });
            }

            next_url = new_next_url;
        }

        Ok(posts)
    }

    async fn download_image(&self, url: &str) -> Result<(Bytes, String)> {
        let resp = self
            .client
            .get(url)
            .bearer_auth(&self.access_token)
            .header("Referer", "https://www.pixiv.net/")
            .send()
            .await?
            .error_for_status()?;
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();
        let data = resp.bytes().await?;
        Ok((data, content_type))
    }
}

/// Twitter/X adapter stub.
pub struct TwitterAdapter {
    pub bearer_token: String,
    client: reqwest::Client,
}

impl TwitterAdapter {
    pub fn new(bearer_token: &str) -> Self {
        TwitterAdapter {
            bearer_token: bearer_token.to_string(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl SourceAdapter for TwitterAdapter {
    fn source_type(&self) -> &str {
        "twitter"
    }

    async fn fetch_liked_posts(&self, _since: Option<&str>) -> Result<Vec<SourcePost>> {
        // TODO: Implement Twitter v2 liked tweets API
        tracing::warn!("TwitterAdapter::fetch_liked_posts is a stub");
        Ok(vec![])
    }

    async fn download_image(&self, url: &str) -> Result<(Bytes, String)> {
        let resp = self
            .client
            .get(url)
            .bearer_auth(&self.bearer_token)
            .send()
            .await?;
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();
        let data = resp.bytes().await?;
        Ok((data, content_type))
    }
}

pub struct IngestorService {
    db: Db,
    r2: R2Client,
    config: AppConfig,
}

impl IngestorService {
    pub fn new(db: Db, r2: R2Client, config: AppConfig) -> Self {
        IngestorService { db, r2, config }
    }

    #[instrument(skip(self, adapter, image_bytes))]
    pub async fn ingest_image(
        &self,
        adapter: &dyn SourceAdapter,
        post_id: i64,
        group_id: i64,
        image_bytes: Bytes,
        content_type: &str,
        source_url: &str,
    ) -> Result<ImageAssetRow> {
        use sha2::Digest;
        let sha256 = {
            let mut hasher = sha2::Sha256::new();
            hasher.update(&image_bytes);
            hex::encode(hasher.finalize())
        };

        let ext = match content_type {
            "image/png" => "png",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => "jpg",
        };

        let key = R2Client::canonical_key(adapter.source_type(), &sha256, ext);
        let r2_url = self.r2.object_url(&self.config.r2_bucket, &key);

        self.r2
            .upload(
                &self.config.r2_bucket,
                &key,
                image_bytes.clone(),
                content_type,
            )
            .await?;

        let asset = self
            .db
            .insert_image_asset(
                Some(post_id),
                Some(group_id),
                &sha256,
                &key,
                &r2_url,
                None,
                None,
                Some(image_bytes.len() as i64),
                content_type,
                Some(source_url),
            )
            .await?;

        self.db.insert_tag_job(asset.id).await?;

        Ok(asset)
    }

    pub async fn run_adapter(&self, adapter: &dyn SourceAdapter) -> Result<usize> {
        let posts = adapter.fetch_liked_posts(None).await?;
        let mut count = 0;
        for post in posts {
            let post_row = self
                .db
                .insert_post(
                    &post.source_type,
                    &post.source_post_id,
                    &post.source_post_url,
                    post.liked_at.as_deref(),
                    post.author_name.as_deref(),
                    post.author_id.as_deref(),
                    post.raw_json.as_deref(),
                )
                .await?;
            let group_row = self.db.insert_image_group(Some(post_row.id)).await?;

            for image_url in &post.image_urls {
                let (data, content_type) = adapter.download_image(image_url).await?;
                self.ingest_image(
                    adapter,
                    post_row.id,
                    group_row.id,
                    data,
                    &content_type,
                    image_url,
                )
                .await?;
                count += 1;
            }
        }
        Ok(count)
    }
}

pub async fn run() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = AppConfig::from_env()?;
    let db = Db::connect(&config.db_path).await?;
    let r2 = R2Client::new(&config).await?;

    let service = IngestorService::new(db, r2, config);

    let pixiv_token = std::env::var("PIXIV_ACCESS_TOKEN").ok();
    let twitter_token = std::env::var("TWITTER_BEARER_TOKEN").ok();

    if pixiv_token.is_none() && twitter_token.is_none() {
        tracing::warn!(
            "No adapter credentials found \
             (PIXIV_ACCESS_TOKEN / TWITTER_BEARER_TOKEN). Nothing to do."
        );
        return Ok(());
    }

    if let Some(ref token) = pixiv_token {
        let adapter = PixivAdapter::new(token);
        let n = service.run_adapter(&adapter).await?;
        tracing::info!("Pixiv: ingested {} image(s)", n);
    }

    if let Some(ref token) = twitter_token {
        let adapter = TwitterAdapter::new(token);
        let n = service.run_adapter(&adapter).await?;
        tracing::info!("Twitter: ingested {} image(s)", n);
    }

    Ok(())
}
