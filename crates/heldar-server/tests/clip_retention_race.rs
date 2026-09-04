//! A clip export racing the retention sweeper answers honestly (#183).
//!
//! `export_clip` selects the segments overlapping the window, then hands their paths to ffmpeg's
//! concat demuxer. If retention removes one in between, ffmpeg reports "Impossible to open" and the
//! request became a **500** — telling a monitor the box is broken when the box is fine and the
//! footage is simply past its retention horizon.
//!
//! Same argument as #168 for snapshots: pruned footage is an EXPECTED condition on a recorder.
//!
//! There is a `SegReadLock` meant to prevent this, and the sweeper honours it
//! (`DELETE ... WHERE locked = 0`). It could not close the window on its own for two reasons, both
//! covered below: it was taken AFTER the media-job permit — a semaphore that blocks for as long as
//! the export queue is deep — and a lock cannot protect a row that was already gone when it was
//! taken.

use chrono::{Duration, Utc};
use heldar_kernel::error::AppError;
use heldar_kernel::state::AppState;

async fn state(dir: &std::path::Path) -> AppState {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    heldar_kernel::db::run_migrations(&pool).await.unwrap();
    let mut cfg = heldar_kernel::config::Config::from_env();
    cfg.auth_enabled = false;
    cfg.data_dir = dir.to_path_buf();
    cfg.recordings_dir = dir.join("recordings");
    cfg.clips_dir = dir.join("clips");
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

/// One camera and two adjacent segments whose files exist on disk.
async fn seed(st: &AppState) -> (chrono::DateTime<Utc>, chrono::DateTime<Utc>, Vec<String>) {
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO cameras (id, name, vendor, main_stream_url, record_stream, created_at, updated_at)
         VALUES ('cam_a','cam_a','generic','rtsp://127.0.0.1:1/x','main',?,?)",
    )
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await
    .unwrap();

    let dir = st.cfg.recordings_dir.join("cam_a");
    std::fs::create_dir_all(&dir).unwrap();
    let mut ids = Vec::new();
    let from = now - Duration::seconds(20);
    for i in 0..2i64 {
        let start = from + Duration::seconds(i * 10);
        let end = start + Duration::seconds(10);
        // REAL media, so the success path is real. A placeholder byte string makes every test
        // here pass or fail on ffmpeg rejecting the fixture rather than on the behaviour under test.
        let path = dir.join(format!("seg{i}.mp4"));
        let ok = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=160x120:rate=5",
                "-t",
                "10",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
                "-y",
            ])
            .arg(&path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok, "could not build the fixture segment with ffmpeg");
        let id = format!("seg_{i}");
        sqlx::query(
            "INSERT INTO segments (id, camera_id, path, start_time, end_time, duration_s, size_bytes, created_at)
             VALUES (?,?,?,?,?,10.0,14,?)",
        )
        .bind(&id)
        .bind("cam_a")
        .bind(path.to_string_lossy().to_string())
        .bind(start)
        .bind(end)
        .bind(now)
        .execute(&st.pool)
        .await
        .unwrap();
        ids.push(id);
    }
    (from, now, ids)
}

fn kind(e: &AppError) -> &'static str {
    match e {
        AppError::NotFound(_) => "NotFound",
        AppError::BadRequest(_) => "BadRequest",
        AppError::Conflict(_) => "Conflict",
        AppError::Unavailable(_) => "Unavailable",
        AppError::Other(_) => "Other(500)",
        _ => "other",
    }
}

/// Footage pruned BEFORE the request is not an error at all — the export covers what survives.
///
/// This is the boundary that makes the failing cases meaningful. Retention removing old footage is
/// the system working, so an export whose window is partly past the horizon must still return the
/// part that exists, with the missing span reported as a gap. If this were an error too, the fix
/// below would just be refusing to export near the retention horizon.
#[tokio::test]
async fn footage_pruned_before_the_request_still_exports_what_remains() {
    require_ffmpeg();
    let dir = tempdir();
    let st = state(&dir).await;
    let (from, to, ids) = seed(&st).await;

    // Exactly what retention does, in its order: delete the row, then unlink the file.
    let path: String = sqlx::query_scalar("SELECT path FROM segments WHERE id = ?")
        .bind(&ids[0])
        .fetch_one(&st.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM segments WHERE id = ?")
        .bind(&ids[0])
        .execute(&st.pool)
        .await
        .unwrap();
    let _ = std::fs::remove_file(&path);

    let out = heldar_kernel::services::clip::export_clip(&st, "cam_a", from, to)
        .await
        .expect("the surviving half of the window must still export");
    assert!(
        !out.gaps.is_empty(),
        "the pruned span must be reported as a gap rather than passed off as continuous footage"
    );
}

/// The row survived but the file did not — retention deletes the row FIRST, so an interrupted sweep
/// leaves exactly this shape, and it is what ffmpeg's "Impossible to open" actually was.
#[tokio::test]
async fn an_indexed_segment_whose_file_is_gone_is_not_a_500() {
    require_ffmpeg();
    let dir = tempdir();
    let st = state(&dir).await;
    let (from, to, ids) = seed(&st).await;

    let path: String = sqlx::query_scalar("SELECT path FROM segments WHERE id = ?")
        .bind(&ids[1])
        .fetch_one(&st.pool)
        .await
        .unwrap();
    std::fs::remove_file(&path).unwrap();

    let err = heldar_kernel::services::clip::export_clip(&st, "cam_a", from, to)
        .await
        .expect_err("a segment indexed without its file must fail");
    assert_eq!(kind(&err), "NotFound", "got: {err}");
    let msg = format!("{err}");
    assert!(
        msg.contains(&ids[1]),
        "the error should name the segment an operator has to go and look at: {msg}"
    );
}

/// The under-lock re-read, tested directly.
///
/// The interleaving it guards — the sweeper deleting a row between the export's SELECT and its read
/// lock — cannot be produced on demand without a hook inside `export_clip`. So the CHECK is tested
/// rather than the race, and this comment is here so nobody later mistakes one for the other.
///
/// This exists because a mutation run found the branch uncovered: deleting it left the whole suite
/// green, which made it decoration.
#[tokio::test]
async fn the_survivor_check_catches_a_row_deleted_under_it() {
    require_ffmpeg();
    let dir = tempdir();
    let st = state(&dir).await;
    let (_from, _to, ids) = seed(&st).await;

    let segments: Vec<heldar_kernel::models::Segment> =
        sqlx::query_as("SELECT * FROM segments ORDER BY start_time")
            .fetch_all(&st.pool)
            .await
            .unwrap();
    assert_eq!(segments.len(), 2, "fixture");

    // Intact: the check must not fire, or it would refuse every export.
    heldar_kernel::services::clip::ensure_still_present(&st.pool, &segments)
        .await
        .expect("an intact set must pass");

    // Now the shape the race produces: the caller still holds a segment the sweeper has removed.
    sqlx::query("DELETE FROM segments WHERE id = ?")
        .bind(&ids[1])
        .execute(&st.pool)
        .await
        .unwrap();
    let err = heldar_kernel::services::clip::ensure_still_present(&st.pool, &segments)
        .await
        .expect_err("a row deleted under the export must be caught");
    assert_eq!(kind(&err), "NotFound", "got: {err}");
    let msg = format!("{err}");
    assert!(
        msg.contains("1 of 2") && msg.contains("pruned by retention"),
        "the message should say how much was lost and why, so a caller can tell this from a bug: {msg}"
    );
}

/// The read lock must be held BEFORE the media-job permit.
///
/// The permit is a semaphore. Taken between the SELECT and the lock, it left a window as long as the
/// export queue was deep — not the microseconds "TOCTOU" implies. Asserted on the source rather than
/// by racing threads, because the failure is an ORDERING and a timing test for it would be flaky in
/// exactly the conditions it is meant to check.
#[test]
fn the_read_lock_is_taken_before_the_blocking_permit() {
    let src = include_str!("../../heldar-kernel/src/services/clip.rs");
    let lock = src
        .find("SegReadLock::acquire")
        .expect("the read lock is gone");
    let permit = src
        .find("media_jobs.acquire(\"clip_export\")")
        .expect("the media-job permit is gone");
    assert!(
        lock < permit,
        "the media-job permit is acquired BEFORE the segment read lock. It blocks, so that reopens \
         the retention race (#183) for as long as the export queue is deep."
    );
}

/// A window with no footage at all is still a plain 404, not the new pruned-mid-export error.
#[tokio::test]
async fn an_empty_window_still_reports_no_footage() {
    require_ffmpeg();
    let dir = tempdir();
    let st = state(&dir).await;
    let (from, _to, _) = seed(&st).await;
    let err = heldar_kernel::services::clip::export_clip(
        &st,
        "cam_a",
        from - Duration::hours(5),
        from - Duration::hours(4),
    )
    .await
    .expect_err("a window with no segments must fail");
    assert_eq!(kind(&err), "NotFound");
    assert!(
        format!("{err}").contains("no recorded footage"),
        "an empty window and a pruned window are different situations and must read differently: {err}"
    );
}

fn require_ffmpeg() {
    let ok = std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(ok, "ffmpeg is required to build the fixture segments");
}

fn tempdir() -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("heldar-cliprace-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&p).unwrap();
    p
}
