use anyhow::Result;

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

fn parse_args() -> Result<CliMode> {
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().is_none() {
        print_usage();
        std::process::exit(1);
    }

    let mut items: Vec<String> = Vec::new();
    while let Some(arg) = args.next() {
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

    // All arguments must be the same kind (all URLs or all local paths).
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

/// Tag images provided as local file paths or URLs, then exit.
/// Pass one or more URLs to tag remote images (Cloud Run Jobs args override mode).
#[tokio::main]
async fn main() -> Result<()> {
    match parse_args()? {
        CliMode::LocalPaths(paths) => {
            fulgorart_tagger::run_for_paths(&paths).await?;
        }
        CliMode::Urls(urls) => {
            fulgorart_tagger::run_for_urls(&urls).await?;
        }
    }
    Ok(())
}
