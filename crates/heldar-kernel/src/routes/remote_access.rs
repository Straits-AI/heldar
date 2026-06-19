//! Kernel-managed WireGuard remote-access API (the `wireguard` feature).
//!
//! Status + peer enrollment for Heldar's own isolated WireGuard interface ([`crate::services::wireguard`]).
//! Status + peer listing are readable by any authenticated principal (`can_view`); enrolling and
//! removing devices are manager+ (`can_manage_registry`) and audited. The privileged `ip`/`wg` work is
//! blocking, so handlers run it on the blocking pool. Peer public keys are base64 (they contain `/`),
//! so they travel as a query parameter, never a path segment.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use tokio::task::spawn_blocking;

use crate::auth::{self, Principal};
use crate::error::{AppError, AppResult};
use crate::services::wireguard;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/remote-access", get(status))
        .route(
            "/api/v1/remote-access/peers",
            get(list_peers).post(add_peer).delete(remove_peer),
        )
}

fn join_err(e: tokio::task::JoinError) -> AppError {
    AppError::Other(anyhow::anyhow!("remote-access task join: {e}"))
}

async fn status(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<wireguard::WgStatus>> {
    principal.require(principal.can_view(), "view remote-access status")?;
    let cfg = st.cfg.clone();
    let s = spawn_blocking(move || wireguard::status(&cfg))
        .await
        .map_err(join_err)?;
    Ok(Json(s))
}

async fn list_peers(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<Vec<wireguard::PeerInfo>>> {
    principal.require(principal.can_view(), "list remote-access peers")?;
    let cfg = st.cfg.clone();
    let peers = spawn_blocking(move || wireguard::list_peers(&cfg))
        .await
        .map_err(join_err)??;
    Ok(Json(peers))
}

#[derive(Deserialize)]
struct AddPeer {
    name: String,
}

async fn add_peer(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<AddPeer>,
) -> AppResult<Json<wireguard::EnrolledPeer>> {
    principal.require(
        principal.can_manage_registry(),
        "enroll remote-access devices",
    )?;
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest("`name` is required".into()));
    }
    let cfg = st.cfg.clone();
    let n = name.clone();
    let peer = spawn_blocking(move || wireguard::add_peer(&cfg, &n))
        .await
        .map_err(join_err)??;
    auth::audit(
        &st.pool,
        &principal,
        "enroll_remote_peer",
        "remote_peer",
        &peer.public_key,
        json!({ "name": name, "address": peer.address }),
    )
    .await;
    Ok(Json(peer))
}

#[derive(Deserialize)]
struct RemoveQuery {
    /// The peer's base64 public key (URL-encoded by the client).
    key: String,
}

async fn remove_peer(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<RemoveQuery>,
) -> AppResult<StatusCode> {
    principal.require(
        principal.can_manage_registry(),
        "remove remote-access devices",
    )?;
    let cfg = st.cfg.clone();
    let key = q.key.clone();
    spawn_blocking(move || wireguard::remove_peer(&cfg, &key))
        .await
        .map_err(join_err)??;
    auth::audit(
        &st.pool,
        &principal,
        "remove_remote_peer",
        "remote_peer",
        &q.key,
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}
