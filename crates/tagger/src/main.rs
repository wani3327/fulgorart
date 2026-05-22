use anyhow::{Context, Result};
use bytes::Bytes;
use fulgorart_storage::{R2Client, R2Config};
use fulgorart_tagger::{OnnxTagger, Tagger, TagPrediction};
use serde::Serialize;

fn print_usage() {
    eprintln!("Usage:");
    eprintln!("  fulgorart-tagger ./image.jpg                  # process one local image file");
    eprintln!("  fulgorart-tagger a.jpg b.png                  # process multiple local files");
    eprintln!("  fulgorart-tagger https://example.com/img.jpg  # download and tag an image URL");
    eprintln!("  fulgorart-tagger <url1> <url2>                # process multiple URLs");
    eprintln!("  fulgorart-tagger r2://images/photo.jpg        # fetch from Cloudflare R2 bucket");
    eprintln!("  fulgorart-tagger r2://<key1> r2://<key2>      # process multiple R2 object keys");
}

fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

fn is_r2_key(s: &str) -> bool {
    s.starts_with("r2://")
}

enum CliMode {
    LocalPaths(Vec<String>),
    Urls(Vec<String>),
    R2Keys(Vec<String>),
}

#[derive(Serialize)]
struct PathTagResult {
    path: String,
    tags: Vec<TagPrediction>,
}

#[derive(Serialize)]
struct UrlTagResult {
    url: String,
    tags: Vec<TagPrediction>,
}

#[derive(Serialize)]
struct R2TagResult {
    key: String,
    tags: Vec<TagPrediction>,
}

fn parse_args() -> Result<CliMode> {
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().is_none() {
        print_usage();
        std::process::exit(1);
    }

    let mut items: Vec<String> = Vec::new();
    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other if other.starts_with('-') => {
                anyhow::bail!("Unknown option: {}", other);
            }
            item => {
                items.push(item.to_string());
            }
        }
    }

    if items.is_empty() {
        print_usage();
        std::process::exit(1);
    }

    let all_urls = items.iter().all(|s| is_url(s));
    let all_r2 = items.iter().all(|s| is_r2_key(s));
    let all_paths = items.iter().all(|s| !is_url(s) && !is_r2_key(s));

    if all_r2 {
        Ok(CliMode::R2Keys(items))
    } else if all_urls {
        Ok(CliMode::Urls(items))
    } else if all_paths {
        Ok(CliMode::LocalPaths(items))
    } else {
        anyhow::bail!("Cannot mix local file paths, URLs, and R2 keys in the same invocation");
    }
}

async fn process_paths(tagger: &OnnxTagger, paths: &[String]) -> Result<usize> {
    let mut total = 0usize;
    for path in paths {
        let bytes = tokio::fs::read(path)
            .await
            .with_context(|| format!("Failed to read image file: {}", path))?;
        let tags = tagger.tag_image(&bytes).await?;
        println!(
            "{}",
            serde_json::to_string(&PathTagResult {
                path: path.clone(),
                tags,
            })?
        );
        total += 1;
    }
    Ok(total)
}

async fn download_url(http: &reqwest::Client, url: &str) -> Result<Bytes> {
    tracing::debug!(%url, "Downloading image from URL");
    let response = http
        .get(url)
        .send()
        .await
        .context("HTTP request failed")?
        .error_for_status()
        .context("HTTP error status")?;
    response.bytes().await.context("Failed to read image body")
}

async fn process_urls(tagger: &OnnxTagger, urls: &[String]) -> Result<usize> {
    let http = reqwest::Client::new();
    let mut total = 0usize;
    for url in urls {
        let bytes = download_url(&http, url).await?;
        let tags = tagger.tag_image(&bytes).await?;
        println!(
            "{}",
            serde_json::to_string(&UrlTagResult {
                url: url.clone(),
                tags,
            })?
        );
        total += 1;
    }
    Ok(total)
}

fn r2_config_from_env() -> Result<R2Config> {
    let config = R2Config::from_env();
    if config.access_key_id.is_empty() || config.secret_access_key.is_empty() {
        anyhow::bail!(
            "FULGORART_R2_ACCESS_KEY_ID and FULGORART_R2_SECRET_ACCESS_KEY are required for R2 mode"
        );
    }
    Ok(config)
}

async fn process_r2_keys(tagger: &OnnxTagger, keys: &[String]) -> Result<usize> {
    let r2_config = r2_config_from_env()?;
    let r2 = R2Client::new(&r2_config).await?;
    let mut total = 0usize;

    for raw_key in keys {
        let key = raw_key.strip_prefix("r2://").unwrap_or(raw_key);
        tracing::debug!(%key, bucket = r2.bucket(), "Fetching image from R2");
        let bytes = r2.download(key).await?;
        let tags = tagger.tag_image(&bytes).await?;
        println!(
            "{}",
            serde_json::to_string(&R2TagResult {
                key: key.to_string(),
                tags,
            })?
        );
        total += 1;
    }

    Ok(total)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .try_init()
        .ok();

    let tagger = OnnxTagger::from_env()?;
    match parse_args()? {
        CliMode::LocalPaths(paths) => {
            let n = process_paths(&tagger, &paths).await?;
            tracing::info!(processed = n, "Tagger processed local file(s)");
        }
        CliMode::Urls(urls) => {
            let n = process_urls(&tagger, &urls).await?;
            tracing::info!(processed = n, "Tagger processed URL image(s)");
        }
        CliMode::R2Keys(keys) => {
            let n = process_r2_keys(&tagger, &keys).await?;
            tracing::info!(processed = n, "Tagger processed R2 image(s)");
        }
    }
    Ok(())
}
