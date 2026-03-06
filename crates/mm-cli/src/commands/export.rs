use anyhow::Result;
use clap::Args;
use mm_config::AppConfig;
use mm_db::Db;
use mm_watcher::export_watchlist_csv;
use std::path::PathBuf;

#[derive(Args)]
pub struct ExportArgs {
    /// What to export: watchlist, missing, all
    #[arg(long, default_value = "missing")]
    what: String,

    /// Output format: csv (json planned)
    #[arg(long, default_value = "csv")]
    format: String,

    /// Output file path (default: stdout)
    #[arg(long)]
    output: Option<PathBuf>,
}

pub async fn run(args: &ExportArgs, cfg: &AppConfig, pool: &Db) -> Result<()> {
    let content = match args.what.as_str() {
        "watchlist" => export_watchlist_csv(pool).await?,
        "missing" => export_missing_csv(pool, &cfg.search.country_code).await?,
        "all" => export_all_releases_csv(pool).await?,
        other => anyhow::bail!("Unknown export target: {other}. Use: watchlist, missing, all"),
    };

    match &args.output {
        Some(path) => {
            std::fs::write(path, &content)?;
            println!("Exported to {}", path.display());
        }
        None => print!("{content}"),
    }

    Ok(())
}

async fn export_missing_csv(pool: &Db, country_code: &str) -> Result<String> {
    #[derive(sqlx::FromRow)]
    struct Row {
        discogs_id: i32,
        artists: Vec<String>,
        title: String,
        year: Option<i32>,
        label: Option<String>,
        formats: Vec<String>,
        copyright_status: String,
        discogs_url: String,
        platforms_checked: Option<i64>,
    }

    let rows = sqlx::query_as::<_, Row>(
        r#"
        SELECT
            r.discogs_id,
            r.artists,
            r.title,
            r.year,
            r.label,
            r.formats,
            r.copyright_status,
            r.discogs_url,
            COUNT(pc.id) FILTER (WHERE pc.found = false) AS platforms_checked
        FROM releases r
        LEFT JOIN platform_checks pc ON pc.release_id = r.id
        WHERE r.country_code = $1
        GROUP BY r.id
        HAVING COUNT(pc.id) FILTER (WHERE pc.found = true) = 0
           AND COUNT(pc.id) > 0
        ORDER BY r.artists[1], r.title
        "#,
    )
    .bind(country_code)
    .fetch_all(pool)
    .await?;

    let mut csv = String::from(
        "discogs_id,artist,title,year,label,formats,copyright_status,discogs_url,platforms_checked\n",
    );

    for row in &rows {
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{},{}\n",
            row.discogs_id,
            csv_escape(row.artists.first().map(String::as_str).unwrap_or("")),
            csv_escape(&row.title),
            row.year.map(|y| y.to_string()).unwrap_or_default(),
            csv_escape(row.label.as_deref().unwrap_or("")),
            csv_escape(&row.formats.join(";")),
            row.copyright_status,
            row.discogs_url,
            row.platforms_checked.unwrap_or(0),
        ));
    }

    Ok(csv)
}

async fn export_all_releases_csv(pool: &Db) -> Result<String> {
    #[derive(sqlx::FromRow)]
    struct Row {
        discogs_id: i32,
        artists: Vec<String>,
        title: String,
        year: Option<i32>,
        label: Option<String>,
        country_code: String,
        copyright_status: String,
        discogs_url: String,
    }

    let rows = sqlx::query_as::<_, Row>(
        "SELECT discogs_id, artists, title, year, label, country_code, copyright_status, discogs_url \
         FROM releases ORDER BY country_code, artists[1], title",
    )
    .fetch_all(pool)
    .await?;

    let mut csv = String::from(
        "discogs_id,artist,title,year,label,country_code,copyright_status,discogs_url\n",
    );

    for row in &rows {
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            row.discogs_id,
            csv_escape(row.artists.first().map(String::as_str).unwrap_or("")),
            csv_escape(&row.title),
            row.year.map(|y| y.to_string()).unwrap_or_default(),
            csv_escape(row.label.as_deref().unwrap_or("")),
            row.country_code,
            row.copyright_status,
            row.discogs_url,
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
