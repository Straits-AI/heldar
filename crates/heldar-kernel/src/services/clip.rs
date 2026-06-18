//! Evidence clip export: concatenates the segments overlapping a time range and trims to
//! the requested window with `-c copy` (no re-encode). Keyframe-aligned (Stage 0 precision).

use std::process::Stdio;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::process::Command;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::models::Segment;
use crate::state::AppState;

const MAX_CLIP_SECONDS: f64 = 3600.0;

#[derive(Debug, Serialize)]
pub struct ClipResult {
    pub id: String,
    pub camera_id: String,
    pub filename: String,
    pub url: String,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub requested_seconds: f64,
    pub size_bytes: u64,
    pub segment_count: usize,
}

pub async fn export_clip(
    state: &AppState,
    camera_id: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> AppResult<ClipResult> {
    if to <= from {
        return Err(AppError::BadRequest("`to` must be after `from`".into()));
    }
    let requested = (to - from).num_milliseconds() as f64 / 1000.0;
    if requested > MAX_CLIP_SECONDS {
        return Err(AppError::BadRequest(format!(
            "clip too long ({requested:.0}s); max {MAX_CLIP_SECONDS:.0}s"
        )));
    }

    let camera_exists: Option<(String,)> = sqlx::query_as("SELECT id FROM cameras WHERE id = ?")
        .bind(camera_id)
        .fetch_optional(&state.pool)
        .await?;
    if camera_exists.is_none() {
        return Err(AppError::NotFound(format!("camera {camera_id} not found")));
    }

    let segments: Vec<Segment> = sqlx::query_as::<_, Segment>(
        "SELECT * FROM segments
         WHERE camera_id = ? AND start_time < ? AND end_time > ?
         ORDER BY start_time ASC",
    )
    .bind(camera_id)
    .bind(to)
    .bind(from)
    .fetch_all(&state.pool)
    .await?;
    if segments.is_empty() {
        return Err(AppError::NotFound(
            "no recorded footage in the requested range".into(),
        ));
    }

    tokio::fs::create_dir_all(&state.cfg.clips_dir)
        .await
        .map_err(|e| AppError::Other(e.into()))?;

    let id = format!("clip_{}", Uuid::new_v4().simple());
    let filename = format!("{id}.mp4");
    let out_path = state.cfg.clips_dir.join(&filename);
    let list_path = state.cfg.clips_dir.join(format!("{id}.txt"));

    // Read-lock the source segments so the retention sweeper can't delete them out from under ffmpeg
    // mid-export (TOCTOU). The RAII guard releases on EVERY outcome — normal return, `?` error,
    // timeout, AND cancellation/panic (where a manual unlock would be skipped, leaking the lock).
    let seg_ids: Vec<String> = segments.iter().map(|s| s.id.clone()).collect();
    let _read_lock = crate::repo::SegReadLock::acquire(&state.pool, seg_ids).await;

    let size_outcome: AppResult<u64> = async {
        let mut list = String::new();
        for s in &segments {
            let escaped = s.path.replace('\'', "'\\''");
            list.push_str(&format!("file '{escaped}'\n"));
        }
        tokio::fs::write(&list_path, list)
            .await
            .map_err(|e| AppError::Other(e.into()))?;

        let first_start = segments[0].start_time;
        let ss = ((from - first_start).num_milliseconds() as f64 / 1000.0).max(0.0);

        let mut cmd = Command::new(&state.cfg.ffmpeg_bin);
        cmd.kill_on_drop(true)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "concat",
                "-safe",
                "0",
            ])
            .arg("-i")
            .arg(&list_path)
            .args(["-ss", &format!("{ss:.3}")])
            .args(["-t", &format!("{requested:.3}")])
            .args([
                "-c",
                "copy",
                "-avoid_negative_ts",
                "make_zero",
                "-movflags",
                "+faststart",
            ])
            .arg(&out_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        // Remux of even an hour of footage is fast; bound it so a hung/cancelled job can't wedge the
        // request or orphan ffmpeg (kill_on_drop kills the child when the timed-out future is dropped).
        let result = tokio::time::timeout(Duration::from_secs(180), cmd.output()).await;
        // Always remove the temp concat list, on every outcome.
        let _ = tokio::fs::remove_file(&list_path).await;

        let out = match result {
            Err(_) => {
                let _ = tokio::fs::remove_file(&out_path).await;
                return Err(AppError::Other(anyhow::anyhow!("clip export timed out")));
            }
            Ok(Err(e)) => {
                let _ = tokio::fs::remove_file(&out_path).await;
                return Err(AppError::Other(e.into()));
            }
            Ok(Ok(out)) => out,
        };

        if !out.status.success() {
            let _ = tokio::fs::remove_file(&out_path).await;
            return Err(AppError::Other(anyhow::anyhow!(
                "ffmpeg clip export failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }

        Ok(tokio::fs::metadata(&out_path)
            .await
            .map(|m| m.len())
            .unwrap_or(0))
    }
    .await;

    // `_read_lock` releases on drop (here or on any early return above). Surface any export error.
    let size_bytes = size_outcome?;

    Ok(ClipResult {
        id,
        camera_id: camera_id.to_string(),
        url: format!("/media/clips/{filename}"),
        filename,
        from,
        to,
        requested_seconds: requested,
        size_bytes,
        segment_count: segments.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_state() -> AppState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let cfg = std::sync::Arc::new(crate::config::Config::from_env());
        AppState {
            recorder: crate::services::recorder::RecorderManager::new(pool.clone(), cfg.clone()),
            sampler: crate::services::sampler::SamplerManager::new(pool.clone(), cfg.clone()),
            mirror: None,
            consumers: std::sync::Arc::new(Vec::new()),
            modules: std::sync::Arc::new(Vec::new()),
            catalog: std::sync::Arc::new(crate::services::registry::CatalogService::new(&cfg)),
            http: reqwest::Client::new(),
            started_at: Utc::now(),
            pool,
            cfg,
        }
    }

    async fn insert_camera(pool: &sqlx::SqlitePool, id: &str) {
        let now = Utc::now();
        sqlx::query("INSERT INTO cameras (id, name, created_at, updated_at) VALUES (?, ?, ?, ?)")
            .bind(id)
            .bind(format!("Camera {id}"))
            .bind(now)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn rejects_to_before_from() {
        let state = test_state().await;
        let from = Utc::now();
        let to = from - chrono::Duration::seconds(5);
        match export_clip(&state, "anycam", from, to).await {
            Err(AppError::BadRequest(msg)) => assert_eq!(msg, "`to` must be after `from`"),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rejects_to_equal_from() {
        // `to <= from` covers the equality boundary.
        let state = test_state().await;
        let from = Utc::now();
        match export_clip(&state, "anycam", from, from).await {
            Err(AppError::BadRequest(msg)) => assert_eq!(msg, "`to` must be after `from`"),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rejects_clip_exceeding_max_length() {
        // 3601s exceeds the 3600s cap; rejected before any DB lookup, so no camera is needed.
        let state = test_state().await;
        let from = Utc::now();
        let to = from + chrono::Duration::seconds(3601);
        match export_clip(&state, "anycam", from, to).await {
            Err(AppError::BadRequest(msg)) => {
                assert!(msg.contains("clip too long"), "msg was: {msg}");
                assert!(msg.contains("3601s"), "msg was: {msg}");
                assert!(msg.contains("3600s"), "msg was: {msg}");
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn max_length_boundary_passes_length_check() {
        // Exactly MAX_CLIP_SECONDS (3600s) is allowed (the guard uses a strict `>`), so validation
        // falls through to the camera lookup instead of returning the length error.
        let state = test_state().await;
        let from = Utc::now();
        let to = from + chrono::Duration::seconds(3600);
        match export_clip(&state, "cam_boundary", from, to).await {
            Err(AppError::NotFound(msg)) => assert_eq!(msg, "camera cam_boundary not found"),
            other => {
                panic!(
                    "expected NotFound (length check should pass at the boundary), got {other:?}"
                )
            }
        }
    }

    #[tokio::test]
    async fn unknown_camera_is_not_found() {
        let state = test_state().await;
        let from = Utc::now();
        let to = from + chrono::Duration::seconds(60);
        match export_clip(&state, "ghost", from, to).await {
            Err(AppError::NotFound(msg)) => assert_eq!(msg, "camera ghost not found"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn existing_camera_without_segments_is_not_found() {
        let state = test_state().await;
        insert_camera(&state.pool, "cam_empty").await;
        let from = Utc::now();
        let to = from + chrono::Duration::seconds(60);
        match export_clip(&state, "cam_empty", from, to).await {
            Err(AppError::NotFound(msg)) => {
                assert_eq!(msg, "no recorded footage in the requested range")
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }
}
