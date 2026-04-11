use anyhow::Result;
use async_trait::async_trait;
use bytes::Bytes;
use fulgorart_core::AppConfig;
use fulgorart_db::{Db, ImageAssetRow};
use fulgorart_storage::R2Client;
use serde::{Deserialize, Serialize};
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

impl PixivAdapter {
    pub fn new(access_token: &str) -> Self {
        PixivAdapter {
            access_token: access_token.to_string(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl SourceAdapter for PixivAdapter {
    fn source_type(&self) -> &str {
        "pixiv"
    }

    async fn fetch_liked_posts(&self, _since: Option<&str>) -> Result<Vec<SourcePost>> {
        // TODO: Implement Pixiv bookmarks API call
        tracing::warn!("PixivAdapter::fetch_liked_posts is a stub");
        Ok(vec![])
    }

    async fn download_image(&self, url: &str) -> Result<(Bytes, String)> {
        let resp = self
            .client
            .get(url)
            .header("Referer", "https://www.pixiv.net/")
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

            for image_url in &post.image_urls {
                let (data, content_type) = adapter.download_image(image_url).await?;
                self.ingest_image(adapter, post_row.id, data, &content_type, image_url)
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
