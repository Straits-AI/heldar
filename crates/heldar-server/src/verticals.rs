//! Proprietary vertical composition — isolated from the open server core.
//!
//! `main.rs` calls these seam functions unconditionally; all proprietary-crate references live HERE,
//! behind the `verticals` Cargo feature. When the feature is off (the open reference build) every
//! function is a no-op and references no proprietary crate. The public repo ships a stub version of
//! this file with the no-op bodies only — so `main.rs` stays identical and pristine across both repos.

use axum::Router;
use heldar_kernel::modules::ModuleManifest;
use heldar_kernel::state::AppState;
use sqlx::SqlitePool;

/// Each vertical's module manifest (collected into AppState + served at GET /api/v1/modules).
#[cfg(feature = "verticals")]
pub fn manifests() -> Vec<ModuleManifest> {
    vec![heldar_bakery::manifest()]
}

/// Apply each vertical's schema (idempotent) against the shared pool.
#[cfg(feature = "verticals")]
pub async fn init_schema(pool: &SqlitePool) -> anyhow::Result<()> {
    use anyhow::Context;
    heldar_bakery::schema::init(pool)
        .await
        .context("bakery schema init")?;
    Ok(())
}

/// Spawn each vertical's supervised background loops.
#[cfg(feature = "verticals")]
pub fn spawn_loops(pool: &SqlitePool) {
    use std::sync::Arc;
    // BakerySense rollup (aggregates anonymous behaviour metrics + prunes its observations).
    let bakery_cfg = Arc::new(heldar_bakery::config::BakeryConfig::from_env());
    let (p, b) = (pool.clone(), bakery_cfg);
    crate::spawn_supervised("bakery_rollup", move || {
        heldar_bakery::rollup::run(p.clone(), b.clone())
    });
}

/// Merge each vertical's router onto the app.
#[cfg(feature = "verticals")]
pub fn merge_routes(router: Router<AppState>) -> Router<AppState> {
    use std::sync::Arc;
    let bakery_cfg = Arc::new(heldar_bakery::config::BakeryConfig::from_env());
    router.merge(heldar_bakery::routes::router(bakery_cfg))
}

// ---- No-op fallbacks when the `verticals` feature is off (the open reference build) ----

#[cfg(not(feature = "verticals"))]
pub fn manifests() -> Vec<ModuleManifest> {
    Vec::new()
}

#[cfg(not(feature = "verticals"))]
pub async fn init_schema(_pool: &SqlitePool) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(not(feature = "verticals"))]
pub fn spawn_loops(_pool: &SqlitePool) {}

#[cfg(not(feature = "verticals"))]
pub fn merge_routes(router: Router<AppState>) -> Router<AppState> {
    router
}
