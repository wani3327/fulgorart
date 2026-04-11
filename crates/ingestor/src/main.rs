use anyhow::Result;

/// Fetch liked posts from all configured source adapters, download images, and exit.
/// Intended to be invoked by cron (e.g. every 5 minutes).
#[tokio::main]
async fn main() -> Result<()> {
    fulgorart_ingestor::run().await
}
