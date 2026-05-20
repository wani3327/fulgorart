use anyhow::Result;

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

    // All arguments must be the same kind (all URLs, all R2 keys, or all local paths).
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

/// Tag images provided as local file paths, URLs, or Cloudflare R2 object keys, then exit.
#[tokio::main]
async fn main() -> Result<()> {
    match parse_args()? {
        CliMode::LocalPaths(paths) => {
            fulgorart_tagger::run_for_paths(&paths).await?;
        }
        CliMode::Urls(urls) => {
            fulgorart_tagger::run_for_urls(&urls).await?;
        }
        CliMode::R2Keys(keys) => {
            fulgorart_tagger::run_for_r2_keys(&keys).await?;
        }
    }
    Ok(())
}
