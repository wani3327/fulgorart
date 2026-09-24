use anyhow::{Context, Result};
use aws_sdk_s3::primitives::ByteStream;
use image::ImageFormat;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::instrument;

#[derive(Debug, Clone)]
pub struct R2Config {
    pub bucket: String,
    pub endpoint: String,
    pub access_key_id: String,
    pub secret_access_key: String,
}

impl R2Config {
    pub fn from_env() -> Self {
        Self {
            bucket: std::env::var("FULGORART_R2_BUCKET")
                .unwrap_or_else(|_| "fulgorart-images".to_string()),
            endpoint: std::env::var("FULGORART_R2_ENDPOINT")
                .unwrap_or_else(|_| "https://example.r2.cloudflarestorage.com".to_string()),
            access_key_id: std::env::var("FULGORART_R2_ACCESS_KEY_ID").unwrap_or_default(),
            secret_access_key: std::env::var("FULGORART_R2_SECRET_ACCESS_KEY").unwrap_or_default(),
        }
    }
}

#[derive(Clone)]
pub struct R2Client {
    client: aws_sdk_s3::Client,
    endpoint_url: String,
    bucket: String,
}

impl R2Client {
    const THUMBNAIL_MAX_DIMENSION: u32 = 480;

    pub async fn new(config: &R2Config) -> Result<Self> {
        use aws_credential_types::Credentials;

        let creds = Credentials::new(
            &config.access_key_id,
            &config.secret_access_key,
            None,
            None,
            "fulgorart",
        );

        let s3_config = aws_sdk_s3::Config::builder()
            .credentials_provider(creds)
            .endpoint_url(&config.endpoint)
            .region(aws_sdk_s3::config::Region::new("auto"))
            .force_path_style(true)
            .build();

        let client = aws_sdk_s3::Client::from_conf(s3_config);
        Ok(R2Client {
            client,
            endpoint_url: config.endpoint.clone(),
            bucket: config.bucket.clone(),
        })
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    #[instrument(skip(self))]
    pub async fn download(&self, key: &str) -> Result<bytes::Bytes> {
        let output = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to get object {key} from bucket {}: {e:?}",
                    self.bucket
                )
            })?;

        let data = output
            .body
            .collect()
            .await
            .with_context(|| format!("Failed to read object body for key {key}"))?
            .into_bytes();

        Ok(data)
    }

    #[instrument(skip(self, data))]
    pub async fn upload(&self, key: &str, data: Vec<u8>, content_type: &str) -> Result<()> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(data))
            .content_type(content_type)
            .send()
            .await?;
        Ok(())
    }

    #[instrument(skip(self, data))]
    pub async fn upload_with_thumbnail(
        &self,
        key_base: &str,
        data: Vec<u8>,
        content_type: &str,
    ) -> Result<()> {
        let original_key = Self::original_key(key_base);
        let thumbnail_key = Self::thumbnail_key(key_base);
        let thumbnail_data = Self::build_thumbnail_webp(&data)?;

        self.upload(&original_key, data, content_type).await?;
        self.upload(&thumbnail_key, thumbnail_data, "image/webp")
            .await?;

        Ok(())
    }

    pub fn object_url(&self, key: &str) -> String {
        format!(
            "{}/{}/{}",
            self.endpoint_url.trim_end_matches('/'),
            self.bucket,
            key
        )
    }

    #[instrument(skip(self))]
    pub async fn presigned_object_url(&self, key: &str, ttl: Duration) -> Result<String> {
        let presigning_config = aws_sdk_s3::presigning::PresigningConfig::expires_in(ttl)?;
        let presigned_request = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .presigned(presigning_config)
            .await
            .with_context(|| format!("Failed to presign object URL for key {key}"))?;

        Ok(presigned_request.uri().to_string())
    }

    pub fn canonical_key_base(category: &str, filename: &str) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        format!("{category}/{filename}.{now}")
    }

    fn original_key(key: &str) -> String {
        format!("{key}/original")
    }

    fn thumbnail_key(key: &str) -> String {
        format!("{key}/thumbnail")
    }

    fn build_thumbnail_webp(data: &[u8]) -> Result<Vec<u8>> {
        let image =
            image::load_from_memory(data).context("Failed to decode image for thumbnail")?;
        let thumbnail =
            image.thumbnail(Self::THUMBNAIL_MAX_DIMENSION, Self::THUMBNAIL_MAX_DIMENSION);
        let mut out = std::io::Cursor::new(Vec::new());
        thumbnail
            .write_to(&mut out, ImageFormat::WebP)
            .context("Failed to encode thumbnail as WebP")?;
        Ok(out.into_inner())
    }
}
