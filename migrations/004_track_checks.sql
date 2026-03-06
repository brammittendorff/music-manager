-- Per-track platform check results.
-- While platform_checks stores the aggregate per (release, platform),
-- track_checks stores the result for each individual song title checked.

CREATE TABLE track_checks (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    release_id   UUID        NOT NULL REFERENCES releases(id) ON DELETE CASCADE,
    track_title  TEXT        NOT NULL,
    track_number INTEGER,
    platform     TEXT        NOT NULL,
    found        BOOLEAN     NOT NULL,
    match_score  FLOAT,
    platform_url TEXT,
    checked_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (release_id, track_title, platform)
);

CREATE INDEX idx_track_checks_release  ON track_checks(release_id);
CREATE INDEX idx_track_checks_platform ON track_checks(platform, found);
