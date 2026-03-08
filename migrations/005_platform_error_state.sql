-- Add error column to platform_checks and track_checks.
-- When a platform returns a transient error (429, timeout, etc.),
-- we store error=true so the UI shows it as "error" (yellow) instead of
-- "missing" (red), and the checker knows to retry later.

ALTER TABLE platform_checks ADD COLUMN error BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE track_checks    ADD COLUMN error BOOLEAN NOT NULL DEFAULT false;

-- Index to quickly find releases with errored checks (for retry logic)
CREATE INDEX idx_platform_checks_error ON platform_checks(error) WHERE error = true;
CREATE INDEX idx_track_checks_error    ON track_checks(error)    WHERE error = true;
