use anyhow::Result;
use std::path::PathBuf;

fn output_dir_from_args(default_output_dir: &str) -> PathBuf {
    let mut args = std::env::args().skip(1);
    if let Some(arg) = args.next() {
        return PathBuf::from(arg);
    }
    PathBuf::from(default_output_dir)
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = fulgorart_ingestor::IngestorConfig::from_env();
    let output_dir = output_dir_from_args(&config.default_output_dir);
    let saved = fulgorart_ingestor::run_to_directory(&config, &output_dir).await?;
    tracing::info!(saved, output_dir = %output_dir.display(), "Ingestor finished");
    Ok(())
}
