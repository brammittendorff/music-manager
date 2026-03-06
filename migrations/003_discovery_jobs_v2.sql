-- Add filter columns and paused status to discovery_jobs
ALTER TABLE discovery_jobs
  ADD COLUMN IF NOT EXISTS country      TEXT NOT NULL DEFAULT 'Netherlands',
  ADD COLUMN IF NOT EXISTS genres       TEXT[] NOT NULL DEFAULT '{}',
  ADD COLUMN IF NOT EXISTS year_from    INTEGER NOT NULL DEFAULT 1950,
  ADD COLUMN IF NOT EXISTS year_to      INTEGER NOT NULL DEFAULT 2005,
  ADD COLUMN IF NOT EXISTS format_filter TEXT NOT NULL DEFAULT 'Album',
  ADD COLUMN IF NOT EXISTS max_pages    INTEGER;
  -- status already exists; 'paused' is now a valid value alongside running/completed/failed/cancelled

-- Link releases to the job that discovered them
ALTER TABLE releases
  ADD COLUMN IF NOT EXISTS discovery_job_id UUID REFERENCES discovery_jobs(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_releases_job ON releases(discovery_job_id);
