use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::auth::{self, Cap, Principal};
use crate::error::{AppError, AppResult};
use crate::services::discovery::{self, DiscoverOptions};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/discover", post(discover_handler))
}

/// Scan a network range for cameras; optionally verify credentials and auto-register them.
/// Scan the LAN for cameras.
///
/// FLEET-ONLY. Every returned device carries `already_registered`, computed against the whole
/// camera table, so a camera-scoped credential would learn the size of the fleet and the address of
/// every camera on it — the roster leak in address space rather than id space. `auto_add` is worse:
/// the ids it mints can never be in the caller's allowlist.
#[utoipa::path(
    post, path = "/api/v1/discover", tag = "cameras",
    operation_id = "discoverCameras",
    responses(
        (status = 200, description = "Devices found, each flagged with whether it is already registered"),
        (status = 400, description = "Malformed scan options", body = crate::openapi::ErrorBody),
        (status = 403, description = "Missing `net:scan`, missing `registry:manage` for `auto_add`, or a camera-scoped credential", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn discover_handler(
    State(st): State<AppState>,
    principal: Principal,
    Json(opts): Json<DiscoverOptions>,
) -> AppResult<Json<Value>> {
    // Scanning the LAN is an operational action (viewer+); auto-registering the cameras it finds is a
    // registry mutation, so it additionally requires manage-registry.
    principal.require_cap(Cap::NetScan, "scan for cameras")?;
    // Box-level, and the sharper of the two discovery surfaces. Every returned device carries
    // `already_registered`, computed against the WHOLE `cameras` table — so a camera-scoped
    // credential learns the size of the fleet and the address of every camera on it, which is the
    // roster leak in address space rather than id space. `auto_add` is worse still: the ids it mints
    // are derived from the device and can never be in the caller's allowlist. There is no camera to
    // scope the scan by, so containment is a refusal. See `cameras::require_fleet_scope`.
    crate::routes::cameras::require_fleet_scope(&principal, "scan the network for cameras")?;
    if opts.auto_add {
        principal.require(
            principal.can_manage_registry(),
            "auto-register discovered cameras",
        )?;
    }
    let devices = discovery::discover(&st.pool, &st.cfg, &st.http, &opts)
        .await
        .map_err(AppError::BadRequest)?;

    let mut added: Vec<String> = Vec::new();
    if opts.auto_add {
        for d in devices
            .iter()
            .filter(|d| d.verified && !d.already_registered)
        {
            match discovery::add_device(&st.pool, d).await {
                Ok(id) => {
                    st.recorder.reconcile(&id).await;
                    added.push(id);
                }
                Err(e) => {
                    tracing::error!(addr = %d.address, error = %e, "discover: auto-add failed")
                }
            }
        }
    }

    if !added.is_empty() {
        auth::audit(
            &st.pool,
            &principal,
            "discover_auto_add",
            "discovery",
            "auto_add",
            json!({ "added": &added }),
        )
        .await;
    }

    Ok(Json(json!({
        "scanned": opts.targets,
        "found": devices.len(),
        "verified": devices.iter().filter(|d| d.verified).count(),
        "added": added,
        "devices": devices,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Scope;
    use axum::extract::State;
    use std::collections::HashSet;
    use std::sync::Arc;

    async fn test_state() -> AppState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let cfg = Arc::new(crate::config::Config::from_env());
        AppState {
            recorder: crate::services::recorder::RecorderManager::new(pool.clone(), cfg.clone()),
            sampler: crate::services::sampler::SamplerManager::new(pool.clone(), cfg.clone()),
            live: crate::services::live_publisher::LivePublisherManager::new(
                pool.clone(),
                cfg.clone(),
                reqwest::Client::new(),
            ),
            mirror: None,
            consumers: Arc::new(Vec::new()),
            modules: Arc::new(Vec::new()),
            catalog: Arc::new(crate::services::registry::CatalogService::new(&cfg)),
            http: reqwest::Client::new(),
            media_jobs: crate::services::media_jobs::MediaJobGovernor::new(2),
            started_at: chrono::Utc::now(),
            pool,
            cfg,
        }
    }

    fn scoped(cameras: &[&str]) -> Principal {
        let set: HashSet<String> = cameras.iter().map(|c| c.to_string()).collect();
        Principal {
            scope: Scope::Cameras(Arc::new(set)),
            ..Principal::system_admin()
        }
    }

    fn opts(auto_add: bool) -> DiscoverOptions {
        serde_json::from_value(json!({ "targets": "127.0.0.1", "auto_add": auto_add })).unwrap()
    }

    /// A LAN sweep answers "which addresses on this segment are cameras, and which of them are
    /// already registered on this box" — the fleet roster in address space — and with `auto_add` it
    /// enrolls cameras whose ids can never be in the caller's allowlist. There is no camera id to
    /// scope it by, so a camera-scoped credential is refused outright, BEFORE any packet is sent.
    #[tokio::test]
    async fn network_discovery_is_refused_to_a_camera_scoped_credential() {
        let st = test_state().await;
        for auto_add in [false, true] {
            let err = discover_handler(State(st.clone()), scoped(&["cam_a"]), Json(opts(auto_add)))
                .await
                .unwrap_err();
            assert!(matches!(err, AppError::Forbidden(_)), "{err:?}");
            // The refusal is about the credential, not about what is out there.
            assert!(!err.to_string().contains("127.0.0.1"));
        }
        // Nothing was enrolled as a side effect of the refused scan.
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cameras")
            .fetch_one(&st.pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
    }
}
