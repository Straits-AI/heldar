use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::auth::{self, Principal};
use crate::error::{AppError, AppResult};
use crate::services::discovery::{self, DiscoverOptions};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/discover", post(discover_handler))
}

/// Scan a network range for cameras; optionally verify credentials and auto-register them.
async fn discover_handler(
    State(st): State<AppState>,
    principal: Principal,
    Json(opts): Json<DiscoverOptions>,
) -> AppResult<Json<Value>> {
    // Scanning the LAN is an operational action (viewer+); auto-registering the cameras it finds is a
    // registry mutation, so it additionally requires manage-registry.
    principal.require(principal.can_view(), "scan for cameras")?;
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
