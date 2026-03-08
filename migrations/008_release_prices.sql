-- Add marketplace price columns to releases table so we can show prices for all releases,
-- not just watchlist items.
ALTER TABLE releases ADD COLUMN lowest_price_eur NUMERIC(8,2);
ALTER TABLE releases ADD COLUMN num_for_sale     INTEGER;
ALTER TABLE releases ADD COLUMN price_checked_at TIMESTAMPTZ;
