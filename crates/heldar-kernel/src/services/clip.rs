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
    // mid-export (TOCTOU). Released on EVERY outcome below.
    let seg_ids: Vec<String> = segments.iter().map(|s| s.id.clone()).collect();
    crate::repo::set_segments_locked(&state.pool, &seg_ids, true).await;

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

    // Release the read-lock on every outcome, then surface any error.
    crate::repo::set_segments_locked(&state.pool, &seg_ids, false).await;
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
