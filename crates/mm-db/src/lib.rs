pub mod models;
pub mod queries;

use anyhow::Result;
use mm_config::AppConfig;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::time::Duration;
use tracing::{info, warn};

pub type Db = PgPool;

/// Connect with retry - gives postgres time to become ready in Docker.
/// Prefers DATABASE_URL env var over config file (standard 12-factor pattern).
pub async fn connect(cfg: &AppConfig) -> Result<Db> {
    let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| cfg.database.url.clone());
    info!("Connecting to database: {}", url.split('@').last().unwrap_or("(url hidden)"));

    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match PgPoolOptions::new()
            .max_connections(cfg.database.max_connections)
            .acquire_timeout(Duration::from_secs(30))
            .connect(&url)
            .await
        {
            Ok(pool) => return Ok(pool),
            Err(e) if attempt < 10 => {
                warn!("DB connect attempt {attempt}/10 failed: {e} - retrying in 3s");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

pub async fn migrate(pool: &Db) -> Result<()> {
    sqlx::migrate!("../../migrations").run(pool).await?;
    Ok(())
}
