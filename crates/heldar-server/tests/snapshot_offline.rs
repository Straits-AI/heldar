//! An offline camera is not a broken box (#168).
//!
//! `GET /api/v1/cameras/{id}/snapshot` returned **500** when the camera's stream was unreachable.
//! An offline camera is the most expected operating condition an NVR has, and 500 says "this box is
//! broken" when the box is fine.
//!
//! The cost is not tidiness. With both cases as 500 a monitor cannot separate "a camera is down"
//! from "the recorder is failing", so the alert that matters is buried under camera churn — and any
//! availability figure computed from 5xx on a site with flaky cameras is measuring the cameras. The
//! qualification harness had to special-case it for exactly that reason.

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

fn have_ffmpeg(bin: &str) -> bool {
    std::process::Command::new(bin)
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// A camera whose stream nothing is serving: loopback on a port with no listener, so the connection
/// is REFUSED immediately rather than waiting out the 10s RTSP timeout.
async fn add_unreachable_camera(st: &AppState, id: &str) {
    let now = chrono::Utc::now();
    sqlx::query(
        "INSERT INTO cameras (id, name, vendor, main_stream_url, record_stream, created_at, updated_at)
         VALUES (?,?,'generic',?,'main',?,?)",
    )
    .bind(id)
    .bind(id)
    .bind("rtsp://127.0.0.1:1/nothing-is-listening")
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn a_camera_that_cannot_be_reached_is_503_not_500() {
    let st = state().await;
    if !have_ffmpeg(&st.cfg.ffmpeg_bin) {
        eprintln!("skipping: ffmpeg not installed");
        return;
    }
    add_unreachable_camera(&st, "cam_down").await;

    let mut app = heldar_kernel::routes::api_router().with_state(st.clone());
    let resp = app
        .call(
            Request::builder()
                .uri("/api/v1/cameras/cam_down/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "an unreachable camera answered {status}; 500 tells a monitor the BOX is broken when the \
         box is fine, and buries the alert that matters under camera churn. Body: {body}"
    );
    assert_eq!(
        body["retryable"].as_bool(),
        Some(true),
        "a camera that is momentarily unreachable is worth retrying: {body}"
    );
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("cam_down"),
        "the error should name the camera an operator has to go and look at: {body}"
    );
}

/// The camera's credentials must not travel in the error, however the failure is classified.
///
/// The 500 path masked the RTSP URL before returning it. Reclassifying to 503 moved that code, and
/// an error body is a far more visible place than a log line — a client displays it.
#[tokio::test]
async fn the_failure_does_not_leak_the_stream_credentials() {
    let st = state().await;
    if !have_ffmpeg(&st.cfg.ffmpeg_bin) {
        eprintln!("skipping: ffmpeg not installed");
        return;
    }
    let now = chrono::Utc::now();
    sqlx::query(
        "INSERT INTO cameras (id, name, vendor, main_stream_url, record_stream, created_at, updated_at)
         VALUES ('cam_secret','cam_secret','generic',?,'main',?,?)",
    )
    .bind("rtsp://admin:hunter2@127.0.0.1:1/stream")
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await
    .unwrap();

    let mut app = heldar_kernel::routes::api_router().with_state(st.clone());
    let resp = app
        .call(
            Request::builder()
                .uri("/api/v1/cameras/cam_secret/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&bytes);
    assert!(
        !body.contains("hunter2"),
        "the camera's password reached the error body: {body}"
    );
}
