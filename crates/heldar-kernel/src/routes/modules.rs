//! Module listing, sidecar registration, and the sidecar reverse proxy.
//!
//! `GET /api/v1/modules` merges the compiled-in manifests (from [`AppState::modules`]) with the
//! runtime-registered sidecars (the DB), so the dashboard builds its nav + routes from one live list.
//! Registration (`POST`/`GET {id}`/`DELETE {id}`) is admin-only — installing a plugin mints it a
//! scoped API key + a webhook subscription. `/m/{id}/*` reverse-proxies to the sidecar's own UI + API
//! so a plugin is single-origin with the console (any authenticated principal may reach it).

use axum::body::{Body, Bytes};
use axum::extract::{OriginalUri, Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::Response;
use axum::routing::{any, get};
use axum::{Json, Router};
use serde_json::json;

use crate::auth::{self, Principal};
use crate::error::{AppError, AppResult};
use crate::modules::{ModuleDetail, ModuleManifest, ModuleRegisterRequest, ModuleRegistered};
use crate::services;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/modules", get(list).post(register))
        .route("/api/v1/modules/{id}", get(detail).delete(unregister))
        // Reverse-proxy a sidecar's own UI + API under /m/{id}/ (single-origin with the console).
        .route("/m/{id}", any(proxy_root))
        .route("/m/{id}/", any(proxy_root))
        .route("/m/{id}/{*rest}", any(proxy_sub))
}

/// Merged view: compiled modules first, then registered sidecars (kind = imported).
async fn list(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<Vec<ModuleManifest>>> {
    principal.require(principal.can_view(), "list modules")?;
    let mut out: Vec<ModuleManifest> = st.modules.as_ref().clone();
    for r in services::modules::list_registered(&st.pool).await? {
        out.push(r.to_manifest());
    }
    Ok(Json(out))
}

/// Register a sidecar plugin. Mints its scoped key + webhook subscription and returns them ONCE.
async fn register(
    State(st): State<AppState>,
    principal: Principal,
    Json(req): Json<ModuleRegisterRequest>,
) -> AppResult<(StatusCode, Json<ModuleRegistered>)> {
    principal.require(principal.can_admin(), "register a module")?;
    let reserved: Vec<String> = st.modules.iter().map(|m| m.id.clone()).collect();
    let (row, api_key, webhook_secret) =
        services::modules::register(&st.pool, req, &reserved).await?;
    auth::audit(
        &st.pool,
        &principal,
        "register_module",
        "module",
        &row.id,
        json!({ "name": row.name, "base_url": row.base_url, "role": row.role }),
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(ModuleRegistered {
            module: ModuleDetail::from(&row),
            api_key,
            webhook_secret,
        }),
    ))
}

/// Admin detail for one registered sidecar (includes its base URL + minted resource ids).
async fn detail(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<ModuleDetail>> {
    principal.require(principal.can_admin(), "view module detail")?;
    let row = services::modules::get_registered(&st.pool, &id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("module `{id}` not found")))?;
    Ok(Json(ModuleDetail::from(&row)))
}

/// Uninstall a sidecar: deletes the row + revokes its key + removes its webhook subscription.
async fn unregister(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    principal.require(principal.can_admin(), "unregister a module")?;
    services::modules::unregister(&st.pool, &id).await?;
    auth::audit(
        &st.pool,
        &principal,
        "unregister_module",
        "module",
        &id,
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

// ------------------------------------------------------------------
// Reverse proxy: /m/{id}/... -> sidecar base_url
// ------------------------------------------------------------------

/// Headers never forwarded in either direction (hop-by-hop + length/host, recomputed by the client).
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "content-length",
    "host",
];

async fn proxy_root(
    State(st): State<AppState>,
    principal: Principal,
    method: Method,
    Path(id): Path<String>,
    uri: OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Response> {
    forward(&st, &principal, &id, "", uri, method, headers, body).await
}

async fn proxy_sub(
    State(st): State<AppState>,
    principal: Principal,
    method: Method,
    Path((id, rest)): Path<(String, String)>,
    uri: OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Response> {
    forward(&st, &principal, &id, &rest, uri, method, headers, body).await
}

#[allow(clippy::too_many_arguments)]
async fn forward(
    st: &AppState,
    principal: &Principal,
    id: &str,
    rest: &str,
    uri: OriginalUri,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Response> {
    principal.require(principal.can_view(), "access a module")?;
    let row = services::modules::get_registered(&st.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("module `{id}` not found")))?;

    let query = uri.0.query().map(|q| format!("?{q}")).unwrap_or_default();
    let target = format!("{}/{}{}", row.base_url, rest, query);

    let mut rb = st.http.request(method, &target);
    for (k, v) in headers.iter() {
        let name = k.as_str().to_ascii_lowercase();
        // Never forward the console session/credentials to a plugin — it authenticates to the kernel
        // with its own minted key, not the user's cookie.
        if HOP_BY_HOP.contains(&name.as_str()) || name == "cookie" || name == "authorization" {
            continue;
        }
        rb = rb.header(k, v);
    }
    if !body.is_empty() {
        rb = rb.body(body.to_vec());
    }
    let resp = rb.send().await.map_err(|e| {
        AppError::Other(anyhow::anyhow!(
            "module `{id}` proxy to {target} failed: {e}"
        ))
    })?;

    let status = resp.status();
    let mut out = Response::builder().status(status);
    for (k, v) in resp.headers().iter() {
        if HOP_BY_HOP.contains(&k.as_str().to_ascii_lowercase().as_str()) {
            continue;
        }
        out = out.header(k, v);
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!("module `{id}` proxy read failed: {e}")))?;
    out.body(Body::from(bytes))
        .map_err(|e| AppError::Other(anyhow::anyhow!("module `{id}` proxy response build: {e}")))
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
    use serde_json::json;
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
            catalog: Arc::new(crate::services::registry::CatalogService::new(&cfg)),
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

    /// Send a request through a fresh router over a clone of `st` (the shared in-memory pool persists).
    async fn send(st: AppState, req: Request<Body>) -> (axum::http::StatusCode, serde_json::Value) {
        let mut app = super::router().with_state(st);
        let res = app.call(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
        };
        (status, json)
    }

    fn post_json(uri: &str, body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn compiled_entry() -> ModuleManifest {
        ModuleManifest::new(
            "entry",
            "Access Control",
            "9.9.9",
            "Heldar",
            ModuleKind::Core,
            "d",
            vec![NavEntry::new("/entry", "Entry", "entry")],
        )
    }

    /// Register mints a scoped key + webhook subscription, the sidecar shows up imported+iframe in the
    /// merged list, and unregister reverses all three.
    #[tokio::test]
    async fn register_list_unregister_lifecycle() {
        let st = state_with(vec![compiled_entry()]).await;

        let (status, json) = send(
            st.clone(),
            post_json(
                "/api/v1/modules",
                json!({
                    "id": "hello",
                    "name": "Hello Plugin",
                    "version": "1.0.0",
                    "publisher": "ACME",
                    "base_url": "http://127.0.0.1:9123",
                    "subscribes": ["zone_enter"],
                    "role": "integration"
                }),
            ),
        )
        .await;
        assert_eq!(status, 201);
        assert!(json["api_key"].as_str().unwrap().starts_with("vok_"));
        assert!(json["webhook_secret"]
            .as_str()
            .unwrap()
            .starts_with("whsec_"));
        assert_eq!(json["module"]["base_url"], "http://127.0.0.1:9123");

        let (_, list) = send(
            st.clone(),
            Request::builder()
                .uri("/api/v1/modules")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        let arr = list.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        let hello = arr.iter().find(|m| m["id"] == "hello").unwrap();
        assert_eq!(hello["kind"], "imported");
        assert_eq!(hello["mount"], "iframe");
        assert_eq!(hello["nav"][0]["path"], "/hello"); // defaulted from id

        // The minted resources exist with least-priv role + the derived webhook URL.
        let role: Option<String> =
            sqlx::query_scalar("SELECT role FROM api_keys WHERE name = 'module:hello'")
                .fetch_optional(&st.pool)
                .await
                .unwrap();
        assert_eq!(role.as_deref(), Some("integration"));
        let url: Option<String> =
            sqlx::query_scalar("SELECT url FROM webhook_subscriptions WHERE name = 'module:hello'")
                .fetch_optional(&st.pool)
                .await
                .unwrap();
        assert_eq!(url.as_deref(), Some("http://127.0.0.1:9123/heldar/events"));

        let (status, _) = send(
            st.clone(),
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/modules/hello")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, 204);

        let (_, list) = send(
            st.clone(),
            Request::builder()
                .uri("/api/v1/modules")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(list.as_array().unwrap().len(), 1);
        let key_gone: Option<String> =
            sqlx::query_scalar("SELECT id FROM api_keys WHERE name = 'module:hello'")
                .fetch_optional(&st.pool)
                .await
                .unwrap();
        assert!(key_gone.is_none());
        let wh_gone: Option<String> =
            sqlx::query_scalar("SELECT id FROM webhook_subscriptions WHERE name = 'module:hello'")
                .fetch_optional(&st.pool)
                .await
                .unwrap();
        assert!(wh_gone.is_none());
    }

    /// A sidecar may not claim a compiled module's id.
    #[tokio::test]
    async fn rejects_reserved_id() {
        let st = state_with(vec![compiled_entry()]).await;
        let (status, _) = send(
            st,
            post_json(
                "/api/v1/modules",
                json!({ "id": "entry", "name": "x", "base_url": "http://127.0.0.1:1" }),
            ),
        )
        .await;
        assert_eq!(status, 409);
    }

    /// Plugin keys are least-privilege: admin/manager/guard are not grantable.
    #[tokio::test]
    async fn rejects_privileged_role() {
        let st = state_with(vec![]).await;
        let (status, _) = send(
            st,
            post_json(
                "/api/v1/modules",
                json!({ "id": "x", "name": "x", "base_url": "http://127.0.0.1:1", "role": "admin" }),
            ),
        )
        .await;
        assert_eq!(status, 400);
    }
}
