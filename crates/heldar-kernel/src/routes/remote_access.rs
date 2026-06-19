//! Kernel-managed WireGuard remote-access API (the `wireguard` feature).
//!
//! Status + peer enrollment for Heldar's own isolated WireGuard interface ([`crate::services::wireguard`]).
//! Status + peer listing are readable by any authenticated principal (`can_view`); enrolling and
//! removing devices are manager+ (`can_manage_registry`) and audited.
//!
//! Onboarding without a login session: a manager mints a short-TTL, single-use **pairing token**
//! (`POST .../pairing`), encodes it in a QR, and a new device redeems it at `POST .../pair` — an
//! UNauthenticated endpoint gated solely by that token. The pair path REQUIRES a client-generated
//! `public_key`, so the unauthenticated surface can never generate or return a private key.
//!
//! Security: prefer the client-generated-key flow on every path — the device makes its own keypair and
//! sends only the public key, so the peer private key never crosses the wire. Omitting `public_key`
//! falls back to server-generating it (convenient for curl; the key then rides in the response).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::task::spawn_blocking;

use crate::auth::{self, Principal};
use crate::error::{AppError, AppResult};
use crate::services::wireguard;
use crate::state::AppState;

/// How long a pairing token is valid (single-use within this window).
const PAIRING_TTL_SECS: i64 = 600;

/// In-memory pairing-token store: token -> expiry (unix seconds). Boot-volatile by design — tokens are
/// short-lived, so losing them on restart is fine and avoids a DB migration / persisted secret.
static PAIRING: LazyLock<Mutex<HashMap<String, i64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/remote-access", get(status))
        .route(
            "/api/v1/remote-access/peers",
            get(list_peers).post(add_peer).delete(remove_peer),
        )
        .route("/api/v1/remote-access/pairing", post(mint_pairing))
        .route("/api/v1/remote-access/pair", post(pair))
}

fn join_err(e: tokio::task::JoinError) -> AppError {
    AppError::Other(anyhow::anyhow!("remote-access task join: {e}"))
}

fn mint_token() -> (String, i64) {
    let token = uuid::Uuid::new_v4().simple().to_string();
    let now = Utc::now().timestamp();
    let expires = now + PAIRING_TTL_SECS;
    let mut g = PAIRING.lock().unwrap();
    g.retain(|_, exp| *exp > now); // prune expired
    g.insert(token.clone(), expires);
    (token, expires)
}

/// Validate + SINGLE-USE consume a pairing token. True iff it existed and was unexpired.
fn consume_token(token: &str) -> bool {
    let now = Utc::now().timestamp();
    let mut g = PAIRING.lock().unwrap();
    g.retain(|_, exp| *exp > now);
    g.remove(token).is_some_and(|exp| exp > now)
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
    /// The device's own WireGuard public key (client-generated). Strongly preferred: the private key
    /// then never crosses the wire. Omit to have the server generate the keypair (returned in `config`).
    #[serde(default)]
    public_key: Option<String>,
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
    let pk = body.public_key.clone();
    let n = name.clone();
    let peer = spawn_blocking(move || wireguard::add_peer(&cfg, &n, pk.as_deref()))
        .await
        .map_err(join_err)??;
    auth::audit(
        &st.pool,
        &principal,
        "enroll_remote_peer",
        "remote_peer",
        &peer.public_key,
        json!({ "name": name, "address": peer.address, "client_key": body.public_key.is_some() }),
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

/// Mint a short-TTL, single-use pairing token (manager+). The dashboard encodes it in a QR for a new
/// device to redeem at `/pair`. Audited so token issuance is traceable.
async fn mint_pairing(State(st): State<AppState>, principal: Principal) -> AppResult<Json<Value>> {
    principal.require(
        principal.can_manage_registry(),
        "create remote-access pairing tokens",
    )?;
    let (token, expires_at) = mint_token();
    auth::audit(
        &st.pool,
        &principal,
        "create_pairing_token",
        "remote_peer",
        "pairing",
        json!({ "ttl_seconds": PAIRING_TTL_SECS }),
    )
    .await;
    Ok(Json(
        json!({ "token": token, "expires_at": expires_at, "ttl_seconds": PAIRING_TTL_SECS }),
    ))
}

#[derive(Deserialize)]
struct PairBody {
    /// A valid, unexpired pairing token from `POST .../pairing`.
    token: String,
    name: String,
    /// REQUIRED on this UNauthenticated path — the device's own public key. The server never generates
    /// a private key here, so the token-gated surface cannot leak one.
    public_key: String,
}

/// Redeem a pairing token to enroll a device — NO login required (gated by the single-use token). The
/// device must supply its own `public_key`; the returned config carries a placeholder for the private
/// key the device already holds.
async fn pair(
    State(st): State<AppState>,
    Json(body): Json<PairBody>,
) -> AppResult<Json<wireguard::EnrolledPeer>> {
    if !consume_token(body.token.trim()) {
        return Err(AppError::Unauthorized(
            "invalid or expired pairing token".into(),
        ));
    }
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest("`name` is required".into()));
    }
    let pk = body.public_key.trim().to_string();
    if pk.is_empty() {
        return Err(AppError::BadRequest("`public_key` is required".into()));
    }
    let cfg = st.cfg.clone();
    let n = name.clone();
    let peer = spawn_blocking(move || wireguard::add_peer(&cfg, &n, Some(&pk)))
        .await
        .map_err(join_err)??;
    tracing::info!(name = %name, address = %peer.address, "device paired via pairing token");
    Ok(Json(peer))
}
