# Wax Vault - Dutch Music Preservation

Discovers Dutch music releases on Discogs that are missing from streaming platforms (Spotify, Deezer, Apple Music, Bandcamp, YouTube Music), manages a buy/watchlist, and auto-rips CDs to MP3 320kbps.

## Features

- **Discovery jobs** - scan Discogs for NL (or any country) album releases with configurable filters (year, genre, format)
- **Platform checking** - concurrent checks against Spotify, Deezer, Apple Music, Bandcamp, YouTube Music
- **Track-level results** - checks each individual track per album, not just album title
- **Release management** - filter by platform status, export/import as JSON, add to watchlist
- **Watchlist pipeline** - to_buy → ordered → purchased → ready_to_rip → done
- **CD ripping** - auto-detects disc insertion, rips to MP3 320kbps or FLAC via cdparanoia/ffmpeg
- **Web UI** - React frontend with live progress, platform filter badges, detail panels

## Stack

| Layer | Tech |
|---|---|
| Backend | Rust (Axum, SQLx, Tokio) |
| Database | PostgreSQL 16 |
| Frontend | React 19, Vite, TanStack Query |
| CD Ripping | cdparanoia (Linux) / ffmpeg (Windows) |

## Quick Start

```bash
# 1. Clone and configure
cp .env.example .env
# Edit .env - add your Discogs token (required) and Spotify keys (optional)

# 2. Start everything
docker compose up -d

# 3. Open the UI
open http://localhost:8888
```

### Services

| Service | URL |
|---|---|
| Frontend (dev, hot-reload) | http://localhost:8888 |
| API | http://localhost:3001 |
| pgAdmin | http://localhost:5050 |
| Adminer | http://localhost:8080 |

## Configuration

Config is layered: `config/default.toml` → `config/local.toml` → environment variables.

All env vars use the prefix `MMGR_` with `__` as the separator for nested keys:
```
MMGR_SEARCH__COUNTRY=Germany
MMGR_API__DISCOGS_TOKEN=xxx
```

See `config/default.toml` for all available options.

## Development

The frontend runs as a Vite dev server with hot-reload - changes to `frontend/src/` appear immediately in the browser.

```bash
# Rebuild API after Rust changes
docker compose up -d --build api

# Frontend hot-reloads automatically (no rebuild needed)
```

## Discovery Workflow

1. **Create a job** on the Jobs page - set country, year range, format (Album/Single), genre
2. **Discovery runs** - scans Discogs page by page, saves release metadata instantly
3. **Platform checker runs** - checks each album's tracks against all platforms concurrently
4. **Browse releases** - filter by "Not on platforms" to find preservation targets
5. **Add to watchlist** - track purchases through the pipeline
6. **Rip CDs** - insert disc, `mm rip daemon` auto-detects and rips

## License

MIT
