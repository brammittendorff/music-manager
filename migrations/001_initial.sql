-- music-manager initial schema
-- Run via: sqlx migrate run

CREATE EXTENSION IF NOT EXISTS "pgcrypto";

-- ─── Releases ─────────────────────────────────────────────────────────────────
-- A release found on Discogs during discovery.
CREATE TABLE releases (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    discogs_id       INTEGER     UNIQUE NOT NULL,
    title            TEXT        NOT NULL,
    artists          TEXT[]      NOT NULL,
    label            TEXT,
    catalog_number   TEXT,
    country          TEXT        NOT NULL,
    country_code     TEXT        NOT NULL,   -- "NL", "DE", …
    year             INTEGER,
    genres           TEXT[]      DEFAULT '{}',
    styles           TEXT[]      DEFAULT '{}',
    formats          TEXT[]      DEFAULT '{}',  -- "Vinyl", "CD", …
    discogs_url      TEXT        NOT NULL,
    thumb_url        TEXT,
    musicbrainz_id   UUID,                       -- resolved MBID (nullable)
    copyright_status TEXT        NOT NULL DEFAULT 'UNKNOWN',
    -- UNKNOWN | PUBLIC_DOMAIN | LIKELY_PUBLIC_DOMAIN | CHECK_REQUIRED | UNDER_COPYRIGHT
    copyright_note   TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_releases_country_code ON releases(country_code);
CREATE INDEX idx_releases_year         ON releases(year);
CREATE INDEX idx_releases_label        ON releases(label);

-- ─── Platform checks ──────────────────────────────────────────────────────────
-- One row per (release, platform) check. Re-checking overwrites via upsert.
CREATE TABLE platform_checks (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    release_id   UUID        NOT NULL REFERENCES releases(id) ON DELETE CASCADE,
    platform     TEXT        NOT NULL,   -- "spotify" | "youtube_music" | …
    found        BOOLEAN     NOT NULL,
    match_score  FLOAT,                  -- Jaro-Winkler 0.0–1.0
    platform_url TEXT,
    checked_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (release_id, platform)
);

CREATE INDEX idx_platform_checks_release ON platform_checks(release_id);
CREATE INDEX idx_platform_checks_found   ON platform_checks(platform, found);

-- ─── Watchlist ────────────────────────────────────────────────────────────────
-- Releases flagged for purchase and digitization.
CREATE TABLE watchlist (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    release_id   UUID        NOT NULL UNIQUE REFERENCES releases(id) ON DELETE CASCADE,
    status       TEXT        NOT NULL DEFAULT 'to_buy',
    -- to_buy → ordered → purchased → ready_to_rip → ripping → done → skipped
    buy_url      TEXT,                   -- Discogs marketplace URL
    price_eur    NUMERIC(8,2),
    seller       TEXT,
    notes        TEXT,
    added_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ─── Rip jobs ─────────────────────────────────────────────────────────────────
-- Triggered when a CD is inserted and matched to a watchlist item.
CREATE TABLE rip_jobs (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    watchlist_id   UUID        REFERENCES watchlist(id),
    release_id     UUID        REFERENCES releases(id),
    disc_id        TEXT,               -- MusicBrainz disc ID (TOC-based)
    musicbrainz_id UUID,
    status         TEXT        NOT NULL DEFAULT 'detected',
    -- detected → ripping → encoding → tagging → done → failed
    drive_path     TEXT        NOT NULL,  -- "/dev/cdrom" or "D:\\"
    backend        TEXT        NOT NULL,  -- "cdparanoia" or "ffmpeg"
    temp_dir       TEXT        NOT NULL,
    output_dir     TEXT,
    track_count    INTEGER,
    error_msg      TEXT,
    accuraterip_status TEXT,             -- "verified" | "partial" | "unverified" | "failed"
    started_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at    TIMESTAMPTZ
);

CREATE INDEX idx_rip_jobs_status ON rip_jobs(status);

-- ─── Tracks ───────────────────────────────────────────────────────────────────
-- Individual digitized tracks produced by a rip job.
CREATE TABLE tracks (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    rip_job_id      UUID        NOT NULL REFERENCES rip_jobs(id) ON DELETE CASCADE,
    release_id      UUID        REFERENCES releases(id),
    track_number    INTEGER     NOT NULL,
    title           TEXT,
    artist          TEXT,
    album           TEXT,
    year            INTEGER,
    file_path       TEXT        NOT NULL,
    file_format     TEXT        NOT NULL,   -- "mp3" | "flac"
    bitrate_kbps    INTEGER,
    sample_rate     INTEGER,
    channels        INTEGER,
    duration_ms     INTEGER,
    file_size_bytes BIGINT,
    accuraterip_v1  TEXT,                   -- checksum
    accuraterip_v2  TEXT,
    accuraterip_ok  BOOLEAN,
    musicbrainz_id  UUID,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_tracks_rip_job  ON tracks(rip_job_id);
CREATE INDEX idx_tracks_release  ON tracks(release_id);

-- ─── Audit log ────────────────────────────────────────────────────────────────
CREATE TABLE audit_log (
    id         BIGSERIAL   PRIMARY KEY,
    table_name TEXT        NOT NULL,
    row_id     UUID        NOT NULL,
    action     TEXT        NOT NULL,   -- INSERT | UPDATE | DELETE
    old_data   JSONB,
    new_data   JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Auto-update updated_at columns
CREATE OR REPLACE FUNCTION touch_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$;

CREATE TRIGGER releases_updated_at  BEFORE UPDATE ON releases  FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER watchlist_updated_at BEFORE UPDATE ON watchlist FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
