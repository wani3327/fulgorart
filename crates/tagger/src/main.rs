use anyhow::Result;

fn print_usage() {
    eprintln!("Usage:");
    eprintln!("  fulgorart-tagger                 # process pending jobs from DB queue");
    eprintln!("  fulgorart-tagger ./image.jpg     # process one local image file");
    eprintln!("  fulgorart-tagger a.jpg b.png     # process multiple local files");
}

fn parse_paths_from_args() -> Result<Option<Vec<String>>> {
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().is_none() {
        return Ok(None);
    }

    let mut paths = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other if other.starts_with('-') => {
                anyhow::bail!("Unknown option: {}", other);
            }
            path => {
                paths.push(path.to_string());
            }
        }
    }

    if paths.is_empty() {
        Ok(None)
    } else {
        Ok(Some(paths))
    }
}

/// Process every pending tag job in the database, then exit.
/// Intended to be invoked by cron (e.g. every minute).
#[tokio::main]
async fn main() -> Result<()> {
    match parse_paths_from_args()? {
        Some(paths) => {
            fulgorart_tagger::run_for_paths(&paths).await?;
        }
        None => {
            fulgorart_tagger::run().await?;
        }
    }
    Ok(())
}
