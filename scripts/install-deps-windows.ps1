# Install system dependencies for music-manager on Windows
# Run as Administrator in PowerShell

Write-Host "Installing music-manager system dependencies (Windows)..." -ForegroundColor Cyan

# Check for Chocolatey
if (-not (Get-Command choco -ErrorAction SilentlyContinue)) {
    Write-Host "Installing Chocolatey..." -ForegroundColor Yellow
    Set-ExecutionPolicy Bypass -Scope Process -Force
    [System.Net.ServicePointManager]::SecurityProtocol = [System.Net.ServicePointManager]::SecurityProtocol -bor 3072
    Invoke-Expression ((New-Object System.Net.WebClient).DownloadString('https://community.chocolatey.org/install.ps1'))
}

Write-Host "Installing ffmpeg (CD ripping backend + format detection)..."
choco install ffmpeg -y

Write-Host "Installing flac (lossless encoding)..."
choco install flac -y

Write-Host "Installing Docker Desktop (for PostgreSQL)..."
choco install docker-desktop -y

Write-Host "Installing Rust (if not present)..."
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $rustup = "$env:TEMP\rustup-init.exe"
    Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $rustup
    Start-Process -FilePath $rustup -Args "-y" -Wait
    $env:PATH += ";$env:USERPROFILE\.cargo\bin"
}

Write-Host "Installing sqlx-cli..."
cargo install sqlx-cli --no-default-features --features postgres

Write-Host ""
Write-Host "NOTE: cdparanoia is not natively available on Windows." -ForegroundColor Yellow
Write-Host "      The ripper will use ffmpeg as the backend instead." -ForegroundColor Yellow
Write-Host "      This is slightly less accurate but works for most CDs." -ForegroundColor Yellow
Write-Host ""
Write-Host "NOTE: For LAME MP3 encoding, the mp3lame-encoder crate requires" -ForegroundColor Yellow
Write-Host "      LAME headers/libs. On Windows, set in config/local.toml:" -ForegroundColor Yellow
Write-Host "      Or compile Rust with bundled LAME (check crate docs)." -ForegroundColor Yellow
Write-Host ""
Write-Host "All dependencies installed!" -ForegroundColor Green
Write-Host ""
Write-Host "Next steps:"
Write-Host "  1. Copy .env.example to .env and fill in your API keys"
Write-Host "  2. Start the database: docker compose up -d postgres"
Write-Host "  3. Run migrations:     mm migrate"
Write-Host "  4. Start searching:    mm search"
