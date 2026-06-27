use anyhow::{Context, Result};
use bytes::Bytes;
use fulgorart_storage::{R2Client, R2Config};
use fulgorart_tagger::{TagPrediction, Wd14Tagger};
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
struct TagResult {
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

async fn process_paths(tagger: &Wd14Tagger, paths: &[String]) -> Result<Vec<TagResult>> {
    let mut res = vec![];
    for path in paths {
        let bytes = tokio::fs::read(path)
            .await
            .with_context(|| format!("Failed to read image file: {}", path))?;
        let tags = tagger.tag(&bytes)?;
        res.push(TagResult {
            key: path.clone(),
            tags,
        });
    }
    Ok(res)
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

async fn process_urls(tagger: &Wd14Tagger, urls: &[String]) -> Result<Vec<TagResult>> {
    let http = reqwest::Client::new();
    let mut res = vec![];
    for url in urls {
        let bytes = download_url(&http, url).await?;
        let tags = tagger.tag(&bytes)?;
        res.push(TagResult {
            key: url.clone(),
            tags,
        });
    }
    Ok(res)
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

async fn process_r2_keys(tagger: &Wd14Tagger, keys: &[String]) -> Result<Vec<TagResult>> {
    let r2_config = r2_config_from_env()?;
    let r2 = R2Client::new(&r2_config).await?;
    let mut res = vec![];
    for raw_key in keys {
        let key = raw_key.strip_prefix("r2://").unwrap_or(raw_key);
        tracing::debug!(%key, bucket = r2.bucket(), "Fetching image from R2");
        let bytes = r2.download(key).await?;
        let tags: Vec<TagPrediction> = tagger.tag(&bytes)?;
        res.push(TagResult {
            key: key.to_string(),
            tags,
        });
    }
    Ok(res)
}

#[tokio::main]
async fn main() -> Result<()> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    
    dotenvy::dotenv().ok();
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_|"info".into()))
        .with(tracing_stackdriver::layer())
        .try_init()
        .ok();

    let tagger = Wd14Tagger::from_env()?;
    let res = match parse_args()? {
        CliMode::LocalPaths(paths) => {
            process_paths(&tagger, &paths).await?
        }
        CliMode::Urls(urls) => {
            process_urls(&tagger, &urls).await?
        }
        CliMode::R2Keys(keys) => {
            process_r2_keys(&tagger, &keys).await?
        }
    };

    let n = res.len();
    for r in res {
        tracing::info!(severity = %tracing_stackdriver::LogSeverity::Notice, "{}", serde_json::to_string(&r)?);
    }

    tracing::info!(processed = n, "Tagger processed file(s)");
    Ok(())
}
