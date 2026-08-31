//! The correlation id reaches the audit log (#169, part of #121).
//!
//! `request_id` already reached the response header, the tracing span and the evidence manifest. It
//! did not reach `audit_log`, so the join worked in one direction only: a bundle could point at an
//! audit row, and the row could not point back at the request. An operator holding a request id from
//! a client's bug report had no way to ask the box what it actually did.
//!
//! # These tests drive the router WITH the middleware
//!
//! `routes::api_router()` does NOT carry `request_id::layer` — the server applies it in
//! `heldar-server/src/lib.rs`. A test built on the bare router would find the task-local unset, see
//! NULL in every row, and prove nothing at all while looking thorough. The layer is added here in
//! the same position the server uses, and `the_bare_router_has_no_correlation_id` pins that
//! difference so this file cannot quietly drift into testing the wrong stack.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use heldar_kernel::state::AppState;
use tower::Service;

async fn state() -> AppState {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    heldar_kernel::db::run_migrations(&pool).await.unwrap();
    let mut cfg = heldar_kernel::config::Config::from_env();
    cfg.auth_enabled = false;
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
        started_at: chrono::Utc::now(),
        pool,
        cfg,
    }
}

/// The production stack: the kernel routes with the correlation-id layer over them.
async fn call(
    st: &AppState,
    method: &str,
    path: &str,
    body: &str,
    supplied_id: Option<&str>,
) -> (StatusCode, Option<String>, serde_json::Value) {
    let mut app = heldar_kernel::routes::api_router()
        .with_state(st.clone())
        .layer(axum::middleware::from_fn(heldar_kernel::request_id::layer));
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(id) = supplied_id {
        b = b.header("x-request-id", id);
    }
    let resp = app
        .call(b.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let echoed = resp
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let v = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, echoed, v)
}

async fn audit_rows(st: &AppState) -> Vec<(String, Option<String>)> {
    sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT action, request_id FROM audit_log ORDER BY rowid",
    )
    .fetch_all(&st.pool)
    .await
    .unwrap()
}

/// A mutation made over HTTP records the SAME id the caller was handed back.
///
/// The two ends are what make it a correlation id: an operator has the header value and needs the
/// audit row, so the assertion is equality with the echoed header, not merely "something was stored".
#[tokio::test]
async fn a_mutation_records_the_id_the_caller_was_given() {
    let st = state().await;
    let (status, echoed, body) = call(
        &st,
        "POST",
        "/api/v1/cameras",
        r#"{"id":"cam_a","name":"A","vendor":"generic"}"#,
        None,
    )
    .await;
    assert!(status.is_success(), "create camera -> {status}: {body}");
    let echoed = echoed.expect("every response carries x-request-id");
    assert!(echoed.starts_with("req_"), "generated id shape: {echoed}");

    let rows = audit_rows(&st).await;
    assert!(
        !rows.is_empty(),
        "creating a camera wrote no audit row at all"
    );
    for (action, rid) in &rows {
        assert_eq!(
            rid.as_deref(),
            Some(echoed.as_str()),
            "audit row {action:?} did not record the request id the caller was handed"
        );
    }
}

/// A caller-supplied id is what lands, so a trace can span the caller's system and the box.
#[tokio::test]
async fn a_caller_supplied_id_is_what_gets_recorded() {
    let st = state().await;
    let (status, echoed, body) = call(
        &st,
        "POST",
        "/api/v1/cameras",
        r#"{"id":"cam_b","name":"B","vendor":"generic"}"#,
        Some("trace-from-the-caller"),
    )
    .await;
    assert!(status.is_success(), "{status}: {body}");
    assert_eq!(echoed.as_deref(), Some("trace-from-the-caller"));
    let rows = audit_rows(&st).await;
    assert_eq!(rows[0].1.as_deref(), Some("trace-from-the-caller"));
}

/// `audit()` called outside a request records NULL, and that is the correct answer.
///
/// This is the shape every pre-migration row has, and the shape a background writer would have if
/// one is ever added — a task-local does not cross `tokio::spawn`. Nothing background audits today,
/// so the claim is about the mechanism rather than about a caller that exists. NULL is a fact worth
/// reading ("no request carried an id into this row"), not a value to paper over with a synthetic id.
#[tokio::test]
async fn an_act_outside_a_request_records_no_request_id() {
    let st = state().await;
    let actor = heldar_kernel::auth::Principal::system_admin();
    heldar_kernel::auth::audit(
        &st.pool,
        &actor,
        "retention_sweep",
        "system",
        "-",
        serde_json::json!({}),
    )
    .await;
    let rows = audit_rows(&st).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].1, None,
        "a background act invented a request id; NULL is the honest answer"
    );
}

/// Two requests, two ids, and each row carries its own — the property a filter can rely on.
#[tokio::test]
async fn separate_requests_do_not_share_an_id() {
    let st = state().await;
    let (_, one, _) = call(
        &st,
        "POST",
        "/api/v1/cameras",
        r#"{"id":"cam_c","name":"C","vendor":"generic"}"#,
        None,
    )
    .await;
    let (_, two, _) = call(
        &st,
        "POST",
        "/api/v1/cameras",
        r#"{"id":"cam_d","name":"D","vendor":"generic"}"#,
        None,
    )
    .await;
    let (one, two) = (one.unwrap(), two.unwrap());
    assert_ne!(one, two, "two requests were given the same correlation id");

    let ids: Vec<Option<String>> = audit_rows(&st).await.into_iter().map(|(_, r)| r).collect();
    assert!(
        ids.contains(&Some(one.clone())),
        "first request's row missing"
    );
    assert!(
        ids.contains(&Some(two.clone())),
        "second request's row missing"
    );
    assert!(
        !ids.iter().flatten().any(|r| r != &one && r != &two),
        "an audit row carried an id belonging to neither request"
    );
}

/// `?request_id=` returns everything one call did, and nothing another call did.
///
/// This is the operator-facing point of the whole change: someone holding an `x-request-id` from a
/// bug report can ask the box what that call actually performed, instead of grepping logs.
#[tokio::test]
async fn the_filter_returns_one_requests_acts_and_no_others() {
    let st = state().await;
    let mut app = heldar_kernel::routes::api_router()
        .merge(heldar_entry::routes::router())
        .with_state(st.clone())
        .layer(axum::middleware::from_fn(heldar_kernel::request_id::layer));

    async fn post(app: &mut axum::Router, id: &str, cam: &str) {
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/cameras")
            .header("content-type", "application/json")
            .header("x-request-id", id)
            .body(Body::from(format!(
                r#"{{"id":"{cam}","name":"{cam}","vendor":"generic"}}"#
            )))
            .unwrap();
        let resp = app.call(req).await.unwrap();
        assert!(resp.status().is_success(), "{cam} -> {}", resp.status());
    }
    post(&mut app, "call-one", "cam_one").await;
    post(&mut app, "call-two", "cam_two").await;

    let resp = app
        .call(
            Request::builder()
                .uri("/api/v1/audit?request_id=call-one&limit=5000")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let rows: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let rows = rows.as_array().expect("an array of audit rows");

    assert!(
        !rows.is_empty(),
        "the filter returned nothing for a call that audited"
    );
    // These rows are parsed from the API RESPONSE BODY, so the loop below also pins that the column
    // is SERVED and not merely stored — an operator reading the API is the audience.
    for r in rows {
        assert_eq!(
            r["request_id"], "call-one",
            "the filter returned a row belonging to a different call: {r}"
        );
    }
    let body = String::from_utf8_lossy(&bytes);
    assert!(
        body.contains("cam_one") && !body.contains("cam_two"),
        "expected only the first call's acts: {body}"
    );
}

/// THE GUARD ON THIS FILE. `api_router()` alone does not carry the middleware, so a test written
/// against it would see NULL everywhere and pass every assertion above by vacuum.
///
/// If this ever starts failing because the bare router DOES set the id, that is good news — and the
/// helpers above should drop their explicit layer so they stop testing it twice.
#[tokio::test]
async fn the_bare_router_has_no_correlation_id() {
    let st = state().await;
    let mut app = heldar_kernel::routes::api_router().with_state(st.clone());
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/cameras")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"id":"cam_e","name":"E","vendor":"generic"}"#,
        ))
        .unwrap();
    let resp = app.call(req).await.unwrap();
    assert!(resp.status().is_success());
    let rows = audit_rows(&st).await;
    assert!(!rows.is_empty());
    assert!(
        rows.iter().all(|(_, r)| r.is_none()),
        "the bare router now sets a correlation id — the tests above are no longer proving that the \
         middleware is what does it, and should be simplified"
    );
}
