mod handlers;
mod models;

use anyhow::Result;
use axum::{
    Router,
    routing::{delete, get, patch, post},
};
use mm_config::AppConfig;
use mm_db::{connect, migrate};
use std::sync::{Arc, atomic::AtomicBool};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

pub struct AppState {
    pub pool: mm_db::Db,
    pub cfg: AppConfig,
    /// ID of the currently running job, None if idle.
    pub active_job_id: tokio::sync::Mutex<Option<Uuid>>,
    /// Set true to pause/cancel the currently running job.
    pub discovery_cancel: Arc<AtomicBool>,
    /// True while the platform checker background task is running.
    pub platform_checker_active: Arc<AtomicBool>,
    /// Set true to stop the platform checker.
    pub platform_checker_cancel: Arc<AtomicBool>,
    /// Handle to the running platform checker task so it can be aborted immediately.
    pub platform_checker_handle: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cfg = AppConfig::load()?;
    let pool = connect(&cfg).await?;
    migrate(&pool).await?;

    // Mark any discovery jobs left in 'running' state as paused (API restarted).
    sqlx::query(
        "UPDATE discovery_jobs SET status = 'paused', error_msg = 'API restarted', \
         finished_at = now() WHERE status = 'running'",
    )
    .execute(&pool)
    .await?;

    let state = Arc::new(AppState {
        pool,
        cfg,
        active_job_id: tokio::sync::Mutex::new(None),
        discovery_cancel: Arc::new(AtomicBool::new(false)),
        platform_checker_active: Arc::new(AtomicBool::new(false)),
        platform_checker_cancel: Arc::new(AtomicBool::new(false)),
        platform_checker_handle: tokio::sync::Mutex::new(None),
    });

    // Auto-start the platform checker watchdog if Discogs token is configured.
    if !state.cfg.api.discogs_token.is_empty() {
        tokio::spawn(handlers::run_platform_checker_watchdog(state.clone()));
        tokio::spawn(handlers::run_watchlist_automation(state.clone()));
    }

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/api/stats",                             get(handlers::stats))
        .route("/api/releases",                          get(handlers::releases))
        .route("/api/releases/export",                   get(handlers::export_releases))
        .route("/api/releases/import",                   post(handlers::import_releases))
        .route("/api/releases/enrich",                   post(handlers::enrich_releases))
        .route("/api/releases/clear",                    delete(handlers::clear_releases))
        .route("/api/releases/:id",                      get(handlers::release_detail))
        .route("/api/releases/:id/tracks",               get(handlers::release_tracks))
        .route("/api/watchlist",                         get(handlers::watchlist).post(handlers::add_to_watchlist))
        .route("/api/watchlist/:id/status",              patch(handlers::update_watchlist_status))
        .route("/api/watchlist/:id",                    delete(handlers::delete_watchlist_item))
        .route("/api/rip-jobs",                          get(handlers::rip_jobs))
        .route("/api/discovery",                         delete(handlers::discovery_clear))
        .route("/api/discovery/jobs",                    get(handlers::discovery_list).post(handlers::discovery_create))
        .route("/api/discovery/jobs/:id/resume",         post(handlers::discovery_resume))
        .route("/api/discovery/jobs/:id/pause",          post(handlers::discovery_pause))
        .route("/api/discovery/jobs/:id",                get(handlers::discovery_job_status).delete(handlers::discovery_delete_job))
        .route("/api/platform-checker/status",           get(handlers::platform_checker_status))
        .route("/api/platform-checker/start",            post(handlers::platform_checker_start))
        .route("/api/platform-checker/stop",             post(handlers::platform_checker_stop))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = "0.0.0.0:3001";
    info!("API server listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
