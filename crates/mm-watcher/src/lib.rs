//! Watchlist management and buy workflow.

use anyhow::Result;
use mm_db::{models::WatchlistStatus, queries, Db};
use mm_discogs::DiscogsClient;
use mm_config::AppConfig;
use tracing::info;
use uuid::Uuid;

/// Add a release to the watchlist by Discogs release ID.
/// Automatically generates the Discogs buy URL.
pub async fn add_release_to_watchlist(
    pool: &Db,
    discogs_id: i32,
    notes: Option<&str>,
    _cfg: &AppConfig,
) -> Result<Uuid> {
    let release = queries::get_release_by_discogs_id(pool, discogs_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Release {discogs_id} not found in DB - run search first"))?;

    let buy_url = DiscogsClient::buy_url(discogs_id as u32);

    let watchlist_id = queries::add_to_watchlist(pool, release.id, Some(&buy_url)).await?;

    if let Some(n) = notes {
        sqlx::query("UPDATE watchlist SET notes = $1 WHERE id = $2")
            .bind(n)
            .bind(watchlist_id)
            .execute(pool)
            .await?;
    }

    info!(
        "Added to watchlist: {} – {} ({})",
        release.primary_artist(),
        release.title,
        buy_url,
    );

    Ok(watchlist_id)
}

/// Mark a watchlist item as purchased and ready to rip.
pub async fn mark_purchased(pool: &Db, watchlist_id: Uuid) -> Result<()> {
    queries::update_watchlist_status(pool, watchlist_id, WatchlistStatus::Purchased).await?;
    info!("Watchlist {watchlist_id}: marked as purchased");
    Ok(())
}

/// Mark a watchlist item as ready to rip (CD is at desk, ready to insert).
pub async fn mark_ready_to_rip(pool: &Db, watchlist_id: Uuid) -> Result<()> {
    queries::update_watchlist_status(pool, watchlist_id, WatchlistStatus::ReadyToRip).await?;
    info!("Watchlist {watchlist_id}: marked as ready_to_rip");
    Ok(())
}

/// Get a summary of the watchlist with release details joined.
pub struct WatchlistSummary {
    pub watchlist_id: Uuid,
    pub status: WatchlistStatus,
    pub artist: String,
    pub title: String,
    pub year: Option<i32>,
    pub label: Option<String>,
    pub buy_url: Option<String>,
    pub copyright_status: String,
}

pub async fn get_watchlist_summary(pool: &Db) -> Result<Vec<WatchlistSummary>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        watchlist_id: Uuid,
        status: String,
        buy_url: Option<String>,
        title: String,
        artists: Vec<String>,
        year: Option<i32>,
        label: Option<String>,
        copyright_status: String,
    }

    let rows = sqlx::query_as::<_, Row>(
        r#"
        SELECT
            w.id            AS watchlist_id,
            w.status,
            w.buy_url,
            r.title,
            r.artists,
            r.year,
            r.label,
            r.copyright_status
        FROM watchlist w
        JOIN releases r ON r.id = w.release_id
        ORDER BY w.status, r.artists[1], r.title
        "#,
    )
    .fetch_all(pool)
    .await?;

    let summaries = rows
        .into_iter()
        .map(|row| WatchlistSummary {
            watchlist_id: row.watchlist_id,
            status: WatchlistStatus::from(row.status),
            artist: row.artists.into_iter().next().unwrap_or_default(),
            title: row.title,
            year: row.year,
            label: row.label,
            buy_url: row.buy_url,
            copyright_status: row.copyright_status,
        })
        .collect();

    Ok(summaries)
}

/// Export the watchlist as CSV string.
pub async fn export_watchlist_csv(pool: &Db) -> Result<String> {
    let items = get_watchlist_summary(pool).await?;

    let mut csv = String::from(
        "watchlist_id,status,artist,title,year,label,copyright_status,buy_url\n",
    );

    for item in &items {
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            item.watchlist_id,
            item.status,
            csv_escape(&item.artist),
            csv_escape(&item.title),
            item.year.map(|y| y.to_string()).unwrap_or_default(),
            csv_escape(item.label.as_deref().unwrap_or("")),
            item.copyright_status,
            item.buy_url.as_deref().unwrap_or(""),
        ));
    }

    Ok(csv)
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}
