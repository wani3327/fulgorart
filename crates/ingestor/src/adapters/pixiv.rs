use anyhow::{Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::{SourceAdapter, SourcePost};

pub struct PixivAdapter {
    access_token: String,
    user_id: Option<String>,
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
    #[serde(rename = "bookmark_data")]
    bookmark_metadata: Option<PixivBookmarkData>,
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
    pub fn new(access_token: &str, user_id: Option<String>) -> Self {
        Self {
            access_token: access_token.to_string(),
            user_id,
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
        if let Some(user_id) = &self.user_id {
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

        let mut seen_urls = HashSet::new();
        urls.into_iter()
            .filter(|url| seen_urls.insert(url.clone()))
            .collect()
    }

    fn extract_liked_at(illust: &PixivIllust) -> Option<String> {
        illust
            .bookmark_date
            .clone()
            .or_else(|| {
                illust
                    .bookmark_metadata
                    .as_ref()
                    .and_then(|data| data.date.clone())
            })
            .or_else(|| {
                illust
                    .bookmark_metadata
                    .as_ref()
                    .and_then(|data| data.created_at.clone())
            })
            .or_else(|| illust.create_date.clone())
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

                let liked_at = Self::extract_liked_at(&illust);
                if !Self::is_since_included(since_ts.as_ref(), liked_at.as_deref()) {
                    continue;
                }

                let image_urls = Self::extract_image_urls(&illust);
                if image_urls.is_empty() {
                    continue;
                }

                let raw_json = serde_json::to_string(&illust).ok();
                let (author_name, author_source_id, author_url) = illust
                    .user
                    .as_ref()
                    .map(|u| {
                        (
                            u.name.clone(),
                            Some(u.id.to_string()),
                            Some(format!("https://www.pixiv.net/users/{}", u.id)),
                        )
                    })
                    .unwrap_or((None, None, None));

                posts.push(SourcePost {
                    source_type: self.source_type().to_string(),
                    source_post_id: source_post_id.clone(),
                    source_post_url: format!("https://www.pixiv.net/artworks/{source_post_id}"),
                    liked_at,
                    author_name,
                    author_source_id,
                    author_url,
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
