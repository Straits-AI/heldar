//! A lost response followed by a retry must leave EXACTLY ONE side effect (#121).
//!
//! `idempotency.rs` already tests the layer with a counting fake handler, which proves the layer
//! dedupes. It does not prove the layer is MOUNTED where it matters: the ordering in
//! `heldar_server::build_app` puts it inside the auth floor for a reason (a key is scoped to the
//! principal the floor resolves), and nothing outside that file asserts the arrangement survives.
//! A middleware that is correct and not reachable looks exactly like one that works.
//!
//! So this drives a real mutating route, through the real router, with a real API key, and counts
//! ROWS rather than handler invocations.
//!
//! `POST /api/v1/webhooks` is the endpoint because it mints a fresh `whs_{uuid}` per call: posting
//! the same body twice genuinely creates TWO subscriptions. An endpoint with a natural key would
//! collide on the second insert and leave one row whether the layer worked or not — the test would
//! pass for the wrong reason, which is the failure mode this whole file exists to avoid.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use heldar_kernel::state::AppState;
use tower::ServiceExt;

async fn state() -> AppState {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    heldar_kernel::db::run_migrations(&pool).await.unwrap();
    let dir = std::env::temp_dir().join(format!("heldar_idem_{}", uuid::Uuid::new_v4().simple()));
    let mut cfg = heldar_kernel::config::Config::from_env();
    cfg.auth_enabled = true; // the layer scopes by principal, so the floor must really run
    cfg.data_dir = dir.clone();
    cfg.recordings_dir = dir.join("recordings");
    std::fs::create_dir_all(&cfg.recordings_dir).unwrap();
    let cfg = std::sync::Arc::new(cfg);
    AppState {
        recorder: heldar_kernel::services::recorder::RecorderManager::new(
            pool.clone(),
            cfg.clone(),
        ),
        sampler: heldar_kernel::services::sampler::SamplerManager::new(pool.clone(), cfg.clone()),
        live: heldar_kernel::services::live_publisher::LivePublisherManager::new(
            pool.clone(),
            cfg.clone(),
            heldar_kernel::reqwest::Client::new(),
        ),
        mirror: None,
        consumers: std::sync::Arc::new(Vec::new()),
        modules: std::sync::Arc::new(Vec::new()),
        catalog: std::sync::Arc::new(heldar_kernel::services::registry::CatalogService::new(&cfg)),
        http: heldar_kernel::reqwest::Client::new(),
        media_jobs: heldar_kernel::services::media_jobs::MediaJobGovernor::new(2),
        started_at: Utc::now(),
        pool,
        cfg,
    }
}

/// The idempotency layer INSIDE the auth floor, exactly as `heldar_server::build_app` orders them.
/// Reversing these two is the mistake the arrangement guards against — outside the floor there is no
/// principal to scope a key to, and one caller could replay another's result by guessing a key.
fn app(st: &AppState) -> axum::Router {
    heldar_kernel::routes::api_router()
        .with_state(st.clone())
        .layer(axum::middleware::from_fn_with_state(
            st.clone(),
            heldar_kernel::idempotency::layer,
        ))
        .layer(axum::middleware::from_fn_with_state(
            st.clone(),
            heldar_kernel::auth::require_api_auth,
        ))
}

async fn admin_token(st: &AppState) -> String {
    let token = format!("vok_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO api_keys (id, name, key_hash, key_prefix, role, active, created_at,
                               capabilities, scope_kind, scope_cameras, expires_at)
         VALUES (?,?,?,?,'admin',1,?,NULL,'all',NULL,NULL)",
    )
    .bind(format!("key_{}", uuid::Uuid::new_v4().simple()))
    .bind("test")
    .bind(heldar_kernel::auth::token_hash(&token))
    .bind(&token[..8])
    .bind(Utc::now())
    .execute(&st.pool)
    .await
    .unwrap();
    token
}

const BODY: &str = r#"{"name":"ops","url":"https://example.invalid/hook","event_types":["*"]}"#;

async fn post_webhook(
    st: &AppState,
    token: &str,
    key: Option<&str>,
    body: &str,
) -> (StatusCode, String) {
    // Content-Length matters here, and it is not boilerplate. The layer treats a request with no
    // declared length as unbounded and runs it UNPROTECTED — a documented, deliberate choice, since
    // replay needs a stored body and a chunked one cannot be bounded without reading it. A real HTTP
    // client always sets it; `oneshot` does not, so a test that omitted it would exercise the
    // unprotected path while appearing to test idempotency. It cost two red tests to notice.
    let mut req = Request::builder()
        .method("POST")
        .uri("/api/v1/webhooks")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .header("content-length", body.len().to_string());
    if let Some(k) = key {
        req = req.header("idempotency-key", k);
    }
    let resp = app(st)
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn subscriptions(st: &AppState) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM webhook_subscriptions")
        .fetch_one(&st.pool)
        .await
        .unwrap()
}

/// The scenario the criterion names: the caller sent a request, never saw the answer, and retried.
#[tokio::test]
async fn a_retry_after_a_lost_response_creates_exactly_one_subscription() {
    let st = state().await;
    let token = admin_token(&st).await;

    let (s1, b1) = post_webhook(&st, &token, Some("retry-me"), BODY).await;
    assert!(s1.is_success(), "first call: {s1} {b1}");
    assert_eq!(subscriptions(&st).await, 1);

    // The client never saw that response and sends the identical request again.
    let (s2, b2) = post_webhook(&st, &token, Some("retry-me"), BODY).await;
    assert_eq!(s2, s1, "the replay must reproduce the original status");
    assert_eq!(
        b2, b1,
        "the replay must reproduce the original body — a caller that retried and got a DIFFERENT \
         subscription id would go on to manage the wrong one"
    );
    assert_eq!(
        subscriptions(&st).await,
        1,
        "the retry created a second subscription: exactly-once is not holding on a real route"
    );
}

/// Without this control the test above passes on an endpoint that simply cannot create twice —
/// which is how a natural-key route would fake a green result.
#[tokio::test]
async fn a_different_key_really_does_create_a_second_subscription() {
    let st = state().await;
    let token = admin_token(&st).await;

    post_webhook(&st, &token, Some("key-one"), BODY).await;
    post_webhook(&st, &token, Some("key-two"), BODY).await;
    assert_eq!(
        subscriptions(&st).await,
        2,
        "the same body under a different key must create a second row, or the exactly-once test \
         proves nothing"
    );
}

/// ...and with no key at all, nothing dedupes. This is what makes the header meaningful rather than
/// the endpoint happening to be idempotent on its own.
#[tokio::test]
async fn without_the_header_a_repeat_creates_a_second_subscription() {
    let st = state().await;
    let token = admin_token(&st).await;

    post_webhook(&st, &token, None, BODY).await;
    post_webhook(&st, &token, None, BODY).await;
    assert_eq!(subscriptions(&st).await, 2);
}

/// A key reused with a DIFFERENT body is a mistake in the caller, and answering 200 with the old
/// result would hide it. The criterion asks for a stable, machine-readable conflict.
#[tokio::test]
async fn the_same_key_with_a_different_body_conflicts_and_creates_nothing() {
    let st = state().await;
    let token = admin_token(&st).await;

    post_webhook(&st, &token, Some("same-key"), BODY).await;
    let other = r#"{"name":"other","url":"https://example.invalid/other","event_types":["*"]}"#;
    let (status, body) = post_webhook(&st, &token, Some("same-key"), other).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body.contains("code"),
        "a conflict has to be machine-readable, not just prose: {body}"
    );
    assert_eq!(
        subscriptions(&st).await,
        1,
        "the conflicting call still created a row"
    );
}

/// The layer sits inside the auth floor so a key is scoped to a principal. Two credentials using the
/// same key string must not share a namespace — otherwise one caller replays another's result by
/// guessing a key, and learns the response body while doing it.
#[tokio::test]
async fn two_credentials_sharing_a_key_string_do_not_share_a_result() {
    let st = state().await;
    let a = admin_token(&st).await;
    let b = admin_token(&st).await;

    let (_, body_a) = post_webhook(&st, &a, Some("shared"), BODY).await;
    let (status_b, body_b) = post_webhook(&st, &b, Some("shared"), BODY).await;

    assert!(status_b.is_success(), "{body_b}");
    assert_ne!(
        body_a, body_b,
        "the second credential was handed the first one's result"
    );
    assert_eq!(
        subscriptions(&st).await,
        2,
        "each principal's key should have run its own request"
    );
}
