//! The composed `heldar-core` binary: the library server (`heldar_server::run`) plus this
//! workspace's vertical composition (`verticals.rs`, a no-op here — the open build links no
//! proprietary code). A private product does NOT fork this file: it depends on this crate (by git
//! tag — the composition crate is `publish = false`) and calls `heldar_server::run` with its own
//! `Verticals` implementation.

use axum::Router;
use heldar_kernel::modules::ModuleManifest;
use heldar_kernel::state::AppState;
use sqlx::SqlitePool;

// All proprietary-crate references live in this module, behind the `verticals` Cargo feature.
mod verticals;

/// Adapter: the in-tree seam module as a [`heldar_server::Verticals`] composition.
struct TreeVerticals;

impl heldar_server::Verticals for TreeVerticals {
    fn manifests(&self) -> Vec<ModuleManifest> {
        verticals::manifests()
    }
    async fn init_schema(&self, pool: SqlitePool) -> anyhow::Result<()> {
        verticals::init_schema(&pool).await
    }
    fn spawn_loops(&self, pool: &SqlitePool) {
        verticals::spawn_loops(pool)
    }
    fn merge_routes(&self, router: Router<AppState>) -> Router<AppState> {
        verticals::merge_routes(router)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    heldar_server::run(TreeVerticals).await
}
