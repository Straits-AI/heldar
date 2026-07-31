//! Vertical composition for this workspace — deliberately a NO-OP.
//!
//! `main.rs` adapts these functions into a [`heldar_server::Verticals`] implementation. Nothing here
//! references a proprietary crate, so the reference `heldar-core` binary links none. This file is
//! also the smallest worked example of the seam: a private product implements the same four hooks
//! in its own repository and passes them to `heldar_server::run`.

use axum::Router;
use heldar_kernel::modules::ModuleManifest;
use heldar_kernel::state::AppState;
use sqlx::SqlitePool;

pub fn manifests() -> Vec<ModuleManifest> {
    Vec::new()
}

pub async fn init_schema(_pool: &SqlitePool) -> anyhow::Result<()> {
    Ok(())
}

pub fn spawn_loops(_pool: &SqlitePool) {}

pub fn merge_routes(router: Router<AppState>) -> Router<AppState> {
    router
}
