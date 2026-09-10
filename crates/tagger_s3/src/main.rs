use anyhow::Result;
use fulgorart_storage::{R2Client, R2Config};
use fulgorart_tagger::{TagPrediction, Wd14Tagger};
use serde::Serialize;

#[derive(Serialize)]
struct TagResult {
    key: String,
    tags: Vec<TagPrediction>,
}

fn print_usage() {
    eprintln!("Usage:");
    eprintln!("  fulgorart-tagger-s3 r2://images/photo.jpg");
    eprintln!("  fulgorart-tagger-s3 r2://<key1> r2://<key2>");
}

fn is_r2_key(s: &str) -> bool {
    s.starts_with("r2://")
}

fn parse_args() -> Result<Vec<String>> {
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().is_none() {
        print_usage();
        std::process::exit(1);
    }

    let mut keys: Vec<String> = Vec::new();
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
                if !is_r2_key(item) {
                    anyhow::bail!("Only r2:// keys are supported, got '{}'", item);
                }
                keys.push(item.to_string());
            }
        }
    }

    if keys.is_empty() {
        print_usage();
        std::process::exit(1);
    }

    Ok(keys)
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

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();

    let keys = parse_args()?;
    let tagger = Wd14Tagger::from_env()?;
    let r2_config = r2_config_from_env()?;
    let r2 = R2Client::new(&r2_config).await?;

    let mut results = Vec::with_capacity(keys.len());
    for raw_key in keys {
        let key = raw_key.strip_prefix("r2://").unwrap_or(raw_key.as_str());
        tracing::debug!(%key, bucket = r2.bucket(), "Fetching image from R2");
        let bytes = r2.download(key).await?;
        let tags = tagger.tag(&bytes)?;
        results.push(TagResult {
            key: key.to_string(),
            tags,
        });
    }

    let processed = results.len();
    for result in &results {
        log_tag_result(result)?;
    }
    tracing::info!(processed, "Tagger processed file(s)");
    Ok(())
}
