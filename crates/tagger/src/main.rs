use anyhow::Result;

/// Process every pending tag job in the database, then exit.
/// Intended to be invoked by cron (e.g. every minute).
#[tokio::main]
async fn main() -> Result<()> {
    fulgorart_tagger::run().await
}
