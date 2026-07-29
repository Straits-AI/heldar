//! Proprietary vertical composition seam — a NO-OP stub in the open repo.
//!
//! `main.rs` calls these functions unconditionally. In the open build they do nothing and reference
//! no proprietary crate. The private workspace replaces this file with the real composition module
//! (the proprietary verticals) — `main.rs` is identical across both repos.

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
