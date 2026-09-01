//! Dry-run and plan hashes for high-impact changes (#121).
//!
//! `PUT /api/v1/system/retention` is the sharpest mutation on the box: the value lands in settings
//! and the sweeper reads it LATER, evicting oldest-first fleet-wide with no principal in scope. An
//! operator who shrinks the cap has already decided how much footage to destroy, whether or not
//! they realise it.
//!
//! These drive the REAL handler, not the hashing helper. A hash function that is deterministic
//! proves nothing about whether the endpoint refuses a stale commit.

use axum::extract::State;
use axum::Json;
use heldar_kernel::state::AppState;

async fn state() -> AppState {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    heldar_kernel::db::run_migrations(&pool).await.unwrap();
    let cfg = std::sync::Arc::new(heldar_kernel::config::Config::from_env());
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

/// Seed `n` bytes of recorded footage across one camera.
async fn seed_footage(st: &AppState, bytes: i64) {
    let now = chrono::Utc::now();
    sqlx::query(
        "INSERT OR IGNORE INTO cameras (id, name, created_at, updated_at) VALUES ('c','c',?,?)",
    )
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO segments (id, camera_id, path, start_time, end_time, duration_s, size_bytes,
             container, created_at)
         VALUES (?, 'c', ?, ?, ?, 10, ?, 'mp4', ?)",
    )
    .bind(format!("seg_{bytes}_{}", uuid::Uuid::new_v4().simple()))
    .bind(format!("/tmp/{bytes}.mp4"))
    .bind(now)
    .bind(now)
    .bind(bytes)
    .bind(now)
    .execute(&st.pool)
    .await
    .unwrap();
}

async fn put(
    st: &AppState,
    body: &str,
) -> Result<serde_json::Value, heldar_kernel::error::AppError> {
    heldar_kernel::routes::system::put_retention(
        State(st.clone()),
        heldar_kernel::auth::Principal::system_admin(),
        Json(serde_json::from_str(body).unwrap()),
    )
    .await
    .map(|Json(v)| v)
}

#[tokio::test]
async fn a_dry_run_says_what_would_be_deleted_and_changes_nothing() {
    let st = state().await;
    seed_footage(&st, 30_000_000_000).await; // ~28 GiB recorded

    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM settings")
        .fetch_one(&st.pool)
        .await
        .unwrap_or(0);

    let plan = put(&st, r#"{"max_recordings_gb": 1.0, "dry_run": true}"#)
        .await
        .expect("dry run");
    assert!(
        plan["effect"]["would_evict_bytes"].as_i64().unwrap_or(0) > 0,
        "shrinking the cap below what is recorded must report an eviction: {plan}"
    );
    assert_eq!(plan["confirmation_required"], true, "{plan}");
    assert!(
        plan["plan_hash"].as_str().unwrap_or("").len() == 64,
        "a dry run must return a hash a commit can present: {plan}"
    );
    assert!(
        plan["note"]
            .as_str()
            .unwrap_or("")
            .contains("delete the oldest footage"),
        "it must say what committing does, in those words: {plan}"
    );

    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM settings")
        .fetch_one(&st.pool)
        .await
        .unwrap_or(0);
    assert_eq!(before, after, "a dry run must write nothing");
}

/// THE POINT OF THE HASH. Between planning and committing, the world moves.
#[tokio::test]
async fn a_commit_is_refused_when_the_state_the_plan_described_has_moved() {
    let st = state().await;
    seed_footage(&st, 10_000_000_000).await;

    let plan = put(&st, r#"{"max_recordings_gb": 5.0, "dry_run": true}"#)
        .await
        .expect("dry run");
    let hash = plan["plan_hash"].as_str().unwrap().to_string();

    // Committing immediately is fine — nothing moved.
    let ok = put(
        &st,
        &format!(r#"{{"max_recordings_gb": 5.0, "plan_hash": "{hash}"}}"#),
    )
    .await;
    assert!(ok.is_ok(), "an unchanged state must commit: {ok:?}");

    // Now the world moves: more footage is recorded, so the plan's "would evict" figure is wrong.
    seed_footage(&st, 40_000_000_000).await;
    let stale = put(
        &st,
        &format!(r#"{{"max_recordings_gb": 5.0, "plan_hash": "{hash}"}}"#),
    )
    .await;
    let err = format!("{:?}", stale.expect_err("a moved state must refuse"));
    assert!(err.contains("out of date"), "{err}");
    assert!(
        err.contains("dry_run"),
        "the refusal must say how to recover: {err}"
    );
}

/// Not planning is allowed. A plan hash is a safety belt for automation, not a way to stop a human
/// with an admin key changing a setting directly — making it mandatory would push people to script
/// around it.
#[tokio::test]
async fn committing_without_a_plan_still_works() {
    let st = state().await;
    seed_footage(&st, 1_000_000_000).await;
    assert!(put(&st, r#"{"max_recordings_gb": 50.0}"#).await.is_ok());
}

/// A wrong hash is refused whether or not anything actually moved — the caller is asserting they
/// saw a specific plan, and they did not.
#[tokio::test]
async fn a_fabricated_hash_is_refused() {
    let st = state().await;
    seed_footage(&st, 1_000_000_000).await;
    let bogus = "0".repeat(64);
    let err = put(
        &st,
        &format!(r#"{{"max_recordings_gb": 50.0, "plan_hash": "{bogus}"}}"#),
    )
    .await
    .expect_err("a hash from no plan at all must not commit");
    assert!(format!("{err:?}").contains("out of date"));
}
