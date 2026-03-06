use anyhow::Result;
use clap::{Args, Subcommand};
use mm_config::AppConfig;
use mm_db::{models::WatchlistStatus, queries, Db};
use mm_watcher::get_watchlist_summary;
use uuid::Uuid;

#[derive(Subcommand)]
pub enum WatchlistCommands {
    /// List watchlist entries
    List(ListArgs),
    /// Add a release to watchlist by Discogs ID
    Add(AddArgs),
    /// Update watchlist item status
    Status(StatusArgs),
    /// Remove a watchlist entry
    Remove(RemoveArgs),
}

#[derive(Args)]
pub struct ListArgs {
    /// Filter by status: to_buy, ordered, purchased, ready_to_rip, ripping, done, skipped
    #[arg(long)]
    status: Option<String>,

    /// Show only items with PUBLIC_DOMAIN copyright
    #[arg(long, default_value = "false")]
    public_domain_only: bool,
}

#[derive(Args)]
pub struct AddArgs {
    /// Discogs release ID
    #[arg(long)]
    discogs_id: i32,

    /// Optional notes
    #[arg(long)]
    notes: Option<String>,
}

#[derive(Args)]
pub struct StatusArgs {
    /// Watchlist entry UUID
    id: Uuid,

    /// New status: to_buy, ordered, purchased, ready_to_rip, done, skipped
    status: String,
}

#[derive(Args)]
pub struct RemoveArgs {
    /// Watchlist entry UUID
    id: Uuid,
}

pub async fn run(cmd: &WatchlistCommands, cfg: &AppConfig, pool: &Db) -> Result<()> {
    match cmd {
        WatchlistCommands::List(args) => {
            let items = get_watchlist_summary(pool).await?;

            let filtered: Vec<_> = items
                .iter()
                .filter(|i| {
                    args.status.as_ref().map_or(true, |s| i.status.to_string() == *s)
                })
                .filter(|i| {
                    !args.public_domain_only
                        || i.copyright_status.contains("PUBLIC_DOMAIN")
                })
                .collect();

            if filtered.is_empty() {
                println!("No watchlist entries found.");
                return Ok(());
            }

            println!(
                "{:<36}  {:<12}  {:<30}  {:<40}  {:<6}  {}",
                "ID", "Status", "Artist", "Title", "Year", "Copyright"
            );
            println!("{}", "─".repeat(140));

            for item in &filtered {
                println!(
                    "{:<36}  {:<12}  {:<30}  {:<40}  {:<6}  {}",
                    item.watchlist_id,
                    item.status.to_string(),
                    truncate(&item.artist, 30),
                    truncate(&item.title, 40),
                    item.year.map(|y| y.to_string()).unwrap_or_else(|| "?".to_owned()),
                    item.copyright_status,
                );
            }

            println!();
            println!("Total: {} entries", filtered.len());
        }

        WatchlistCommands::Add(args) => {
            let id = mm_watcher::add_release_to_watchlist(
                pool,
                args.discogs_id,
                args.notes.as_deref(),
                cfg,
            )
            .await?;
            println!("Added to watchlist with ID: {id}");
        }

        WatchlistCommands::Status(args) => {
            let status = WatchlistStatus::from(args.status.clone());
            queries::update_watchlist_status(pool, args.id, status).await?;
            println!("Updated watchlist {} → {}", args.id, args.status);
        }

        WatchlistCommands::Remove(args) => {
            sqlx::query("DELETE FROM watchlist WHERE id = $1")
                .bind(args.id)
                .execute(pool)
                .await?;
            println!("Removed watchlist entry {}", args.id);
        }
    }

    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_owned()
    } else {
        format!("{}…", &s[..max - 1])
    }
}
