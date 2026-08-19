//! Camera confinement inside the structured executor, driven against a real pool.
//!
//! `routes.rs` confines `plan.cameras` before it executes, and every unit test of that confinement
//! stops at the field's value. Nobody had run the EXECUTOR with a confined plan against a table the
//! caller does not own most of — which is where the confinement stops being a filter and starts being
//! a page.
//!
//! The executor fetches each source with `ORDER BY <ts> DESC LIMIT fetch_cap` and applies the camera
//! filter afterwards in Rust. `fetch_cap` therefore bounds ROWS EXAMINED, not rows returned, so newer
//! rows from cameras the caller does not hold consume the page and the caller's own in-window matches
//! never reach the filter. There is no offset or cursor on these routes, so past the cap the caller's
//! own rows are unreachable by any query the API accepts.
//!
//! The same ordering makes `truncated` dishonest in the other direction: it was raised from the
//! UNFILTERED row count, so `truncated: true` beside `count: 0` reported the FLEET's in-window volume
//! to a caller confined to one camera — a bit about cameras it does not hold, differencable over a
//! swept window into the fleet's event-rate profile.
//!
//! Both are asserted here through `query::execute` with an explicit `max`, so `fetch_cap` is a
//! constant of the test rather than of the deployment's `HELDAR_SEARCH_MAX_RESULTS`.

use chrono::{TimeDelta, Utc};
use heldar_search::query::{execute, QueryPlan};
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

/// `execute(_, _, max)` computes `fetch_cap = (max * 5).clamp(100, 20_000)`, so any small `max` gives
/// the floor. 100 is the page every assertion below is sized against.
const MAX: i64 = 20;
const FETCH_CAP: usize = 100;

async fn pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory sqlite");
    heldar_kernel::db::run_migrations(&pool).await.unwrap();
    pool
}

/// A zone event on `camera`. `zone_events` is a KERNEL table, so this test needs no app crate other
/// than the one under test.
async fn zone_event(pool: &SqlitePool, id: &str, camera: &str, ts: chrono::DateTime<Utc>) {
    sqlx::query(
        "INSERT INTO zone_events (id, camera_id, zone_id, zone_name, event_type, timestamp, created_at)
         VALUES (?, ?, 'zn_1', 'dock', 'enter', ?, ?)",
    )
    .bind(id)
    .bind(camera)
    .bind(ts)
    .bind(ts)
    .execute(pool)
    .await
    .unwrap();
}

fn plan_for(cameras: &[&str]) -> QueryPlan {
    QueryPlan {
        cameras: cameras.iter().map(|c| c.to_string()).collect(),
        sources: vec!["zone".to_string()],
        ..QueryPlan::default()
    }
}

/// THE CROWD-OUT. One in-window row on the caller's own camera, then a full page of newer rows on a
/// camera it does not hold.
#[tokio::test]
async fn a_confined_callers_own_rows_survive_a_page_full_of_other_cameras_rows() {
    let pool = pool().await;
    let now = Utc::now();
    // Oldest, so every fleet row below is newer and wins the `ORDER BY timestamp DESC` page.
    zone_event(
        &pool,
        "ze_own",
        "cam_own",
        now - TimeDelta::try_hours(6).unwrap(),
    )
    .await;
    for i in 0..(FETCH_CAP + 20) {
        zone_event(
            &pool,
            &format!("ze_other_{i}"),
            "cam_SENTINEL_B",
            now - TimeDelta::try_seconds(i as i64).unwrap(),
        )
        .await;
    }

    let out = execute(&pool, &plan_for(&["cam_own"]), MAX).await.unwrap();
    // Both halves of the same defect, asserted together so a pre-fix run reports both observed
    // values: the caller's own row is GONE, and `truncated` is reporting the fleet's volume.
    assert_eq!(
        (out.hits.len(), out.truncated),
        (1, false),
        "(count, truncated) — the caller's own in-window zone event fell off a page filled by \
         cameras it does not hold (fetch cap {FETCH_CAP}, no cursor on this route, so it is \
         unreachable by any query the API accepts), and/or `truncated` was raised from the \
         UNFILTERED fetch, reporting that the FLEET has more than {FETCH_CAP} rows in this window — \
         a fact about cameras the caller does not hold"
    );
    assert_eq!(out.hits[0].id, "ze_own");
    assert_eq!(out.hits[0].camera_id.as_deref(), Some("cam_own"));
}

/// The confinement must not have been replaced by "return everything and hope": a camera the caller
/// did not name must still not appear, and naming nothing must still mean the whole box.
#[tokio::test]
async fn confinement_still_excludes_unnamed_cameras_and_an_empty_list_still_means_all() {
    let pool = pool().await;
    let now = Utc::now();
    zone_event(&pool, "ze_own", "cam_own", now).await;
    zone_event(&pool, "ze_other", "cam_SENTINEL_B", now).await;

    let confined = execute(&pool, &plan_for(&["cam_own"]), MAX).await.unwrap();
    let ids: Vec<&str> = confined.hits.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(ids, vec!["ze_own"]);
    let body = serde_json::to_string(&confined.hits).unwrap();
    assert!(!body.contains("cam_SENTINEL_B"), "{body}");

    // CONSTRAINT: an unscoped caller naming no camera still searches the whole box, exactly as before.
    let all = execute(&pool, &plan_for(&[]), MAX).await.unwrap();
    assert_eq!(all.hits.len(), 2);
}

/// `truncated` must still fire when the CALLER'S OWN rows overflow the page — the honesty signal is
/// meant to say "your answer may be incomplete", and a fix that just hard-codes `false` would satisfy
/// the first test while silently promising completeness it cannot deliver.
#[tokio::test]
async fn truncated_still_fires_when_the_callers_own_rows_overflow_the_page() {
    let pool = pool().await;
    let now = Utc::now();
    for i in 0..(FETCH_CAP + 5) {
        zone_event(
            &pool,
            &format!("ze_own_{i}"),
            "cam_own",
            now - TimeDelta::try_seconds(i as i64).unwrap(),
        )
        .await;
    }
    let out = execute(&pool, &plan_for(&["cam_own"]), MAX).await.unwrap();
    assert!(
        out.truncated,
        "the caller's own rows filled the fetch page, so the result IS non-exhaustive and must say so"
    );
}

/// The camera list is caller-supplied and `planner::sanitize` does not bound it, so pushing it into
/// SQL turns it into SQL VARIABLES. A list past SQLite's ceiling must degrade to the pre-existing Rust
/// filter, not fail the statement: making an unbounded field a bind count is how a filter fix becomes
/// a 500 on a body that used to work.
#[tokio::test]
async fn an_absurd_camera_list_still_answers_correctly() {
    let pool = pool().await;
    let now = Utc::now();
    zone_event(&pool, "ze_own", "cam_own", now).await;
    zone_event(&pool, "ze_other", "cam_SENTINEL_B", now).await;

    // Distinct junk ids (dedup cannot shrink these), far past any variable limit, with the real
    // camera buried in the middle.
    let mut cameras: Vec<String> = (0..60_000).map(|i| format!("cam_junk_{i}")).collect();
    cameras.insert(30_000, "cam_own".to_string());
    let plan = QueryPlan {
        cameras,
        sources: vec!["zone".to_string()],
        ..QueryPlan::default()
    };
    let out = execute(&pool, &plan, MAX)
        .await
        .expect("an over-long camera list must not fail the query");
    let ids: Vec<&str> = out.hits.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["ze_own"],
        "the answer must not change with the list size"
    );

    // A REPEATED id is not an absurd list: dedup keeps the pushdown, so this one still gets the
    // page-eviction guarantee rather than silently falling back.
    let plan = QueryPlan {
        cameras: vec!["cam_own".to_string(); 50_000],
        sources: vec!["zone".to_string()],
        ..QueryPlan::default()
    };
    let out = execute(&pool, &plan, MAX).await.unwrap();
    assert_eq!(
        out.hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
        vec!["ze_own"]
    );
}

/// A row with no camera at all must stay excluded from a confined search. `IN (…)` drops NULL, which
/// is what the Rust filter did (`.unwrap_or(false)`), so pushing the predicate into SQL must not
/// quietly start admitting them.
#[tokio::test]
async fn a_camera_less_row_stays_out_of_a_confined_search() {
    let pool = pool().await;
    let now = Utc::now();
    // `zone_events.camera_id` is NOT NULL, so the camera-less case lives on `entry_events_read`. The
    // view belongs to heldar-entry; stand its columns up directly rather than depend on that crate.
    sqlx::query(
        "CREATE TABLE entry_events_read (
             id TEXT PRIMARY KEY, timestamp TEXT NOT NULL, camera_id TEXT, event_type TEXT NOT NULL,
             plate TEXT, subject TEXT NOT NULL, auth_status TEXT NOT NULL, evidence TEXT NOT NULL,
             direction TEXT NOT NULL, track_id TEXT)",
    )
    .execute(&pool)
    .await
    .unwrap();
    for (id, cam) in [
        ("evt_own", Some("cam_own")),
        ("evt_other", Some("cam_SENTINEL_B")),
        ("evt_none", None),
    ] {
        sqlx::query(
            "INSERT INTO entry_events_read (id, timestamp, camera_id, event_type, plate, subject,
                                            auth_status, evidence, direction)
             VALUES (?, ?, ?, 'anpr', 'ABC123', '{}', 'matched', '{}', 'inbound')",
        )
        .bind(id)
        .bind(now)
        .bind(cam)
        .execute(&pool)
        .await
        .unwrap();
    }

    let plan = QueryPlan {
        cameras: vec!["cam_own".to_string()],
        sources: vec!["entry".to_string()],
        ..QueryPlan::default()
    };
    let out = execute(&pool, &plan, MAX).await.unwrap();
    let ids: Vec<&str> = out.hits.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(ids, vec!["evt_own"], "camera-less rows must stay excluded");

    // ...and an unnamed-camera search still returns all three, camera-less row included.
    let plan_all = QueryPlan {
        sources: vec!["entry".to_string()],
        ..QueryPlan::default()
    };
    assert_eq!(execute(&pool, &plan_all, MAX).await.unwrap().hits.len(), 3);
}
