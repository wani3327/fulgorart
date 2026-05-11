use anyhow::Result;
use aws_sdk_s3::primitives::ByteStream;
use fulgorart_core::AppConfig;
use tracing::instrument;

pub struct R2Client {
    client: aws_sdk_s3::Client,
    endpoint_url: String,
}

impl R2Client {
    pub async fn new(config: &AppConfig) -> Result<Self> {
        use aws_credential_types::Credentials;

        let creds = Credentials::new(
            &config.r2_access_key_id,
            &config.r2_secret_access_key,
            None,
            None,
            "fulgorart",
        );

        let s3_config = aws_sdk_s3::Config::builder()
            .credentials_provider(creds)
            .endpoint_url(&config.r2_endpoint)
            .region(aws_sdk_s3::config::Region::new("auto"))
            .force_path_style(true)
            .build();

        let client = aws_sdk_s3::Client::from_conf(s3_config);
        Ok(R2Client {
            client,
            endpoint_url: config.r2_endpoint.clone(),
        })
    }

    #[instrument(skip(self))]
    pub async fn download(&self, bucket: &str, key: &str) -> Result<bytes::Bytes> {
        let output = self
            .client
            .get_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| {
                anyhow::anyhow!("Failed to get object {key} from bucket {bucket}: {e:?}")
            })?;

        let data = output
            .body
            .collect()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read object body for key {key}: {e}"))?
            .into_bytes();

        Ok(data)
    }

    #[instrument(skip(self, data))]
    pub async fn upload(
        &self,
        bucket: &str,
        key: &str,
        data: bytes::Bytes,
        content_type: &str,
    ) -> Result<()> {
        self.client
            .put_object()
            .bucket(bucket)
            .key(key)
            .body(ByteStream::from(data))
            .content_type(content_type)
            .send()
            .await?;
        Ok(())
    }

    pub fn object_url(&self, bucket: &str, key: &str) -> String {
        format!(
            "{}/{}/{}",
            self.endpoint_url.trim_end_matches('/'),
            bucket,
            key
        )
    }

    pub fn canonical_key(source_type: &str, sha256: &str, ext: &str) -> String {
        let now = chrono::Utc::now();
        format!(
            "images/{}/{}/{}/{}/{}.{}",
            source_type,
            now.format("%Y"),
            now.format("%m"),
            now.format("%d"),
            sha256,
            ext
        )
    }
}
