use anyhow::Result;
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{PlatformCheck, Release, RipJob, Track, WatchlistEntry, WatchlistStatus};

// ─── Releases ─────────────────────────────────────────────────────────────────

pub async fn upsert_release(pool: &PgPool, r: &Release) -> Result<Uuid> {
    let id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO releases (
            id, discogs_id, title, artists, label, catalog_number,
            country, country_code, year, genres, styles, formats,
            discogs_url, thumb_url, musicbrainz_id, copyright_status, copyright_note
        ) VALUES (
            $1, $2, $3, $4, $5, $6,
            $7, $8, $9, $10, $11, $12,
            $13, $14, $15, $16, $17
        )
        ON CONFLICT (discogs_id) DO UPDATE SET
            title            = EXCLUDED.title,
            artists          = EXCLUDED.artists,
            label            = EXCLUDED.label,
            catalog_number   = EXCLUDED.catalog_number,
            year             = EXCLUDED.year,
            genres           = EXCLUDED.genres,
            styles           = EXCLUDED.styles,
            formats          = EXCLUDED.formats,
            thumb_url        = EXCLUDED.thumb_url,
            musicbrainz_id   = COALESCE(EXCLUDED.musicbrainz_id, releases.musicbrainz_id),
            copyright_status = EXCLUDED.copyright_status,
            copyright_note   = EXCLUDED.copyright_note,
            updated_at       = now()
        RETURNING id
        "#,
    )
    .bind(r.id)
    .bind(r.discogs_id)
    .bind(&r.title)
    .bind(&r.artists)
    .bind(&r.label)
    .bind(&r.catalog_number)
    .bind(&r.country)
    .bind(&r.country_code)
    .bind(r.year)
    .bind(&r.genres)
    .bind(&r.styles)
    .bind(&r.formats)
    .bind(&r.discogs_url)
    .bind(&r.thumb_url)
    .bind(r.musicbrainz_id)
    .bind(&r.copyright_status)
    .bind(&r.copyright_note)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

pub async fn get_release_by_discogs_id(pool: &PgPool, discogs_id: i32) -> Result<Option<Release>> {
    let r = sqlx::query_as::<_, Release>(
        "SELECT * FROM releases WHERE discogs_id = $1",
    )
    .bind(discogs_id)
    .fetch_optional(pool)
    .await?;
    Ok(r)
}

pub async fn list_releases_not_on_platforms(
    pool: &PgPool,
    country_code: &str,
    platforms: &[&str],
) -> Result<Vec<Release>> {
    let releases = sqlx::query_as::<_, Release>(
        r#"
        SELECT r.*
        FROM releases r
        WHERE r.country_code = $1
          AND NOT EXISTS (
              SELECT 1 FROM platform_checks pc
              WHERE pc.release_id = r.id
                AND pc.platform = ANY($2)
                AND pc.found = true
          )
        ORDER BY r.year, r.artists[1], r.title
        "#,
    )
    .bind(country_code)
    .bind(platforms)
    .fetch_all(pool)
    .await?;
    Ok(releases)
}

// ─── Platform checks ──────────────────────────────────────────────────────────

pub async fn upsert_platform_check(pool: &PgPool, check: &PlatformCheck) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO platform_checks (id, release_id, platform, found, match_score, platform_url)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (release_id, platform) DO UPDATE SET
            found        = EXCLUDED.found,
            match_score  = EXCLUDED.match_score,
            platform_url = EXCLUDED.platform_url,
            checked_at   = now()
        "#,
    )
    .bind(check.id)
    .bind(check.release_id)
    .bind(&check.platform)
    .bind(check.found)
    .bind(check.match_score)
    .bind(&check.platform_url)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_checks_for_release(
    pool: &PgPool,
    release_id: Uuid,
) -> Result<Vec<PlatformCheck>> {
    let checks = sqlx::query_as::<_, PlatformCheck>(
        "SELECT * FROM platform_checks WHERE release_id = $1 ORDER BY platform",
    )
    .bind(release_id)
    .fetch_all(pool)
    .await?;
    Ok(checks)
}

// ─── Watchlist ────────────────────────────────────────────────────────────────

pub async fn add_to_watchlist(
    pool: &PgPool,
    release_id: Uuid,
    buy_url: Option<&str>,
) -> Result<Uuid> {
    let id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO watchlist (release_id, buy_url)
        VALUES ($1, $2)
        ON CONFLICT (release_id) DO UPDATE SET updated_at = now()
        RETURNING id
        "#,
    )
    .bind(release_id)
    .bind(buy_url)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

pub async fn update_watchlist_status(
    pool: &PgPool,
    id: Uuid,
    status: WatchlistStatus,
) -> Result<()> {
    sqlx::query(
        "UPDATE watchlist SET status = $1, updated_at = now() WHERE id = $2",
    )
    .bind(status.to_string())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_watchlist(
    pool: &PgPool,
    status_filter: Option<&str>,
) -> Result<Vec<WatchlistEntry>> {
    let entries = match status_filter {
        Some(s) => {
            sqlx::query_as::<_, WatchlistEntry>(
                "SELECT * FROM watchlist WHERE status = $1 ORDER BY added_at DESC",
            )
            .bind(s)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query_as::<_, WatchlistEntry>(
                "SELECT * FROM watchlist ORDER BY added_at DESC",
            )
            .fetch_all(pool)
            .await?
        }
    };
    Ok(entries)
}

// ─── Rip jobs ─────────────────────────────────────────────────────────────────

pub async fn create_rip_job(pool: &PgPool, job: &RipJob) -> Result<Uuid> {
    let id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO rip_jobs (
            id, watchlist_id, release_id, disc_id, musicbrainz_id,
            status, drive_path, backend, temp_dir, output_dir, track_count
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
        RETURNING id
        "#,
    )
    .bind(job.id)
    .bind(job.watchlist_id)
    .bind(job.release_id)
    .bind(&job.disc_id)
    .bind(job.musicbrainz_id)
    .bind(&job.status)
    .bind(&job.drive_path)
    .bind(&job.backend)
    .bind(&job.temp_dir)
    .bind(&job.output_dir)
    .bind(job.track_count)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

pub async fn update_rip_job_status(
    pool: &PgPool,
    id: Uuid,
    status: &str,
    error_msg: Option<&str>,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE rip_jobs
        SET status = $1,
            error_msg = $2,
            finished_at = CASE WHEN $1 IN ('done','failed') THEN now() ELSE NULL END
        WHERE id = $3
        "#,
    )
    .bind(status)
    .bind(error_msg)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn insert_track(pool: &PgPool, t: &Track) -> Result<Uuid> {
    let id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO tracks (
            id, rip_job_id, release_id, track_number, title, artist, album, year,
            file_path, file_format, bitrate_kbps, sample_rate, channels,
            duration_ms, file_size_bytes, accuraterip_v1, accuraterip_v2, accuraterip_ok
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)
        RETURNING id
        "#,
    )
    .bind(t.id)
    .bind(t.rip_job_id)
    .bind(t.release_id)
    .bind(t.track_number)
    .bind(&t.title)
    .bind(&t.artist)
    .bind(&t.album)
    .bind(t.year)
    .bind(&t.file_path)
    .bind(&t.file_format)
    .bind(t.bitrate_kbps)
    .bind(t.sample_rate)
    .bind(t.channels)
    .bind(t.duration_ms)
    .bind(t.file_size_bytes)
    .bind(&t.accuraterip_v1)
    .bind(&t.accuraterip_v2)
    .bind(t.accuraterip_ok)
    .fetch_one(pool)
    .await?;
    Ok(id)
}
