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

use crate::auth::{self, Cap, Principal};
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
    principal.require_cap(Cap::SystemRead, "list modules")?;
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

/// Response headers a sidecar may NOT project onto the console origin.
///
/// The proxy makes a plugin single-origin with the dashboard, which means any header the sidecar sets
/// is interpreted by the browser as coming from the console itself. These carry authority over the
/// WHOLE origin, not just the proxied response, so a compromised or malicious plugin must not be able
/// to set them:
///   - `set-cookie`/`set-cookie2`: would let a plugin write cookies on the console origin — overwrite
///     or fixate the session cookie, or shadow it with a same-named cookie on a different path.
///   - `clear-site-data`: would let a plugin wipe the console's cookies/storage (forced logout).
///   - `strict-transport-security`: origin-wide, and persists long after the plugin is uninstalled.
///   - `alt-svc`: redirects the origin's future traffic to a host the plugin chooses.
///   - `www-authenticate`: pops a browser credential prompt that appears to come from the console.
///   - `access-control-allow-*`: would let a plugin relax CORS on the console origin so a third-party
///     site can read proxied responses with the user's session; it also duplicates the headers the
///     kernel's own CORS layer sets, which browsers reject outright.
///   - `public-key-pins`: dead in browsers, but origin-wide and permanently bricking where honoured.
const FORBIDDEN_RESPONSE_HEADERS: &[&str] = &[
    "set-cookie",
    "set-cookie2",
    "clear-site-data",
    "strict-transport-security",
    "alt-svc",
    "www-authenticate",
    "public-key-pins",
    "public-key-pins-report-only",
];

/// Cap on a single proxied sidecar response. Everything served through `/m/{id}/*` is plugin UI assets
/// (HTML/JS/CSS/images) or JSON, so 8 MiB is generous for a legitimate plugin; without a cap a
/// malicious or wedged sidecar can stream unbounded bytes and OOM the kernel process, which takes the
/// recorder and every camera down with it. The body is accumulated incrementally and abandoned the
/// moment it would cross this line — reading first and checking the length afterwards would already
/// have allocated the memory the cap exists to bound.
const MAX_PROXY_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// Timeout for one proxied request to a sidecar.
const PROXY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

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
    principal.require_cap(Cap::ModuleProxy, "access a module")?;
    let row = services::modules::get_registered(&st.pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("module `{id}` not found")))?;

    let query = uri.0.query().map(|q| format!("?{q}")).unwrap_or_default();
    let target = format!("{}/{}{}", row.base_url, rest, query);

    // Resolve-validate-PIN the sidecar at REQUEST time, exactly like the health poller does.
    // `base_url` is screened once at registration and re-checked every 30s by the health sweep, but the
    // proxy used to issue the real request through the shared, unpinned client — so a hostname
    // `base_url` only had to look benign at registration/health-check time and could re-resolve to the
    // cloud metadata endpoint or an internal service by the time a user hit `/m/{id}`. That is the exact
    // TOCTOU the pinning exists to close. `EgressPolicy::LAN` because sidecars legitimately live on
    // loopback/the LAN; the link-local/metadata and unspecified ranges are refused under LAN too.
    let policy = crate::net_guard::EgressPolicy::LAN;
    let parsed = crate::net_guard::validate_egress_url(&target, &policy).map_err(|e| {
        tracing::warn!(module = %id, target = %target, error = %e, "modules: proxy target rejected by the egress guard");
        AppError::Unavailable(format!("module `{id}` proxy target was rejected by the egress guard"))
    })?;
    let client = crate::net_guard::resolve_validate_pin(&parsed, &policy, PROXY_TIMEOUT)
        .await
        .map_err(|e| {
            tracing::warn!(module = %id, target = %target, error = %e, "modules: proxy target rejected by the egress guard");
            AppError::Unavailable(format!("module `{id}` proxy target was rejected by the egress guard"))
        })?;

    let mut rb = client.request(method, parsed.clone());
    for (k, v) in headers.iter() {
        let name = k.as_str().to_ascii_lowercase();
        // Never forward the console session/credentials to a plugin — it authenticates to the kernel
        // with its own minted key, not the user's cookie. Also drop any client-supplied `x-heldar-*`
        // header so a caller can't SPOOF the identity headers the kernel adds below.
        if HOP_BY_HOP.contains(&name.as_str())
            || name == "cookie"
            || name == "authorization"
            || name.starts_with("x-heldar-")
        {
            continue;
        }
        rb = rb.header(k, v);
    }
    // Propagate the AUTHENTICATED caller's identity + role so the sidecar can enforce its OWN
    // authorization across the proxy boundary. Previously the kernel stripped the caller's credentials
    // but added nothing, so every proxied request reached the sidecar unattributed and `can_view`
    // (true for every authenticated principal, including a viewer or another plugin's integration key)
    // was the only gate — the sidecar could not tell who was calling or with what role. These headers
    // are authoritative (the client-supplied ones were dropped just above).
    rb = rb
        .header("x-heldar-user", principal.id.as_str())
        .header("x-heldar-role", principal.role.as_str())
        .header(
            "x-heldar-principal-kind",
            match principal.kind {
                crate::auth::PrincipalKind::User => "user",
                crate::auth::PrincipalKind::ApiKey => "api_key",
                crate::auth::PrincipalKind::System => "system",
            },
        );
    if !body.is_empty() {
        rb = rb.body(body.to_vec());
    }
    let mut resp = rb.send().await.map_err(|e| {
        AppError::Other(anyhow::anyhow!(
            "module `{id}` proxy to {target} failed: {e}"
        ))
    })?;

    let status = resp.status();
    let mut out = Response::builder().status(status);
    for (k, v) in resp.headers().iter() {
        let name = k.as_str().to_ascii_lowercase();
        if HOP_BY_HOP.contains(&name.as_str())
            || FORBIDDEN_RESPONSE_HEADERS.contains(&name.as_str())
            || name.starts_with("access-control-allow-")
        {
            continue;
        }
        out = out.header(k, v);
    }

    // Accumulate incrementally so an oversized body is abandoned mid-stream instead of being buffered
    // in full and measured afterwards (see MAX_PROXY_RESPONSE_BYTES). Dropping `resp` closes the
    // connection, so the sidecar stops sending.
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!("module `{id}` proxy read failed: {e}")))?
    {
        push_capped(&mut buf, &chunk, MAX_PROXY_RESPONSE_BYTES).map_err(|e| {
            tracing::warn!(module = %id, target = %target, "modules: {e}");
            AppError::Unavailable(format!(
                "module `{id}` response exceeded the {MAX_PROXY_RESPONSE_BYTES} byte proxy limit"
            ))
        })?;
    }

    out.body(Body::from(buf))
        .map_err(|e| AppError::Other(anyhow::anyhow!("module `{id}` proxy response build: {e}")))
}

/// Append `chunk` to `buf` unless doing so would exceed `limit`. Checked BEFORE the copy so the
/// oversized bytes are never allocated. Split out from [`forward`] so the cap is unit-testable without
/// standing up a sidecar.
fn push_capped(buf: &mut Vec<u8>, chunk: &[u8], limit: usize) -> Result<(), String> {
    if buf.len().saturating_add(chunk.len()) > limit {
        return Err(format!(
            "response exceeded the {limit} byte proxy limit (aborted after {} bytes)",
            buf.len()
        ));
    }
    buf.extend_from_slice(chunk);
    Ok(())
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
            live: crate::services::live_publisher::LivePublisherManager::new(
                pool.clone(),
                cfg.clone(),
                reqwest::Client::new(),
            ),
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

    // ---------------- reverse proxy: egress guard, header hygiene, response cap ----------------

    /// The response-size cap is enforced BEFORE the bytes are copied, so an oversized body is never
    /// allocated. Unit-testable without a sidecar.
    #[test]
    fn push_capped_aborts_before_allocating_past_the_limit() {
        let mut buf = Vec::new();
        assert!(super::push_capped(&mut buf, b"hello", 10).is_ok());
        assert!(super::push_capped(&mut buf, b"world", 10).is_ok()); // exactly at the limit is fine
        assert_eq!(buf.len(), 10);
        // The next byte would cross it: rejected, and nothing was appended.
        assert!(super::push_capped(&mut buf, b"!", 10).is_err());
        assert_eq!(buf.len(), 10);
        // A single oversized chunk is refused outright rather than partially buffered.
        let mut buf = Vec::new();
        assert!(super::push_capped(&mut buf, &[0u8; 64], 10).is_err());
        assert!(buf.is_empty());
    }

    /// A stand-in sidecar on loopback: one small response carrying headers a plugin must not be able to
    /// project onto the console origin, and one response larger than the proxy cap.
    async fn spawn_sidecar() -> (String, tokio::task::JoinHandle<()>) {
        use axum::http::header;
        use axum::response::IntoResponse;
        use axum::routing::get;
        let app = axum::Router::new()
            .route(
                "/small",
                get(|| async {
                    (
                        [
                            (header::SET_COOKIE, "heldar_session=attacker; Path=/"),
                            (header::STRICT_TRANSPORT_SECURITY, "max-age=63072000"),
                            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "https://evil.example"),
                        ],
                        "ok",
                    )
                        .into_response()
                }),
            )
            .route(
                "/big",
                get(|| async {
                    Body::from(vec![b'x'; super::MAX_PROXY_RESPONSE_BYTES + 1]).into_response()
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), handle)
    }

    /// Send a request through a fresh router and keep the raw response (headers included).
    async fn send_raw(st: AppState, req: Request<Body>) -> axum::http::Response<Body> {
        let mut app = super::router().with_state(st);
        app.call(req).await.unwrap()
    }

    /// The proxy strips the response headers that would give a plugin authority over the console origin
    /// (it is single-origin with the dashboard), and refuses a response larger than the cap instead of
    /// buffering it into kernel memory.
    #[tokio::test]
    async fn proxy_strips_origin_authority_headers_and_caps_response_size() {
        let (origin, server) = spawn_sidecar().await;
        let st = state_with(vec![]).await;
        let (status, _) = send(
            st.clone(),
            post_json(
                "/api/v1/modules",
                json!({ "id": "side", "name": "Side", "base_url": origin, "role": "viewer" }),
            ),
        )
        .await;
        assert_eq!(status, 201);

        let res = send_raw(
            st.clone(),
            Request::builder()
                .uri("/m/side/small")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(res.status(), 200);
        assert!(
            res.headers().get("set-cookie").is_none(),
            "a plugin must not be able to set cookies on the console origin"
        );
        assert!(res.headers().get("strict-transport-security").is_none());
        assert!(res.headers().get("access-control-allow-origin").is_none());
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"ok");

        let res = send_raw(
            st.clone(),
            Request::builder()
                .uri("/m/side/big")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(res.status(), 503);
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            json["error"].as_str().unwrap().contains("proxy limit"),
            "expected the size cap to be the reason, got {json}"
        );

        server.abort();
    }

    /// The SSRF guard runs in the PROXY path, at request time — not only at registration and in the
    /// health poller. A `base_url` that was benign when it was screened but now points at the cloud
    /// metadata endpoint must be refused before any connection is attempted.
    #[tokio::test]
    async fn proxy_re_validates_the_target_at_request_time() {
        let st = state_with(vec![]).await;
        let (status, _) = send(
            st.clone(),
            post_json(
                "/api/v1/modules",
                json!({ "id": "drift", "name": "Drift", "base_url": "http://127.0.0.1:9123", "role": "viewer" }),
            ),
        )
        .await;
        assert_eq!(status, 201);
        // Simulate the TOCTOU window: the stored origin now resolves somewhere forbidden.
        sqlx::query("UPDATE module_registrations SET base_url = ? WHERE id = 'drift'")
            .bind("http://169.254.169.254")
            .execute(&st.pool)
            .await
            .unwrap();

        let res = send_raw(
            st.clone(),
            Request::builder()
                .uri("/m/drift/latest/meta-data/")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(res.status(), 503);
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(json["error"].as_str().unwrap().contains("egress guard"));
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
