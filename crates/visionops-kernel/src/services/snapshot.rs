//! Snapshot extraction: a single JPEG frame either from recorded footage at a timestamp,
//! or live from the camera stream right now.

use std::process::Stdio;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::process::Command;

use crate::camera_url;
use crate::error::{AppError, AppResult};
use crate::models::{Camera, Segment};
use crate::state::AppState;

/// Extract one frame from recorded footage at `at`.
pub async fn snapshot_at(
    state: &AppState,
    camera_id: &str,
    at: DateTime<Utc>,
) -> AppResult<Vec<u8>> {
    let seg: Option<Segment> = sqlx::query_as::<_, Segment>(
        "SELECT * FROM segments
         WHERE camera_id = ? AND start_time <= ? AND end_time >= ?
         ORDER BY start_time DESC LIMIT 1",
    )
    .bind(camera_id)
    .bind(at)
    .bind(at)
    .fetch_optional(&state.pool)
    .await?;
    let seg = seg.ok_or_else(|| AppError::NotFound("no footage at that timestamp".into()))?;

    // Read-lock the source segment so retention can't delete it out from under ffmpeg (TOCTOU).
    let seg_ids = vec![seg.id.clone()];
    crate::repo::set_segments_locked(&state.pool, &seg_ids, true).await;

    let outcome: AppResult<Vec<u8>> = async {
        let offset = ((at - seg.start_time).num_milliseconds() as f64 / 1000.0).max(0.0);
        let mut cmd = Command::new(&state.cfg.ffmpeg_bin);
        cmd.kill_on_drop(true)
            .args(["-hide_banner", "-loglevel", "error"])
            .args(["-ss", &format!("{offset:.3}")])
            .arg("-i")
            .arg(&seg.path)
            .args([
                "-frames:v",
                "1",
                "-q:v",
                "3",
                "-f",
                "image2",
                "-c:v",
                "mjpeg",
                "pipe:1",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let out = tokio::time::timeout(Duration::from_secs(20), cmd.output())
            .await
            .map_err(|_| AppError::Other(anyhow::anyhow!("snapshot timed out")))?
            .map_err(|e| AppError::Other(e.into()))?;

        if !out.status.success() || out.stdout.is_empty() {
            return Err(AppError::Other(anyhow::anyhow!(
                "snapshot failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(out.stdout)
    }
    .await;

    crate::repo::set_segments_locked(&state.pool, &seg_ids, false).await;
    outcome
}

/// Grab one frame live from the camera stream (sub-stream preferred).
pub async fn snapshot_live(state: &AppState, camera_id: &str) -> AppResult<Vec<u8>> {
    let cam: Option<Camera> = sqlx::query_as::<_, Camera>("SELECT * FROM cameras WHERE id = ?")
        .bind(camera_id)
        .fetch_optional(&state.pool)
        .await?;
    let cam = cam.ok_or_else(|| AppError::NotFound(format!("camera {camera_id} not found")))?;
    let url = camera_url::stream_url(&cam, "sub")
        .or_else(|| camera_url::record_url(&cam))
        .ok_or_else(|| AppError::BadRequest("camera has no stream URL".into()))?;

    let mut cmd = Command::new(&state.cfg.ffmpeg_bin);
    cmd.kill_on_drop(true)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-rtsp_transport",
            "tcp",
            "-timeout",
            "10000000",
        ])
        .arg("-i")
        .arg(&url)
        .args([
            "-frames:v",
            "1",
            "-q:v",
            "3",
            "-f",
            "image2",
            "-c:v",
            "mjpeg",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let out = tokio::time::timeout(Duration::from_secs(20), cmd.output())
        .await
        .map_err(|_| AppError::Other(anyhow::anyhow!("live snapshot timed out")))?
        .map_err(|e| AppError::Other(e.into()))?;

    if !out.status.success() || out.stdout.is_empty() {
        // Mask credentials that ffmpeg echoes back in the RTSP URL on failure.
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(AppError::Other(anyhow::anyhow!(
            "live snapshot failed: {}",
            camera_url::mask_url(stderr.trim())
        )));
    }
    Ok(out.stdout)
}
