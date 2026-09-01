//! Entry reports and search must agree about what "yesterday" is (#125).
//!
//! `date=YYYY-MM-DD` used to resolve as the UTC day. After search learned to read relative dates in
//! the site's zone, that made "yesterday" in a search and "yesterday" in the daily entry report two
//! different 24 hours on the same box — with nothing in either response to reveal it.

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
    heldar_entry::schema::init(&pool).await.unwrap();
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

async fn get(st: &AppState, path: &str) -> (StatusCode, serde_json::Value) {
    let mut app = heldar_entry::routes::router().with_state(st.clone());
    let req = Request::builder().uri(path).body(Body::empty()).unwrap();
    let resp = app.call(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

/// Seed one entry event at a given UTC instant.
async fn seed_event(st: &AppState, id: &str, at: &str) {
    let ts = chrono::DateTime::parse_from_rfc3339(at)
        .unwrap()
        .with_timezone(&chrono::Utc);
    sqlx::query(
        "INSERT INTO entry_events
           (id, camera_id, event_type, plate, auth_status, timestamp, created_at)
         VALUES (?, 'cam_a', 'anpr', 'ABC123', 'authorized', ?, ?)",
    )
    .bind(id)
    .bind(ts)
    .bind(ts)
    .execute(&st.pool)
    .await
    .unwrap();
}

async fn set_zone(st: &AppState, tz: &str) {
    heldar_kernel::services::settings::set_str(
        &st.pool,
        heldar_kernel::services::tz::DEFAULT_TIMEZONE,
        tz,
    )
    .await
    .unwrap();
}

/// `date=` is a calendar day in the site's zone, not UTC's.
#[tokio::test]
async fn a_reports_calendar_day_is_the_sites_day() {
    let st = state().await;
    // 2026-06-01 23:30 in Kuala Lumpur is 15:30Z the SAME day; 2026-06-02 00:30 KL is 16:30Z on
    // 06-01. Only the second belongs to KL's 2026-06-02.
    seed_event(&st, "e_kl_0601", "2026-06-01T15:30:00Z").await;
    seed_event(&st, "e_kl_0602", "2026-06-01T16:30:00Z").await;

    // Unconfigured: the UTC day. Both instants are 2026-06-01 in UTC.
    let (s, v) = get(&st, "/api/v1/reports/entry-log?date=2026-06-01").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["total"], 2, "both are the UTC day: {v}");
    assert_eq!(v["interpretation"]["timezone"], "UTC");
    assert_eq!(v["interpretation"]["timezone_source"], "utc_fallback");

    set_zone(&st, "Asia/Kuala_Lumpur").await;
    let (_, v) = get(&st, "/api/v1/reports/entry-log?date=2026-06-01").await;
    assert_eq!(
        v["total"], 1,
        "in Kuala Lumpur only the 15:30Z event is 2026-06-01 — the other is already the 2nd: {v}"
    );
    assert_eq!(v["interpretation"]["timezone"], "Asia/Kuala_Lumpur");
    assert_eq!(v["interpretation"]["timezone_source"], "default");
    assert_eq!(
        v["from"], "2026-05-31T16:00:00Z",
        "KL's 2026-06-01 starts at 16:00Z the previous day: {v}"
    );

    let (_, v) = get(&st, "/api/v1/reports/entry-log?date=2026-06-02").await;
    assert_eq!(v["total"], 1, "and the other event is KL's 2nd: {v}");
}

/// An explicit `tz` wins, so a head-office operator can ask for a specific site's day.
#[tokio::test]
async fn an_explicit_tz_overrides_the_resolved_one() {
    let st = state().await;
    seed_event(&st, "e1", "2026-06-01T16:30:00Z").await;
    set_zone(&st, "UTC").await;

    let (_, utc) = get(&st, "/api/v1/reports/entry-log?date=2026-06-01").await;
    assert_eq!(utc["total"], 1);

    let (_, kl) = get(
        &st,
        "/api/v1/reports/entry-log?date=2026-06-01&tz=Asia/Kuala_Lumpur",
    )
    .await;
    assert_eq!(kl["total"], 0, "16:30Z is already the 2nd in KL: {kl}");
    assert_eq!(kl["interpretation"]["timezone_source"], "explicit");

    let (s, _) = get(&st, "/api/v1/reports/entry-log?date=2026-06-01&tz=Asia/KL").await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "a bad zone is refused");
}

/// Absolute `from`/`to` are unambiguous instants, so the zone is irrelevant and they are never
/// refused — including on a box whose sites disagree.
#[tokio::test]
async fn absolute_windows_are_untouched_by_the_zone() {
    let st = state().await;
    seed_event(&st, "e1", "2026-06-01T16:30:00Z").await;
    for tz in ["UTC", "Asia/Kuala_Lumpur", "America/New_York"] {
        set_zone(&st, tz).await;
        let (s, v) = get(
            &st,
            "/api/v1/reports/entry-log?from=2026-06-01T00:00:00Z&to=2026-06-02T00:00:00Z",
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(
            v["total"], 1,
            "an absolute window means the same instants in {tz}: {v}"
        );
        assert_eq!(
            v["interpretation"]["calendar_day_in"],
            serde_json::Value::Null,
            "and it says the zone did not decide the window: {v}"
        );
    }
}

/// Both report endpoints must agree; an exceptions report on a different clock than the entry log
/// is the same divergence one level down.
#[tokio::test]
async fn both_reports_state_the_same_clock() {
    let st = state().await;
    set_zone(&st, "Asia/Kuala_Lumpur").await;
    for path in ["entry-log", "exceptions"] {
        let (s, v) = get(&st, &format!("/api/v1/reports/{path}?date=2026-06-01")).await;
        assert_eq!(s, StatusCode::OK, "{path}: {v}");
        assert_eq!(
            v["interpretation"]["timezone"], "Asia/Kuala_Lumpur",
            "{path} must state its clock: {v}"
        );
        assert_eq!(v["from"], "2026-05-31T16:00:00Z", "{path}: {v}");
    }
}
