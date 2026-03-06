use anyhow::Result;
use clap::Args;
use mm_config::AppConfig;
use mm_copyright::estimate as estimate_copyright;
use mm_db::{models::Release, queries, Db};
use mm_discogs::DiscogsClient;
use mm_platforms::PlatformCoordinator;

use uuid::Uuid;

#[derive(Args)]
pub struct SearchArgs {
    /// Override country (e.g. "Germany")
    #[arg(long)]
    country: Option<String>,

    /// Override country code (e.g. "DE")
    #[arg(long)]
    country_code: Option<String>,

    /// Only save releases NOT found on any platform
    #[arg(long, default_value = "false")]
    missing_only: bool,

    /// Skip platform availability checks (faster, discovery only)
    #[arg(long, default_value = "false")]
    no_check: bool,

    /// Max pages to fetch from Discogs
    #[arg(long)]
    max_pages: Option<u32>,

    /// Auto-add missing releases to watchlist
    #[arg(long, default_value = "false")]
    auto_watchlist: bool,
}

pub async fn run(args: &SearchArgs, cfg: &AppConfig, pool: &Db) -> Result<()> {
    // Build effective config (apply CLI overrides)
    let mut cfg = cfg.clone();
    if let Some(c) = &args.country {
        cfg.search.country = c.clone();
    }
    if let Some(cc) = &args.country_code {
        cfg.search.country_code = cc.clone();
    }
    if let Some(mp) = args.max_pages {
        cfg.search.max_pages = mp;
    }

    println!(
        "Searching Discogs for releases from {} ({})...",
        cfg.search.country, cfg.search.country_code
    );

    let discogs = DiscogsClient::new(&cfg)?;
    let releases = discogs.search_releases(&cfg).await?;

    println!("Found {} releases on Discogs.", releases.len());

    let coordinator = if !args.no_check {
        Some(PlatformCoordinator::new(&cfg).await?)
    } else {
        None
    };

    let mut saved = 0usize;
    let mut missing = 0usize;

    for dr in &releases {
        let (artist, title) = dr.split_artist_title();
        let year = dr.year_as_i32();

        // Estimate copyright
        let (copyright_status, copyright_note) =
            estimate_copyright(year, &cfg.copyright);

        // Build DB model
        let release = Release {
            id: Uuid::new_v4(),
            discogs_id: dr.id as i32,
            title: title.clone(),
            artists: if artist.is_empty() {
                vec!["Unknown".to_owned()]
            } else {
                vec![artist.clone()]
            },
            label: dr.primary_label(),
            catalog_number: dr.catno.clone(),
            country: cfg.search.country.clone(),
            country_code: cfg.search.country_code.clone(),
            year,
            genres: dr.genres_vec(),
            styles: dr.styles_vec(),
            formats: dr.formats_vec(),
            discogs_url: dr.discogs_url(),
            thumb_url: dr.thumb.clone(),
            discovery_job_id: None,
            musicbrainz_id: None,
            copyright_status: copyright_status.to_string(),
            copyright_note: Some(copyright_note),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let release_id = queries::upsert_release(pool, &release).await?;
        saved += 1;

        // Platform availability check
        if let Some(coord) = &coordinator {
            let results = coord
                .check_all(&artist, &title, cfg.platforms.match_threshold)
                .await;

            let any_found = results.iter().any(|r| r.found);

            for result in &results {
                let check = mm_db::models::PlatformCheck {
                    id: Uuid::new_v4(),
                    release_id,
                    platform: result.platform.clone(),
                    found: result.found,
                    match_score: result.match_result.as_ref().map(|m| m.score),
                    platform_url: result
                        .match_result
                        .as_ref()
                        .and_then(|m| m.platform_url.clone()),
                    checked_at: chrono::Utc::now(),
                };
                queries::upsert_platform_check(pool, &check).await?;
            }

            if !any_found {
                missing += 1;

                let platforms: Vec<&str> = results.iter().map(|r| r.platform.as_str()).collect();
                println!(
                    "  [MISSING] {:?} - {} ({}) | {} | not on: {}",
                    artist,
                    title,
                    year.map(|y| y.to_string()).unwrap_or_else(|| "?".to_owned()),
                    copyright_status,
                    platforms.join(", ")
                );

                if args.auto_watchlist {
                    let buy_url = DiscogsClient::buy_url(dr.id);
                    queries::add_to_watchlist(pool, release_id, Some(&buy_url)).await?;
                    println!("    → Added to watchlist: {buy_url}");
                }
            } else if !args.missing_only {
                let found_on: Vec<String> = results
                    .iter()
                    .filter(|r| r.found)
                    .map(|r| r.platform.clone())
                    .collect();
                println!(
                    "  [FOUND]   {} - {} ({}) | found on: {}",
                    artist,
                    title,
                    year.map(|y| y.to_string()).unwrap_or_else(|| "?".to_owned()),
                    found_on.join(", ")
                );
            }
        } else {
            println!(
                "  {} - {} ({}) | {}",
                artist,
                title,
                year.map(|y| y.to_string()).unwrap_or_else(|| "?".to_owned()),
                copyright_status
            );
        }
    }

    println!();
    println!("Summary:");
    println!("  Total releases saved : {saved}");
    if !args.no_check {
        println!("  Not on any platform  : {missing}");
    }

    Ok(())
}
