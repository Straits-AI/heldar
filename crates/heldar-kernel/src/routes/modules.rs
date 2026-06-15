//! Loaded-module listing.
//!
//! `GET /api/v1/modules` returns the manifests the composing binary populated into
//! [`AppState::modules`]. The dashboard reads this to build its nav rail + client routes from live
//! truth, so only loaded modules appear and routes stay dynamic. Readable by any authenticated
//! principal (`can_view`); it carries no secrets, just module identity + nav.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};

use crate::auth::Principal;
use crate::error::AppResult;
use crate::modules::ModuleManifest;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/modules", get(list))
}

async fn list(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<Vec<ModuleManifest>>> {
    principal.require(principal.can_view(), "list modules")?;
    Ok(Json(st.modules.as_ref().clone()))
}

#[cfg(test)]
mod tests {
    use crate::config::Config;
    use crate::modules::{ModuleKind, ModuleManifest, NavEntry};
    use crate::services::recorder::RecorderManager;
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
        cfg.auth_enabled = false; // exercise the handler without an auth principal
        let cfg = Arc::new(cfg);
        AppState {
            recorder: RecorderManager::new(pool.clone(), cfg.clone()),
            sampler: SamplerManager::new(pool.clone(), cfg.clone()),
            mirror: None,
            consumers: Arc::new(Vec::new()),
            modules: Arc::new(modules),
            http: reqwest::Client::new(),
            started_at: chrono::Utc::now(),
            pool,
            cfg,
        }
    }

    /// GET /api/v1/modules returns exactly the composed manifests, serialized as the dashboard expects.
    #[tokio::test]
    async fn lists_loaded_modules() {
        let m = ModuleManifest::new(
            "entry",
            "Access Control",
            "9.9.9",
            "Heldar",
            ModuleKind::Core,
            "desc",
            vec![NavEntry::new("/entry", "Entry", "entry")],
        );
        let st = state_with(vec![m]).await;
        let mut app = super::router().with_state(st);
        let res = app
            .call(
                Request::builder()
                    .uri("/api/v1/modules")
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
        let arr = json.as_array().expect("array");
        assert_eq!(arr.len(), 1);
        assert_eq!(json[0]["id"], "entry");
        assert_eq!(json[0]["kind"], "core"); // snake_case enum serialization
        assert_eq!(json[0]["nav"][0]["path"], "/entry");
    }

    /// With no modules composed (e.g. an API-only build), the endpoint returns an empty list, not 404.
    #[tokio::test]
    async fn empty_when_no_modules() {
        let st = state_with(vec![]).await;
        let mut app = super::router().with_state(st);
        let res = app
            .call(
                Request::builder()
                    .uri("/api/v1/modules")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"[]");
    }
}
