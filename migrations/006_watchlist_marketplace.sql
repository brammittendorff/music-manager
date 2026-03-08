-- Add marketplace stats to watchlist items and a skip reason.
ALTER TABLE watchlist ADD COLUMN lowest_price_eur NUMERIC(8,2);
ALTER TABLE watchlist ADD COLUMN num_for_sale     INTEGER;
ALTER TABLE watchlist ADD COLUMN skip_reason      TEXT;  -- 'found_on_streaming', 'too_expensive', 'manual', etc.
ALTER TABLE watchlist ADD COLUMN price_checked_at TIMESTAMPTZ;
