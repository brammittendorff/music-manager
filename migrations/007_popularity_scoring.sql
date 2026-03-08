-- Add popularity scoring columns to releases table
ALTER TABLE releases ADD COLUMN IF NOT EXISTS discogs_want INTEGER;
ALTER TABLE releases ADD COLUMN IF NOT EXISTS discogs_have INTEGER;
ALTER TABLE releases ADD COLUMN IF NOT EXISTS discogs_rating REAL;
ALTER TABLE releases ADD COLUMN IF NOT EXISTS discogs_rating_count INTEGER;
ALTER TABLE releases ADD COLUMN IF NOT EXISTS popularity_score REAL;
ALTER TABLE releases ADD COLUMN IF NOT EXISTS lastfm_listeners INTEGER;
ALTER TABLE releases ADD COLUMN IF NOT EXISTS lastfm_playcount INTEGER;
ALTER TABLE releases ADD COLUMN IF NOT EXISTS has_wikipedia BOOLEAN DEFAULT FALSE;
ALTER TABLE releases ADD COLUMN IF NOT EXISTS enriched_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS idx_releases_popularity_score ON releases(popularity_score DESC NULLS LAST);
