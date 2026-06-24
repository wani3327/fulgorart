mod adapters;
mod config;
mod model;

pub use adapters::{PixivAdapter, SourceAdapter, TwitterAdapter};
pub use config::IngestorConfig;
pub use model::{GrabbedImage, GrabbedPost, SourcePost};

use anyhow::Result;
use std::path::Path;

/// Grabs liked posts from different sources. SourcePost(adapter) -> GrabbedPost
/// 실제로 하는 일: download_image를 불러서 이미지를 Bytes 형식으로 메모리에 올림.
pub async fn grab<T: SourceAdapter + ?Sized>(adapter: &T) -> Result<Vec<GrabbedPost>> {
    let mut grabbed_posts = Vec::new();

    let posts = adapter.fetch_liked_posts(None).await?;
    for post in posts {
        let mut images = Vec::new();
        for image_url in &post.image_urls {
            let (bytes, content_type) = adapter.download_image(image_url).await?;
            images.push(GrabbedImage {
                source_url: image_url.clone(),
                content_type,
                bytes,
            });
        }

        if !images.is_empty() {
            grabbed_posts.push(GrabbedPost {
                source_type: post.source_type,
                source_post_id: post.source_post_id,
                source_post_url: post.source_post_url,
                liked_at: post.liked_at,
                author_name: post.author_name,
                author_source_id: post.author_source_id,
                author_url: post.author_url,
                raw_json: post.raw_json,
                images,
            });
        }
    }

    Ok(grabbed_posts)
}

pub async fn run_to_directory(config: &IngestorConfig, output_dir: &Path) -> Result<usize> {
    let adapters = config.build_adapters();
    if adapters.is_empty() {
        tracing::warn!(
            "No adapter credentials found (PIXIV_ACCESS_TOKEN / TWITTER_BEARER_TOKEN). Nothing to do."
        );
        return Ok(0);
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .try_init()
        .ok();

    let mut posts = Vec::new();

    for adapter in adapters {
        posts.extend(grab(adapter.as_ref()).await?);
    }

    save_grabbed_posts(output_dir, &posts).await
}

pub async fn save_grabbed_posts(output_dir: &Path, posts: &[GrabbedPost]) -> Result<usize> {
    tokio::fs::create_dir_all(output_dir).await?;
    let mut saved = 0usize;

    for post in posts {
        let post_dir = output_dir
            .join(sanitize_filename_component(&post.source_type))
            .join(sanitize_filename_component(&post.source_post_id));
        tokio::fs::create_dir_all(&post_dir).await?;

        for (index, image) in post.images.iter().enumerate() {
            let file_path = post_dir.join(format!(
                "{:03}.{}",
                index + 1,
                file_extension(&image.content_type)
            ));
            tokio::fs::write(&file_path, image.bytes.as_ref()).await?;
            tracing::info!(path = %file_path.display(), source_url = %image.source_url, "Saved grabbed image");
            saved += 1;
        }
    }

    Ok(saved)
}

fn sanitize_filename_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn file_extension(content_type: &str) -> &'static str {
    match content_type {
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "jpg",
    }
}
