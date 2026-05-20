use anyhow::Result;

fn install_rustls_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    install_rustls_crypto_provider();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = fulgorart_bridge::BridgeConfig::from_env()?;
    let service = fulgorart_bridge::BridgeService::new(config).await?;
    service.run_once().await?;
    Ok(())
}
