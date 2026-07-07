//! ANR (Automatic Network Replenishment) edge re-fill.
//!
//! Many IP cameras keep their own onboard SD/NVR recording that survives a network outage. When the
//! kernel loses the live RTSP stream the indexer records a [`recording_gaps`](crate::models::RecordingGap)
//! row; this loop (spawned from `main` only when `HELDAR_ANR_ENABLED`) tries to re-fetch the missed
//! footage from the camera's onboard storage by recording its REPLAY stream into the camera's normal
//! recordings dir (so the indexer then folds it back into the timeline).
//!
//! It is BEST-EFFORT and CAMERA-DEPENDENT: it only works when the camera retained the footage and
//! exposes a replay/playback endpoint. The replay URL comes from the per-camera
//! `anr_replay_url_template` (or the default Hikvision RTSP playback endpoint) — see
//! [`crate::camera_url::anr_replay_url`]. The re-filled file is named by the gap's START time (UTC
//! strftime, matching the recorder) so it lands at the right place on the timeline rather than "now".

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use sqlx::SqlitePool;
use tokio::process::Command;

use crate::camera_url;
use crate::config::Config;
use crate::models::{Camera, RecordingGap};

/// Upper bound on a single fill ffmpeg job regardless of gap length (a stuck replay can't wedge it).
const MAX_FILL_TIMEOUT_S: u64 = 4 * 3600;
/// How many pending gaps a single sweep attempts (keeps each tick bounded).
const MAX_GAPS_PER_SWEEP: i64 = 20;

pub async fn run(pool: SqlitePool, cfg: Arc<Config>) {
    if !cfg.anr_enabled {
        tracing::info!("anr: disabled (HELDAR_ANR_ENABLED=false)");
        return;
    }
    let interval_s = cfg.anr_interval_s.max(10);
    tracing::info!(
        interval_s,
        max_gap_hours = cfg.anr_max_gap_hours,
        max_attempts = cfg.anr_max_attempts,
        "anr: edge re-fill started"
    );
    let mut tick = tokio::time::interval(Duration::from_secs(interval_s));
    loop {
        tick.tick().await;
        if let Err(e) = sweep(&pool, &cfg).await {
            tracing::error!(error = %e, "anr: sweep failed");
        }
    }
}

/// Pick pending gaps (young enough, attempts left, on ANR-enabled cameras) and try to fill each.
async fn sweep(pool: &SqlitePool, cfg: &Config) -> anyhow::Result<()> {
    let max_attempts = cfg.anr_max_attempts.max(1);
    let cutoff = Utc::now() - chrono::Duration::hours(cfg.anr_max_gap_hours.max(1));
    let gaps: Vec<RecordingGap> = sqlx::query_as::<_, RecordingGap>(
        "SELECT g.* FROM recording_gaps g
           JOIN cameras c ON c.id = g.camera_id
          WHERE g.fill_state = 'pending'
            AND g.fill_attempts < ?
            AND g.gap_start >= ?
            AND c.anr_enabled = 1
          ORDER BY g.gap_start DESC
          LIMIT ?",
    )
    .bind(max_attempts)
    .bind(cutoff)
    .bind(MAX_GAPS_PER_SWEEP)
    .fetch_all(pool)
    .await?;

    for gap in gaps {
        let cam = match sqlx::query_as::<_, Camera>("SELECT * FROM cameras WHERE id = ?")
            .bind(&gap.camera_id)
            .fetch_optional(pool)
            .await?
        {
            Some(c) => c,
            None => continue, // camera vanished mid-sweep
        };
        match fill_gap(cfg, &cam, &gap).await {
            Ok(()) => {
                tracing::info!(camera = %gap.camera_id, gap = %gap.id, "anr: gap re-filled from camera storage");
                mark_filled(pool, &gap.id).await;
            }
            Err(e) => {
                tracing::warn!(camera = %gap.camera_id, gap = %gap.id, error = %e, "anr: fill attempt failed");
                bump_attempt(pool, &gap.id, max_attempts).await;
            }
        }
    }
    Ok(())
}

/// Record the camera's replay stream for the gap window into the recordings dir, named by gap start.
async fn fill_gap(cfg: &Config, cam: &Camera, gap: &RecordingGap) -> anyhow::Result<()> {
    let url = camera_url::anr_replay_url(cam, gap.gap_start, gap.gap_end).ok_or_else(|| {
        anyhow::anyhow!(
            "no replay URL: set anr_replay_url_template, or address+credentials for the default"
        )
    })?;

    let dir = cfg.camera_recordings_dir(&cam.id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| anyhow::anyhow!("creating recordings dir: {e}"))?;

    // Name by the gap's START (UTC strftime, like the recorder) so the indexer places it correctly.
    let fname = format!("{}.mp4", gap.gap_start.format("%Y%m%d_%H%M%S"));
    let final_path = dir.join(&fname);
    if tokio::fs::try_exists(&final_path).await.unwrap_or(false) {
        // Already present (a prior fill, or the recorder named a file at the same second): done.
        return Ok(());
    }
    // Write to a non-`.mp4` temp path so the indexer ignores it until the fill completes, then rename.
    let part_path = dir.join(format!("{fname}.part"));
    let _ = tokio::fs::remove_file(&part_path).await;

    let dur = gap.gap_seconds.max(1) as u64;
    let audio_args: &[&str] = if cam.record_audio {
        &["-c:a", "copy"]
    } else {
        &["-an"]
    };
    let mut cmd = Command::new(&cfg.ffmpeg_bin);
    cmd.kill_on_drop(true)
        .env("TZ", "UTC")
        .args(["-nostdin", "-hide_banner", "-loglevel", "warning"])
        .args(["-rtsp_transport", "tcp"])
        .args(["-timeout", "15000000"]) // 15s RTSP socket I/O timeout -> exit on stall
        .args(["-i", &url])
        .args(["-t", &dur.to_string()]) // bound to the gap length (safety cap vs replay end)
        .args(["-c", "copy"]) // stream-copy (no decode)
        .args(audio_args)
        .args(["-movflags", "+faststart"])
        .arg(&part_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());

    let job_timeout = Duration::from_secs((dur + 60).min(MAX_FILL_TIMEOUT_S));
    let outcome = tokio::time::timeout(job_timeout, cmd.output()).await;

    let out = match outcome {
        Err(_) => {
            let _ = tokio::fs::remove_file(&part_path).await;
            anyhow::bail!("replay capture timed out after {}s", job_timeout.as_secs());
        }
        Ok(Err(e)) => {
            let _ = tokio::fs::remove_file(&part_path).await;
            anyhow::bail!("spawning ffmpeg: {e}");
        }
        Ok(Ok(out)) => out,
    };
    if !out.status.success() {
        let err = camera_url::mask_url(String::from_utf8_lossy(&out.stderr).trim());
        let _ = tokio::fs::remove_file(&part_path).await;
        anyhow::bail!("ffmpeg replay failed: {err}");
    }

    let size = tokio::fs::metadata(&part_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    if size == 0 {
        let _ = tokio::fs::remove_file(&part_path).await;
        anyhow::bail!(
            "replay produced an empty file (camera likely has no footage for this window)"
        );
    }
    tokio::fs::rename(&part_path, &final_path)
        .await
        .map_err(|e| anyhow::anyhow!("finalizing {fname}: {e}"))?;
    Ok(())
}

async fn mark_filled(pool: &SqlitePool, gap_id: &str) {
    let now = Utc::now();
    let _ = sqlx::query(
        "UPDATE recording_gaps
            SET fill_state = 'filled', filled_at = ?, last_attempt_at = ?,
                fill_attempts = fill_attempts + 1
          WHERE id = ?",
    )
    .bind(now)
    .bind(now)
    .bind(gap_id)
    .execute(pool)
    .await;
}

/// Bump the attempt counter; mark `failed` once attempts are exhausted so the gap drops out of the
/// pending queue (the retry endpoint can reset it to `pending`).
async fn bump_attempt(pool: &SqlitePool, gap_id: &str, max_attempts: i64) {
    let _ = sqlx::query(
        "UPDATE recording_gaps
            SET fill_attempts = fill_attempts + 1,
                last_attempt_at = ?,
                fill_state = CASE WHEN fill_attempts + 1 >= ? THEN 'failed' ELSE 'pending' END
          WHERE id = ?",
    )
    .bind(Utc::now())
    .bind(max_attempts)
    .bind(gap_id)
    .execute(pool)
    .await;
}
