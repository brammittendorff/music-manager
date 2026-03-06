#!/usr/bin/env bash
# Reset dev database and re-run migrations
set -euo pipefail

echo "Stopping PostgreSQL..."
docker compose down -v

echo "Starting fresh PostgreSQL..."
docker compose up -d postgres

echo "Waiting for PostgreSQL to be ready..."
until docker compose exec postgres pg_isready -U mmgr -d music_manager 2>/dev/null; do
    sleep 1
done

echo "Running migrations..."
export DATABASE_URL="postgres://mmgr:mmgr@localhost:5432/music_manager"
sqlx migrate run --source migrations

echo "Done. Database is fresh."
