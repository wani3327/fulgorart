use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use serde::Deserialize;
use std::collections::HashSet;
use tokio::process::Command;

use crate::{SourceAdapter, SourcePost};

pub struct PixivAdapter {
    access_token: String,
    user_id: Option<String>,
    client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct PythonPost {
    source_post_id: String,
    source_post_url: String,
    liked_at: Option<String>,
    author_name: Option<String>,
    author_id: Option<String>,
    image_urls: Vec<String>,
    raw_json: Option<String>,
}

impl PixivAdapter {
    pub fn new(access_token: &str, user_id: Option<String>) -> Self {
        Self {
            access_token: access_token.to_string(),
            user_id,
            client: reqwest::Client::new(),
        }
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

    async fn fetch_with_pixivpy3(&self) -> Result<Vec<PythonPost>> {
        let script = r#"
import json
import os
import sys

from pixivpy3 import AppPixivAPI


def to_dict(value):
    if value is None:
        return None
    if isinstance(value, dict):
        return value
    if hasattr(value, "model_dump"):
        return value.model_dump()
    if hasattr(value, "dict"):
        return value.dict()
    try:
        return dict(value)
    except Exception:
        return None


def get_value(value, key, default=None):
    if value is None:
        return default
    if isinstance(value, dict):
        return value.get(key, default)
    return getattr(value, key, default)


def pick_first_image(image_urls):
    image_urls = to_dict(image_urls) or {}
    return image_urls.get("original") or image_urls.get("large") or image_urls.get("medium")


def extract_image_urls(illust):
    urls = []

    meta_pages = get_value(illust, "meta_pages", []) or []
    if meta_pages:
        for page in meta_pages:
            image_urls = get_value(page, "image_urls")
            url = pick_first_image(image_urls)
            if url:
                urls.append(url)
    else:
        meta_single_page = get_value(illust, "meta_single_page")
        original = get_value(meta_single_page, "original_image_url")
        if original:
            urls.append(original)
        else:
            image_urls = get_value(illust, "image_urls")
            url = pick_first_image(image_urls)
            if url:
                urls.append(url)

    deduped = []
    seen = set()
    for url in urls:
        if url not in seen:
            seen.add(url)
            deduped.append(url)
    return deduped


def extract_liked_at(illust):
    bookmark_data = get_value(illust, "bookmark_data")
    return (
        get_value(illust, "bookmark_date")
        or get_value(bookmark_data, "date")
        or get_value(bookmark_data, "created_at")
        or get_value(illust, "create_date")
    )


def main():
    access_token = os.environ.get("PIXIV_ACCESS_TOKEN")
    if not access_token:
        raise RuntimeError("PIXIV_ACCESS_TOKEN is required")

    user_id = (os.environ.get("PIXIV_USER_ID") or "").strip() or None

    api = AppPixivAPI()
    api.set_auth(access_token, None)

    if user_id is None:
        me_response = api.no_auth_requests_call(
            "GET", f"{api.hosts}/v2/user/me", req_auth=True
        )
        me = api.parse_result(me_response)
        me_user = get_value(me, "user")
        me_user_id = get_value(me_user, "id")
        if me_user_id is None:
            raise RuntimeError("Could not resolve Pixiv user id")
        user_id = str(me_user_id)

    posts = []
    seen_post_ids = set()
    params = {"user_id": user_id, "restrict": "public"}

    while params:
        result = api.user_bookmarks_illust(**params)
        for illust in get_value(result, "illusts", []) or []:
            illust_id = get_value(illust, "id")
            if illust_id is None:
                continue

            source_post_id = str(illust_id)
            if source_post_id in seen_post_ids:
                continue
            seen_post_ids.add(source_post_id)

            image_urls = extract_image_urls(illust)
            if not image_urls:
                continue

            user = get_value(illust, "user")
            posts.append(
                {
                    "source_post_id": source_post_id,
                    "source_post_url": f"https://www.pixiv.net/artworks/{source_post_id}",
                    "liked_at": extract_liked_at(illust),
                    "author_name": get_value(user, "name"),
                    "author_id": str(get_value(user, "id")) if get_value(user, "id") is not None else None,
                    "image_urls": image_urls,
                    "raw_json": json.dumps(to_dict(illust), ensure_ascii=False, separators=(",", ":")),
                }
            )

        next_url = get_value(result, "next_url")
        params = api.parse_qs(next_url)

    sys.stdout.write(json.dumps(posts, ensure_ascii=False))


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:
        print(f"pixivpy3 error: {exc}", file=sys.stderr)
        raise
"#;

        let user_id = self.user_id.clone().unwrap_or_default();
        let output = Command::new("python3")
            .arg("-c")
            .arg(script)
            .env("PIXIV_ACCESS_TOKEN", &self.access_token)
            .env("PIXIV_USER_ID", user_id)
            .output()
            .await
            .context("failed to execute python3 for pixivpy3")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("pixivpy3 command failed: {stderr}"));
        }

        let stdout =
            String::from_utf8(output.stdout).context("pixivpy3 output was not valid UTF-8")?;
        let posts: Vec<PythonPost> =
            serde_json::from_str(&stdout).context("failed to parse pixivpy3 output JSON")?;

        Ok(posts)
    }
}

#[async_trait]
impl SourceAdapter for PixivAdapter {
    fn source_type(&self) -> &str {
        "pixiv"
    }

    async fn fetch_liked_posts(&self, since: Option<&str>) -> Result<Vec<SourcePost>> {
        let posts = self.fetch_with_pixivpy3().await?;
        let since_ts = Self::parse_since(since);

        let mut result = Vec::with_capacity(posts.len());
        for post in posts {
            if !Self::is_since_included(since_ts.as_ref(), post.liked_at.as_deref()) {
                continue;
            }

            let mut seen = HashSet::new();
            let image_urls = post
                .image_urls
                .into_iter()
                .filter(|url| seen.insert(url.clone()))
                .collect::<Vec<_>>();
            if image_urls.is_empty() {
                continue;
            }

            result.push(SourcePost {
                source_type: self.source_type().to_string(),
                source_post_id: post.source_post_id,
                source_post_url: post.source_post_url,
                liked_at: post.liked_at,
                author_name: post.author_name,
                author_id: post.author_id,
                image_urls,
                raw_json: post.raw_json,
            });
        }

        Ok(result)
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
