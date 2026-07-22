//! The composed `heldar-core` binary for THIS workspace: the library server plus the in-tree
//! vertical composition (`verticals.rs` — the real proprietary module here; a no-op stub in the
//! open repo, so this file is identical across both). An out-of-tree overlay builds its own bin
//! against `heldar_server::run` instead of this one.

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
