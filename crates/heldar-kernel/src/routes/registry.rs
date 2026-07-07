//! Plugin store registry routes (Phase C).
//!
//! `GET /api/v1/registry` serves the merged store catalog (bundled + verified remote, cross-referenced
//! with installed/compiled state) — readable by any principal, offline-safe, no network. `POST
//! /api/v1/registry/refresh` re-fetches the remote registries (admin, audited, network). Installing an
//! entry reuses the Phase B `POST /api/v1/modules` register path; this module only does discovery.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use crate::auth::{self, Principal};
use crate::error::AppResult;
use crate::registry::RegistryView;
use crate::services;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/registry", get(list))
        .route("/api/v1/registry/refresh", post(refresh))
}

/// The merged store view: bundled + remote catalogs, each entry cross-referenced for installed state.
async fn list(State(st): State<AppState>, principal: Principal) -> AppResult<Json<RegistryView>> {
    principal.require(principal.can_view(), "view the plugin store")?;
    let registrations = services::modules::list_registered(&st.pool).await?;
    Ok(Json(st.catalog.view(&st.modules, &registrations)))
}

/// Re-fetch the remote registries now (admin; performs outbound requests). Returns the refreshed view.
async fn refresh(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<RegistryView>> {
    principal.require(principal.can_admin(), "refresh the plugin registry")?;
    st.catalog.refresh().await;
    auth::audit(
        &st.pool,
        &principal,
        "refresh_registry",
        "registry",
        "remote",
        json!({}),
    )
    .await;
    let registrations = services::modules::list_registered(&st.pool).await?;
    Ok(Json(st.catalog.view(&st.modules, &registrations)))
}

#[cfg(test)]
mod tests {
    use crate::config::Config;
    use crate::modules::{ModuleKind, ModuleManifest, NavEntry};
    use crate::services::recorder::RecorderManager;
    use crate::services::registry::CatalogService;
    use crate::services::sampler::SamplerManager;
    use crate::state::AppState;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::Arc;
    use tower::Service;

    async fn state_with(modules: Vec<ModuleManifest>) -> AppState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let mut cfg = Config::from_env();
        cfg.auth_enabled = false;
        let cfg = Arc::new(cfg);
        AppState {
            recorder: RecorderManager::new(pool.clone(), cfg.clone()),
            sampler: SamplerManager::new(pool.clone(), cfg.clone()),
            mirror: None,
            consumers: Arc::new(Vec::new()),
            modules: Arc::new(modules),
            catalog: Arc::new(CatalogService::new(&cfg)),
            http: reqwest::Client::new(),
            started_at: chrono::Utc::now(),
            pool,
            cfg,
        }
    }

    /// GET /api/v1/registry serves the bundled catalog, cross-referenced: a compiled module shows
    /// `included`, an uninstalled sidecar shows `available`, and bundled entries are `verified`.
    #[tokio::test]
    async fn serves_bundled_catalog_cross_referenced() {
        let entry = ModuleManifest::new(
            "entry",
            "Access Control",
            "0.0.0",
            "Heldar",
            ModuleKind::Core,
            "d",
            vec![NavEntry::new("/entry", "Entry", "entry")],
        );
        let st = state_with(vec![entry]).await;
        let mut app = super::router().with_state(st);
        let res = app
            .call(
                Request::builder()
                    .uri("/api/v1/registry")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        let entries = json["entries"].as_array().unwrap();
        let find = |id: &str| entries.iter().find(|e| e["id"] == id).cloned().unwrap();

        let entry_v = find("entry");
        assert_eq!(entry_v["shelf"], "core");
        assert_eq!(entry_v["state"], "included"); // compiled in
        assert_eq!(entry_v["verified"], true); // bundled = trusted by construction
        assert_eq!(entry_v["source"], "bundled");

        // movement is in the bundled catalog but NOT compiled into this minimal build.
        assert_eq!(find("movement")["state"], "not_in_build");

        // the reference sidecar is community + available (not installed).
        let hello = find("hello-module");
        assert_eq!(hello["shelf"], "community");
        assert_eq!(hello["state"], "available");

        // the bundled source is verified + first-party.
        let bundled = json["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["source"] == "bundled")
            .unwrap();
        assert_eq!(bundled["verified"], true);
        assert_eq!(bundled["first_party"], true);
    }
}
