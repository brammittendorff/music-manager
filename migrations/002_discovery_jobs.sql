-- Discovery jobs: tracks background Discogs search progress.
-- One active job at a time; completed/failed jobs are kept for history.

CREATE TABLE discovery_jobs (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    status         TEXT        NOT NULL DEFAULT 'running',
    -- running | completed | failed | cancelled
    country_code   TEXT        NOT NULL DEFAULT 'NL',
    current_page   INTEGER     NOT NULL DEFAULT 0,
    total_pages    INTEGER,
    releases_saved INTEGER     NOT NULL DEFAULT 0,
    missing_count  INTEGER     NOT NULL DEFAULT 0,
    started_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at    TIMESTAMPTZ,
    error_msg      TEXT
);

CREATE INDEX idx_discovery_jobs_status ON discovery_jobs(status);
