use anyhow::Result;
use clap::{Parser, Subcommand};
use fulgorart_db::{Db, DbConfig};
use fulgorart_storage::{R2Client, R2Config};

mod gallery_dl_tool;
mod ingestor;
mod remote_tagger_tool;
mod tagger_tool;
mod upload_tool;

#[derive(Parser)]
#[command(name = "fulgorart-cli", about = "FulgorArt command-line tool")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Register a local image into DB + storage and queue tagging
    UploadTool(upload_tool::Args),
    /// Process uploaded tag jobs with the local WD14 tagger
    TaggerTool(tagger_tool::Args),
    /// Import gallery-dl Pixiv JSON and downloaded files into the DB
    GalleryDl(gallery_dl_tool::Args),
    /// Placeholder wrapper for future fulgorart-ingestor orchestration
    Ingestor(ingestor::Args),
    /// Placeholder wrapper for future Cloud Run tagging orchestration
    RemoteTagger,
}

async fn connect_db() -> Result<Db> {
    let db_config = DbConfig::from_env();
    Db::connect(&db_config.path).await
}

async fn connect_r2() -> Result<R2Client> {
    let r2_config = R2Config::from_env();
    R2Client::new(&r2_config).await
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::UploadTool(args) => {
            let db = connect_db().await?;
            let r2 = connect_r2().await?;
            upload_tool::run(args, &db, &r2).await?;
        }
        Commands::TaggerTool(args) => {
            let db = connect_db().await?;
            let r2 = connect_r2().await?;
            tagger_tool::run(args, &db, &r2).await?;
        }
        Commands::GalleryDl(args) => {
            let db = connect_db().await?;
            let r2 = connect_r2().await?;
            gallery_dl_tool::run(args, &db, &r2).await?;
        }
        Commands::Ingestor(args) => ingestor::run(args)?,
        Commands::RemoteTagger => {
            let db = connect_db().await?;
            let job = remote_tagger_tool::CloudRunJob {
                project_id: "".to_string(),
                location: "".to_string(),
                job_name: String::new(),
            };
            remote_tagger_tool::run(db, &job).await?
        }
    }

    Ok(())
}
