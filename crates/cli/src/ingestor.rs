//! `cli ingestor <service>`: pull liked posts from a service into R2 + the DB.
//!
//! Each service is a `SourceAdapter` from `fulgorart-ingestor`; the paging,
//! dedup and storage below are shared. To add a service: implement the
//! adapter, add a `Source` variant, and call `ingest` with it.

use std::io::{self, BufRead as _, Write as _};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args as ClapArgs, Subcommand};
use fulgorart_db::{
    Db, DbConfig, GalleryImportAuthorArgs, GalleryImportImageArgs, GalleryImportPostArgs,
};
use fulgorart_ingestor::{
    x::{self as x_api, XAppConfig},
    SourceAdapter, SourcePost, XAdapter,
};
use fulgorart_storage::R2Client;

use crate::gallery_dl_tool::image_metadata;

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    pub source: Source,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Source {
    /// Images from posts you liked on X (Twitter), via the official API
    X(XArgs),
}

#[derive(Debug, Clone, ClapArgs)]
#[command(args_conflicts_with_subcommands = true)]
pub struct XArgs {
    #[command(subcommand)]
    pub command: Option<XCommand>,
    #[command(flatten)]
    pub options: IngestOptions,
}

#[derive(Debug, Clone, Subcommand)]
pub enum XCommand {
    /// Authorize access to your X likes (needed once; tokens refresh themselves)
    Login,
}

#[derive(Debug, Clone, ClapArgs)]
pub struct IngestOptions {
    /// Stop after this many pages of likes (0 = no limit). The source API may
    /// bill per post read, so the default keeps a run small
    #[arg(long, default_value_t = 10)]
    pub max_pages: u32,

    /// Keep paging past likes that were already imported. Normally a run stops
    /// at the first page whose posts are all known; use this to backfill gaps
    #[arg(long)]
    pub full: bool,

    /// Only list what would be imported; download, upload and write nothing
    #[arg(long)]
    pub dry_run: bool,
}

pub async fn run(args: Args) -> Result<()> {
    match args.source {
        Source::X(x) => match x.command {
            Some(XCommand::Login) => x_login(&x_config()?).await,
            None => {
                let adapter = XAdapter::new(x_config()?)?;
                println!("source=x user=@{}", adapter.username().await);
                ingest_with(&adapter, &x.options).await
            }
        },
    }
}

/// X app settings; the token file defaults to sitting next to the DB, so it
/// lives in the same volume in Docker.
fn x_config() -> Result<XAppConfig> {
    let db_path = DbConfig::from_env().path;
    let default_token_path = db_path
        .parent()
        .map(|dir| dir.join("x-token.json"))
        .unwrap_or_else(|| PathBuf::from("x-token.json"));
    XAppConfig::from_env(default_token_path)
}

async fn x_login(config: &XAppConfig) -> Result<()> {
    let start = x_api::login_start(config)?;
    println!("1. Open this URL and approve access:\n\n{}\n", start.authorize_url);
    println!(
        "2. Your browser is then sent to {} (the page may fail to load; that's fine).",
        config.redirect_uri
    );
    print!("3. Paste the full URL from the address bar here: ");
    io::stdout().flush()?;

    let mut redirected = String::new();
    io::stdin().lock().read_line(&mut redirected)?;
    let tokens = x_api::login_finish(config, &start, &redirected).await?;
    println!(
        "Logged in as @{} (id {}). Tokens saved to {}",
        tokens.username,
        tokens.user_id,
        config.token_path.display()
    );
    Ok(())
}

async fn ingest_with(adapter: &dyn SourceAdapter, options: &IngestOptions) -> Result<()> {
    let db = crate::connect_db().await?;
    let r2 = if options.dry_run {
        None
    } else {
        Some(crate::connect_r2().await?)
    };
    let summary = ingest(adapter, &db, r2.as_ref(), options).await?;
    println!(
        "pages={} new_posts={} known_posts={} uploaded_images={} duplicate_images={} failed_images={}{}",
        summary.pages,
        summary.new_posts,
        summary.known_posts,
        summary.uploaded,
        summary.duplicates,
        summary.failed,
        if summary.hit_page_limit {
            " (stopped at --max-pages; run again or raise it for older likes)"
        } else {
            ""
        }
    );
    Ok(())
}

#[derive(Debug, Default, PartialEq)]
pub struct IngestSummary {
    pub pages: u32,
    pub new_posts: usize,
    pub known_posts: usize,
    pub uploaded: usize,
    pub duplicates: usize,
    pub failed: usize,
    pub hit_page_limit: bool,
}

/// Walk liked posts newest-first and import the ones not in the DB yet.
/// `r2 = None` is a dry run: nothing is downloaded or written.
pub async fn ingest(
    adapter: &dyn SourceAdapter,
    db: &Db,
    r2: Option<&R2Client>,
    options: &IngestOptions,
) -> Result<IngestSummary> {
    let mut summary = IngestSummary::default();
    let mut cursor: Option<String> = None;

    loop {
        if options.max_pages != 0 && summary.pages >= options.max_pages {
            summary.hit_page_limit = true;
            break;
        }
        let page = adapter.fetch_liked_page(cursor.as_deref()).await?;
        summary.pages += 1;

        let (mut with_images, mut known_on_page) = (0usize, 0usize);
        for post in page.posts.iter().filter(|p| !p.image_urls.is_empty()) {
            with_images += 1;
            if db
                .get_post_by_source(adapter.source_type(), &post.source_post_id)
                .await?
                .is_some()
            {
                known_on_page += 1;
                continue;
            }
            summary.new_posts += 1;

            let Some(r2) = r2 else {
                println!(
                    "would_import post={} images={}",
                    post.source_post_url,
                    post.image_urls.len()
                );
                continue;
            };
            for (index, url) in post.image_urls.iter().enumerate() {
                match import_image(adapter, db, r2, post, index, url).await {
                    Ok(ImageOutcome::Uploaded) => summary.uploaded += 1,
                    Ok(ImageOutcome::Duplicate) => summary.duplicates += 1,
                    Err(error) => {
                        summary.failed += 1;
                        eprintln!("image_failed post={} url={url} error={error:#}", post.source_post_url);
                    }
                }
            }
        }
        summary.known_posts += known_on_page;

        // Likes come newest first, so a page of nothing but known posts means
        // we've caught up with the last run.
        let caught_up = with_images > 0 && known_on_page == with_images;
        match page.next_cursor {
            Some(next) if !caught_up || options.full => cursor = Some(next),
            _ => break,
        }
    }

    Ok(summary)
}

enum ImageOutcome {
    Uploaded,
    Duplicate,
}

async fn import_image(
    adapter: &dyn SourceAdapter,
    db: &Db,
    r2: &R2Client,
    post: &SourcePost,
    index: usize,
    url: &str,
) -> Result<ImageOutcome> {
    let (bytes, content_type) = adapter
        .download_image(url)
        .await
        .with_context(|| format!("download {url}"))?;
    let data = bytes.to_vec();
    let (sha256, width, height) = image_metadata(&data);
    let source_type = adapter.source_type();
    let filename = format!("{}_p{index}", post.source_post_id);
    let s3_key_base = R2Client::canonical_key_base(source_type, &filename);

    let author = post
        .author_source_id
        .as_deref()
        .map(|id| GalleryImportAuthorArgs {
            source_author_id: id,
            name: post.author_name.as_deref(),
            url: post.author_url.as_deref(),
            profile_url: None,
        });
    let claimed = db
        .claim_gallery_image_upload(
            &GalleryImportPostArgs {
                source_type,
                source_post_id: &post.source_post_id,
                source_post_url: &post.source_post_url,
                uploaded_at: post.posted_at.as_deref(),
                title: None,
                caption: post.caption.as_deref(),
                raw_json: post.raw_json.as_deref(),
            },
            author.as_ref(),
            &GalleryImportImageArgs {
                sha256: &sha256,
                s3_key_base: &s3_key_base,
                filename: Some(&filename),
                width,
                height,
                file_size: Some(data.len() as i64),
                content_type: &content_type,
                source_url: Some(url),
            },
        )
        .await?;

    let Some(claimed) = claimed.claimed_upload else {
        println!("skipped_duplicate post={} sha256={sha256}", post.source_post_url);
        return Ok(ImageOutcome::Duplicate);
    };

    match r2.upload_with_thumbnail(&s3_key_base, data, &content_type).await {
        Ok(()) => {
            db.update_tag_job_status(claimed.job.id, "uploaded", None)
                .await?;
            println!(
                "uploaded post={} image_id={} key={s3_key_base}",
                post.source_post_url, claimed.asset.id
            );
            Ok(ImageOutcome::Uploaded)
        }
        Err(error) => {
            let message = format!("{error:#}");
            db.update_tag_job_status(claimed.job.id, "failed", Some(&message))
                .await?;
            Err(error.context(format!("upload to {s3_key_base}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use bytes::Bytes;
    use fulgorart_ingestor::LikedPage;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Serves fixed pages: page i has the given post ids, each with one image.
    struct FakeAdapter {
        pages: Vec<Vec<&'static str>>,
        fetched: AtomicU32,
    }

    #[async_trait]
    impl SourceAdapter for FakeAdapter {
        fn source_type(&self) -> &str {
            "fake"
        }

        async fn fetch_liked_page(&self, cursor: Option<&str>) -> Result<LikedPage> {
            self.fetched.fetch_add(1, Ordering::SeqCst);
            let index: usize = cursor.map_or(0, |c| c.parse().unwrap());
            let posts = self.pages[index]
                .iter()
                .map(|id| SourcePost {
                    source_type: "fake".into(),
                    source_post_id: id.to_string(),
                    source_post_url: format!("https://fake/{id}"),
                    liked_at: None,
                    posted_at: None,
                    caption: None,
                    author_name: None,
                    author_source_id: None,
                    author_url: None,
                    image_urls: vec![format!("https://fake/{id}.jpg")],
                    raw_json: None,
                })
                .collect();
            let next_cursor = (index + 1 < self.pages.len()).then(|| (index + 1).to_string());
            Ok(LikedPage { posts, next_cursor })
        }

        async fn download_image(&self, _url: &str) -> Result<(Bytes, String)> {
            unreachable!("dry run must not download")
        }
    }

    async fn db_with_known_post(id: &str) -> Db {
        let path = std::env::temp_dir().join(format!(
            "fulgorart-ingest-test-{}-{}.db",
            std::process::id(),
            rand_suffix()
        ));
        let db = Db::connect(&path).await.unwrap();
        db.insert_post(
            &GalleryImportPostArgs {
                source_type: "fake",
                source_post_id: id,
                source_post_url: "https://fake/known",
                uploaded_at: None,
                title: None,
                caption: None,
                raw_json: None,
            },
            None,
        )
        .await
        .unwrap();
        db
    }

    fn rand_suffix() -> u32 {
        static N: AtomicU32 = AtomicU32::new(0);
        N.fetch_add(1, Ordering::SeqCst)
    }

    fn options(max_pages: u32, full: bool) -> IngestOptions {
        IngestOptions {
            max_pages,
            full,
            dry_run: true,
        }
    }

    fn adapter() -> FakeAdapter {
        // page 2 is entirely already imported; page 3 is an older gap
        FakeAdapter {
            pages: vec![vec!["a", "b"], vec!["known"], vec!["d"]],
            fetched: AtomicU32::new(0),
        }
    }

    #[tokio::test]
    async fn stops_at_first_fully_known_page() {
        let (adapter, db) = (adapter(), db_with_known_post("known").await);
        let summary = ingest(&adapter, &db, None, &options(10, false)).await.unwrap();
        assert_eq!(adapter.fetched.load(Ordering::SeqCst), 2);
        assert_eq!((summary.new_posts, summary.known_posts), (2, 1));
        assert!(!summary.hit_page_limit);
    }

    #[tokio::test]
    async fn full_keeps_paging_past_known_posts() {
        let (adapter, db) = (adapter(), db_with_known_post("known").await);
        let summary = ingest(&adapter, &db, None, &options(10, true)).await.unwrap();
        assert_eq!(summary.pages, 3);
        assert_eq!((summary.new_posts, summary.known_posts), (3, 1));
    }

    #[tokio::test]
    async fn respects_max_pages() {
        let (adapter, db) = (adapter(), db_with_known_post("known").await);
        let summary = ingest(&adapter, &db, None, &options(1, true)).await.unwrap();
        assert_eq!(adapter.fetched.load(Ordering::SeqCst), 1);
        assert_eq!(summary.new_posts, 2);
        assert!(summary.hit_page_limit);
    }
}
