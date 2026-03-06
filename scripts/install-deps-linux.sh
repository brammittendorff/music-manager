#!/usr/bin/env bash
# Install system dependencies for music-manager on Linux (Debian/Ubuntu)
set -euo pipefail

echo "Installing music-manager system dependencies..."

sudo apt-get update
sudo apt-get install -y \
    cdparanoia        \  # CD ripping backend
    cd-discid         \  # Disc ID / MusicBrainz TOC computation
    flac              \  # FLAC encoding CLI
    ffmpeg            \  # Fallback ripping and format detection
    libmp3lame-dev    \  # LAME MP3 encoder (for mp3lame-encoder Rust crate)
    libudev-dev       \  # udev bindings (for disc detection)
    pkg-config        \  # Required to find native libraries
    build-essential      # Rust build tools

echo ""
echo "Installing Rust (if not present)..."
if ! command -v cargo &>/dev/null; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
fi

echo ""
echo "Installing sqlx-cli for migrations..."
cargo install sqlx-cli --no-default-features --features postgres

echo ""
echo "All dependencies installed!"
echo ""
echo "Next steps:"
echo "  1. Copy .env.example to .env and fill in your API keys"
echo "  2. Start the database: docker compose up -d postgres"
echo "  3. Run migrations:     mm migrate"
echo "  4. Start searching:    mm search"
