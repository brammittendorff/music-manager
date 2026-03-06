use anyhow::Result;
use clap::Args;
use mm_config::AppConfig;
use mm_db::{models::PlatformCheck, queries, Db};
use mm_platforms::PlatformCoordinator;
use uuid::Uuid;

#[derive(Args)]
pub struct CheckArgs {
    /// Discogs release ID to check
    #[arg(long)]
    discogs_id: Option<i32>,

    /// Re-check all releases in the database
    #[arg(long, default_value = "false")]
    all: bool,

    /// Only re-check releases not yet checked
    #[arg(long, default_value = "false")]
    unchecked_only: bool,
}

pub async fn run(args: &CheckArgs, cfg: &AppConfig, pool: &Db) -> Result<()> {
    let coordinator = PlatformCoordinator::new(cfg).await?;

    if let Some(discogs_id) = args.discogs_id {
        // Check a single release
        let release = queries::get_release_by_discogs_id(pool, discogs_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Release {discogs_id} not found - run search first"))?;

        let artist = release.primary_artist().to_owned();
        println!("Checking: {} - {}", artist, release.title);

        let results = coordinator
            .check_all(&artist, &release.title, cfg.platforms.match_threshold)
            .await;

        for r in &results {
            let icon = if r.found { "✓" } else { "✗" };
            let score = r
                .match_result
                .as_ref()
                .map(|m| format!(" ({:.2})", m.score))
                .unwrap_or_default();
            let url = r
                .match_result
                .as_ref()
                .and_then(|m| m.platform_url.as_deref())
                .unwrap_or("");
            println!("  {icon} {:<15} {score} {url}", r.platform);

            // Persist result
            let check = PlatformCheck {
                id: Uuid::new_v4(),
                release_id: release.id,
                platform: r.platform.clone(),
                found: r.found,
                match_score: r.match_result.as_ref().map(|m| m.score),
                platform_url: r
                    .match_result
                    .as_ref()
                    .and_then(|m| m.platform_url.clone()),
                checked_at: chrono::Utc::now(),
            };
            queries::upsert_platform_check(pool, &check).await?;
        }
    } else if args.all {
        // Check all releases in DB
        let releases = sqlx::query_as::<_, mm_db::models::Release>(
            "SELECT * FROM releases ORDER BY artists[1], title",
        )
        .fetch_all(pool)
        .await?;

        println!("Checking {} releases across all platforms...", releases.len());

        for release in &releases {
            let artist = release.primary_artist().to_owned();
            let results = coordinator
                .check_all(&artist, &release.title, cfg.platforms.match_threshold)
                .await;

            for r in &results {
                let check = PlatformCheck {
                    id: Uuid::new_v4(),
                    release_id: release.id,
                    platform: r.platform.clone(),
                    found: r.found,
                    match_score: r.match_result.as_ref().map(|m| m.score),
                    platform_url: r
                        .match_result
                        .as_ref()
                        .and_then(|m| m.platform_url.clone()),
                    checked_at: chrono::Utc::now(),
                };
                queries::upsert_platform_check(pool, &check).await?;
            }

            let found_on: Vec<_> = results.iter().filter(|r| r.found).map(|r| r.platform.as_str()).collect();
            if found_on.is_empty() {
                println!("  [MISSING] {} - {}", artist, release.title);
            } else {
                println!("  [FOUND]   {} - {} ({})", artist, release.title, found_on.join(", "));
            }
        }
    } else {
        println!("Specify --discogs-id <ID> or --all");
    }

    Ok(())
}
