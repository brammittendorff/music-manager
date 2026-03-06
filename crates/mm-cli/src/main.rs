mod commands;

use anyhow::Result;
use clap::{Parser, Subcommand};
use mm_config::AppConfig;
use mm_db::{connect, migrate};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "mm",
    about = "music-manager - discover, buy, rip and preserve Dutch music",
    version
)]
struct Cli {
    /// Config directory (default: ./config)
    #[arg(long, default_value = "config", global = true)]
    config_dir: String,

    /// Log level: trace, debug, info, warn, error
    #[arg(long, default_value = "info", global = true, env = "MMGR_LOG")]
    log: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Search Discogs and check platform availability
    Search(commands::search::SearchArgs),
    /// Platform availability commands
    Check(commands::check::CheckArgs),
    /// Watchlist management
    #[command(subcommand)]
    Watchlist(commands::watchlist::WatchlistCommands),
    /// CD ripping
    #[command(subcommand)]
    Rip(commands::rip::RipCommands),
    /// Export data
    Export(commands::export::ExportArgs),
    /// Show database stats
    Stats,
    /// Run database migrations
    Migrate,
    /// Run background platform-check worker
    Worker(commands::worker::WorkerArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(&cli.log)),
        )
        .init();

    // Load config
    let cfg = AppConfig::load_from(&cli.config_dir)?;

    match &cli.command {
        Commands::Migrate => {
            let pool = connect(&cfg).await?;
            migrate(&pool).await?;
            println!("Migrations applied successfully.");
        }

        Commands::Stats => {
            let pool = connect(&cfg).await?;
            commands::stats::run(&pool).await?;
        }

        Commands::Search(args) => {
            let pool = connect(&cfg).await?;
            commands::search::run(args, &cfg, &pool).await?;
        }

        Commands::Check(args) => {
            let pool = connect(&cfg).await?;
            commands::check::run(args, &cfg, &pool).await?;
        }

        Commands::Watchlist(cmd) => {
            let pool = connect(&cfg).await?;
            commands::watchlist::run(cmd, &cfg, &pool).await?;
        }

        Commands::Rip(cmd) => {
            let pool = connect(&cfg).await?;
            commands::rip::run(cmd, &cfg, pool).await?;
        }

        Commands::Export(args) => {
            let pool = connect(&cfg).await?;
            commands::export::run(args, &cfg, &pool).await?;
        }

        Commands::Worker(args) => {
            let pool = connect(&cfg).await?;
            commands::worker::run(args, &cfg, &pool).await?;
        }
    }

    Ok(())
}
