mod discovery;
pub mod enrichment;
mod platform_checker;
mod releases;
mod rip;
mod watchlist;

use axum::{Json, http::StatusCode};

type AppResult<T> = Result<Json<T>, (StatusCode, String)>;

fn db_err(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

// Re-export all public handler functions
pub use discovery::{
    discovery_clear, discovery_create, discovery_delete_job, discovery_job_status,
    discovery_list, discovery_pause, discovery_resume,
};
pub use enrichment::{enrich_releases, run_enrichment_loop};
pub use platform_checker::{
    platform_checker_start, platform_checker_status, platform_checker_stop,
    run_platform_checker_watchdog,
};
pub use releases::{
    clear_releases, export_releases, import_releases, release_detail, release_tracks, releases,
    rip_jobs, stats,
};
pub use rip::{detect_drives, ready_to_rip, start_rip};
pub use watchlist::{
    add_to_watchlist, delete_watchlist_item, run_watchlist_automation, update_watchlist_status,
    watchlist,
};
