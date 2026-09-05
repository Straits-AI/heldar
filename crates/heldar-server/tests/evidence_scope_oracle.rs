//! A camera-scoped credential must not learn about incidents on cameras it does not hold (#118).
//!
//! `POST /api/v1/evidence/exports` accepts an `incident_id` and DERIVES the camera from the
//! incident's segments. The derived id then reached the ordinary caller-supplied scope check, whose
//! refusal names the camera — so a scoped caller could probe incident ids and read back:
//!
//!   * whether the incident exists at all (404 vs 403),
//!   * which camera it is tagged to (named in the 403 body),
//!   * and how many cameras it spans (400 "spans N cameras").
//!
//! Footage never leaked — every gatherer in `services::evidence` binds `camera_id` alongside the
//! time range — but the fleet roster did, one probe at a time. `AppState::resource_camera`'s doc
//! comment already forbids this exact shape, and `GET /evidence/exports/{id}` already gets it right
//! by collapsing out-of-scope to the same 404 as nonexistent. This path was the one that did not.
//!
//! These tests compare refusals BYTE FOR BYTE rather than by status code alone: two 403s with
//! different messages are still an oracle.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::{Duration, Utc};
use heldar_kernel::auth::{Principal, Scope};
use heldar_kernel::routes::evidence;
use heldar_kernel::state::AppState;

async fn state() -> AppState {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    heldar_kernel::db::run_migrations(&pool).await.unwrap();
    let dir = std::env::temp_dir().join(format!("heldar_evo_{}", uuid::Uuid::new_v4().simple()));
    let mut cfg = heldar_kernel::config::Config::from_env();
    cfg.auth_enabled = true;
    cfg.data_dir = dir.clone();
    cfg.recordings_dir = dir.join("recordings");
    cfg.evidence_dir = dir.join("evidence");
    std::fs::create_dir_all(&cfg.recordings_dir).unwrap();
    std::fs::create_dir_all(&cfg.evidence_dir).unwrap();
    let cfg = Arc::new(cfg);
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
        consumers: Arc::new(Vec::new()),
        modules: Arc::new(Vec::new()),
        catalog: Arc::new(heldar_kernel::services::registry::CatalogService::new(&cfg)),
        http: heldar_kernel::reqwest::Client::new(),
        media_jobs: heldar_kernel::services::media_jobs::MediaJobGovernor::new(2),
        started_at: Utc::now(),
        pool,
        cfg,
    }
}

/// A camera with one segment, optionally tagged to an incident. No media: every case here is
/// refused before anything reads a file, and requiring ffmpeg would make a security test skippable.
async fn seed(st: &AppState, camera: &str, seg: &str, incident: Option<&str>) {
    let now = Utc::now();
    sqlx::query(
        "INSERT OR IGNORE INTO cameras (id, name, created_at, updated_at) VALUES (?,?,?,?)",
    )
    .bind(camera)
    .bind(camera)
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO segments (id, camera_id, path, start_time, end_time, duration_s, codec,
             size_bytes, container, created_at, incident_id)
         VALUES (?,?,?,?,?,?, 'h264', 1024, 'mp4', ?, ?)",
    )
    .bind(seg)
    .bind(camera)
    .bind(format!("/nonexistent/{seg}.mp4"))
    .bind(now - Duration::seconds(60))
    .bind(now)
    .bind(60.0_f64)
    .bind(now)
    .bind(incident)
    .execute(&st.pool)
    .await
    .unwrap();
}

/// Admin caps, camera scope — only `scope` differs from the synthetic admin, so any difference in
/// outcome is attributable to the scope and nothing else. `video:export` is not an unscopable
/// capability, so this combination is one the API can really mint.
fn scoped_to(cameras: &[&str]) -> Principal {
    let set: HashSet<String> = cameras.iter().map(|c| c.to_string()).collect();
    Principal {
        scope: Scope::Cameras(Arc::new(set)),
        ..Principal::system_admin()
    }
}

fn body(incident: &str) -> serde_json::Value {
    let now = Utc::now();
    serde_json::json!({
        "incident_id": incident,
        "from": (now - Duration::seconds(60)).to_rfc3339(),
        "to": now.to_rfc3339(),
        "dry_run": true,
    })
}

/// Every refusal, rendered exactly as the caller would see it.
async fn refusal(st: &AppState, p: &Principal, incident: &str) -> String {
    let req = serde_json::from_value(body(incident)).unwrap();
    match evidence::create(axum::extract::State(st.clone()), p.clone(), axum::Json(req)).await {
        Ok(_) => "OK".to_string(),
        Err(e) => {
            let r = axum::response::IntoResponse::into_response(e);
            let status = r.status();
            let bytes = axum::body::to_bytes(r.into_body(), 1 << 16).await.unwrap();
            format!("{status} {}", String::from_utf8_lossy(&bytes))
        }
    }
}

#[tokio::test]
async fn a_scoped_caller_cannot_tell_these_four_cases_apart() {
    let st = state().await;
    seed(&st, "cam_a", "seg_a", Some("inc_mine")).await;
    seed(&st, "cam_b", "seg_b", Some("inc_theirs")).await;
    // An incident spanning one camera they hold and one they do not.
    seed(&st, "cam_a", "seg_m1", Some("inc_mixed")).await;
    seed(&st, "cam_c", "seg_m2", Some("inc_mixed")).await;

    let p = scoped_to(&["cam_a"]);
    let nonexistent = refusal(&st, &p, "inc_does_not_exist").await;
    let theirs = refusal(&st, &p, "inc_theirs").await;
    let mixed = refusal(&st, &p, "inc_mixed").await;

    assert_eq!(
        nonexistent, theirs,
        "an incident on another camera is distinguishable from one that does not exist"
    );
    assert_eq!(
        nonexistent, mixed,
        "an incident spanning a camera they do not hold is distinguishable from a nonexistent one"
    );

    // ...and the refusal itself must carry none of what the caller was fishing for.
    for probe in [
        "cam_b",
        "cam_c",
        "inc_theirs",
        "inc_mixed",
        "inc_does_not_exist",
        "spans",
        "2",
    ] {
        assert!(
            !nonexistent.contains(probe),
            "the refusal leaks {probe:?}: {nonexistent}"
        );
    }
}

/// The fix must not cost a scoped caller access to their OWN incidents.
#[tokio::test]
async fn a_scoped_caller_still_reaches_an_incident_on_a_camera_they_hold() {
    let st = state().await;
    seed(&st, "cam_a", "seg_a", Some("inc_mine")).await;
    let got = refusal(&st, &scoped_to(&["cam_a"]), "inc_mine").await;
    assert!(
        !got.starts_with("403"),
        "a scoped caller was refused their own incident: {got}"
    );
}

/// Holding every camera an incident spans, the count is not new information — so the useful message
/// survives. A blanket refusal would have made the fix cost more than it bought.
#[tokio::test]
async fn holding_every_camera_the_incident_spans_still_gets_the_useful_message() {
    let st = state().await;
    seed(&st, "cam_a", "seg_1", Some("inc_both")).await;
    seed(&st, "cam_b", "seg_2", Some("inc_both")).await;
    let got = refusal(&st, &scoped_to(&["cam_a", "cam_b"]), "inc_both").await;
    assert!(
        got.starts_with("400"),
        "expected the multi-camera message: {got}"
    );
    assert!(got.contains("spans 2 cameras"), "{got}");
}

/// An unscoped principal holds every camera, so nothing in these messages is new to them. Keeping
/// them specific is what makes the box usable for the operator who is allowed to see everything.
#[tokio::test]
async fn an_unscoped_caller_keeps_the_specific_messages() {
    let st = state().await;
    seed(&st, "cam_b", "seg_b", Some("inc_theirs")).await;
    seed(&st, "cam_a", "seg_1", Some("inc_both")).await;
    seed(&st, "cam_c", "seg_2", Some("inc_both")).await;
    let p = Principal::system_admin();

    let missing = refusal(&st, &p, "inc_nope").await;
    assert!(missing.starts_with("404"), "{missing}");
    assert!(missing.contains("inc_nope"), "{missing}");

    let spans = refusal(&st, &p, "inc_both").await;
    assert!(
        spans.starts_with("400") && spans.contains("spans 2 cameras"),
        "{spans}"
    );
}
