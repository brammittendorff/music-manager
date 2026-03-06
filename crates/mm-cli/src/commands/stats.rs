use anyhow::Result;
use mm_db::Db;

pub async fn run(pool: &Db) -> Result<()> {
    let total_releases: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM releases")
            .fetch_one(pool).await?;

    let public_domain: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM releases WHERE copyright_status = 'PUBLIC_DOMAIN'")
            .fetch_one(pool).await?;

    let releases_found: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT release_id) FROM platform_checks WHERE found = true",
    )
    .fetch_one(pool).await?;

    let releases_not_found: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM (
            SELECT release_id FROM platform_checks
            GROUP BY release_id
            HAVING bool_and(NOT found)
        ) sub
        "#,
    )
    .fetch_one(pool).await?;

    let watchlist_total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM watchlist")
            .fetch_one(pool).await?;

    let watchlist_to_buy: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM watchlist WHERE status = 'to_buy'")
            .fetch_one(pool).await?;

    let watchlist_done: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM watchlist WHERE status = 'done'")
            .fetch_one(pool).await?;

    let rip_jobs_total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM rip_jobs")
            .fetch_one(pool).await?;

    let rip_jobs_done: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM rip_jobs WHERE status = 'done'")
            .fetch_one(pool).await?;

    let tracks_total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tracks")
            .fetch_one(pool).await?;

    println!("=== music-manager stats ===");
    println!();
    println!("Discovery:");
    println!("  Releases in DB             : {total_releases}");
    println!("  Public domain              : {public_domain}");
    println!("  Found on streaming         : {releases_found}");
    println!("  NOT on any platform        : {releases_not_found}");
    println!();
    println!("Watchlist:");
    println!("  Total                      : {watchlist_total}");
    println!("  To buy                     : {watchlist_to_buy}");
    println!("  Digitized (done)           : {watchlist_done}");
    println!();
    println!("Ripping:");
    println!("  Rip jobs                   : {rip_jobs_total}");
    println!("  Completed jobs             : {rip_jobs_done}");
    println!("  Tracks digitized           : {tracks_total}");

    Ok(())
}
