use anyhow::{Context, Result};
use bytes::Bytes;
use fulgorart_tagger::{TagPrediction, Wd14Tagger};
use serde::Serialize;

fn print_usage() {
    eprintln!("Usage:");
    eprintln!("  fulgorart-tagger ./image.jpg                  # process one local image file");
    eprintln!("  fulgorart-tagger a.jpg b.png                  # process multiple local files");
    eprintln!("  fulgorart-tagger https://example.com/img.jpg  # download and tag an image URL");
    eprintln!("  fulgorart-tagger <url1> <url2>                # process multiple URLs");
}

fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

enum CliMode {
    LocalPaths(Vec<String>),
    Urls(Vec<String>),
}

#[derive(Serialize)]
struct TagResult {
    key: String,
    tags: Vec<TagPrediction>,
}

#[cfg(not(feature = "gcp"))]
fn init_tracing() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .try_init()
        .ok();
}

#[cfg(feature = "gcp")]
fn init_tracing() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_stackdriver::layer())
        .try_init()
        .ok();
}

#[cfg(not(feature = "gcp"))]
fn log_tag_result(result: &TagResult) -> Result<()> {
    tracing::info!("{}", serde_json::to_string(result)?);
    Ok(())
}

#[cfg(feature = "gcp")]
fn log_tag_result(result: &TagResult) -> Result<()> {
    tracing::info!(
        severity = %tracing_stackdriver::LogSeverity::Notice,
        "{}",
        serde_json::to_string(result)?
    );
    Ok(())
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
    let all_paths = items.iter().all(|s| !is_url(s));

    if all_urls {
        Ok(CliMode::Urls(items))
    } else if all_paths {
        Ok(CliMode::LocalPaths(items))
    } else {
        anyhow::bail!("Cannot mix local file paths and URLs in the same invocation");
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

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();

    let tagger = Wd14Tagger::from_env()?;
    let res = match parse_args()? {
        CliMode::LocalPaths(paths) => process_paths(&tagger, &paths).await?,
        CliMode::Urls(urls) => process_urls(&tagger, &urls).await?,
    };

    let n = res.len();
    for r in res {
        log_tag_result(&r)?;
    }

    tracing::info!(processed = n, "Tagger processed file(s)");
    Ok(())
}
