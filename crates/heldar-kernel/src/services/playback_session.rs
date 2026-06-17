//! Segment-spanning HLS playback sessions: interactive review of recorded footage over a time range,
//! with native seek. A session concatenates the segments overlapping the requested window, trims to
//! the exact offsets, and remuxes (`-c copy`, no re-encode) into a self-contained HLS VOD playlist
//! under `playback_dir/{session_id}/`. HLS players seek freely within a VOD playlist.
//!
//! The source segments are read-locked (`repo::set_segments_locked`) for the session lifetime so the
//! retention sweeper cannot prune footage that is being reviewed; the lock is released when the
//! session is deleted or expires. Sessions are tracked entirely on the filesystem (a `session.json`
//! per dir) so the background cleanup sweeper needs no shared in-memory state — and a crash leaves no
//! dangling locks (startup [`crate::db::clear_segment_read_locks`] clears every transient read-lock).

use std::path::Path as StdPath;
use std::process::Stdio;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::models::Segment;
use crate::state::AppState;

/// Bound a single remux so a hung/cancelled job cannot wedge the request or orphan ffmpeg
/// (kill_on_drop kills the child when the timed-out future is dropped). Stream-copy of even a couple
/// of hours of footage is fast; this is a generous backstop.
const BUILD_TIMEOUT: Duration = Duration::from_secs(300);

/// How often the cleanup sweeper looks for expired sessions to remove (seconds).
const CLEANUP_INTERVAL_S: u64 = 60;

/// Filename of the per-session metadata sidecar inside each session dir.
const META_FILE: &str = "session.json";

/// A live playback session over a recorded time range. Returned to clients (Serialize); the durable
/// metadata sidecar carries the extra bookkeeping fields the cleanup sweeper needs.
#[derive(Debug, Serialize)]
pub struct PlaybackSession {
    pub id: String,
    pub camera_id: String,
    /// HLS VOD playlist served under `/media/playback/{session_id}/index.m3u8` (play with hls.js).
    pub playlist_url: String,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    /// Length of the requested window in seconds (the playlist may be shorter where footage has gaps).
    pub duration_s: f64,
    pub segment_count: usize,
}

/// On-disk session record (`playback_dir/{session_id}/session.json`). Carries the source segment ids
/// so delete/cleanup can release exactly this session's read-locks, plus `created_at` for TTL expiry.
#[derive(Debug, Serialize, Deserialize)]
struct SessionMeta {
    id: String,
    camera_id: String,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    duration_s: f64,
    segment_count: usize,
    /// Source segment ids read-locked for this session's lifetime.
    segment_ids: Vec<String>,
    created_at: DateTime<Utc>,
}

/// A session id is server-generated (`pbs_<hex>`). Validate any client-supplied id strictly before
/// joining it to `playback_dir`, so a crafted id cannot traverse out of the playback tree (the id is
/// used to `remove_dir_all` a path).
fn is_valid_session_id(id: &str) -> bool {
    id.starts_with("pbs_")
        && id.len() <= 64
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Create a playback session for `camera_id` over `[from, to)`: read-lock the overlapping segments and
/// generate a trimmed HLS VOD playlist from them. Rejects (400) a range longer than
/// `HELDAR_MAX_PLAYBACK_SECONDS`, an empty/reversed range, or a range with no recorded footage (404).
pub async fn create_session(
    state: &AppState,
    camera_id: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> AppResult<PlaybackSession> {
    if to <= from {
        return Err(AppError::BadRequest("`to` must be after `from`".into()));
    }
    let requested = (to - from).num_milliseconds() as f64 / 1000.0;
    let max = state.cfg.max_playback_seconds;
    if requested > max {
        return Err(AppError::BadRequest(format!(
            "playback range too long ({requested:.0}s); max {max:.0}s"
        )));
    }

    // Camera must exist; its segment length sets the target HLS segment duration.
    let cam: Option<(i64,)> = sqlx::query_as("SELECT segment_seconds FROM cameras WHERE id = ?")
        .bind(camera_id)
        .fetch_optional(&state.pool)
        .await?;
    let Some((segment_seconds,)) = cam else {
        return Err(AppError::NotFound(format!("camera {camera_id} not found")));
    };

    // Same overlap query the clip exporter uses: any segment intersecting the window.
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

    let session_id = format!("pbs_{}", Uuid::new_v4().simple());
    let session_dir = state.cfg.playback_dir.join(&session_id);
    tokio::fs::create_dir_all(&session_dir)
        .await
        .map_err(|e| AppError::Other(e.into()))?;

    // Read-lock the source segments so the retention sweeper can't delete them out from under the
    // session (the HLS dir is a self-contained copy, but the product holds footage under review).
    let seg_ids: Vec<String> = segments.iter().map(|s| s.id.clone()).collect();
    crate::repo::set_segments_locked(&state.pool, &seg_ids, true).await;

    let hls_time = segment_seconds.max(2);
    let build = generate_hls(state, &session_dir, &segments, from, requested, hls_time).await;
    if let Err(e) = build {
        // Generation failed: release the locks and remove the half-built dir, then surface the error.
        crate::repo::set_segments_locked(&state.pool, &seg_ids, false).await;
        let _ = tokio::fs::remove_dir_all(&session_dir).await;
        return Err(e);
    }

    let meta = SessionMeta {
        id: session_id.clone(),
        camera_id: camera_id.to_string(),
        from,
        to,
        duration_s: requested,
        segment_count: segments.len(),
        segment_ids: seg_ids.clone(),
        created_at: Utc::now(),
    };
    let meta_json = serde_json::to_vec(&meta).map_err(|e| AppError::Other(e.into()))?;
    if let Err(e) = tokio::fs::write(session_dir.join(META_FILE), meta_json).await {
        // Without the sidecar the cleanup sweeper can't release the locks; fail clean instead.
        crate::repo::set_segments_locked(&state.pool, &seg_ids, false).await;
        let _ = tokio::fs::remove_dir_all(&session_dir).await;
        return Err(AppError::Other(e.into()));
    }

    tracing::info!(
        session = %session_id,
        camera = %camera_id,
        segments = segments.len(),
        duration_s = requested,
        "playback: created session"
    );
    Ok(PlaybackSession {
        playlist_url: format!("/media/playback/{session_id}/index.m3u8"),
        id: session_id,
        camera_id: camera_id.to_string(),
        from,
        to,
        duration_s: requested,
        segment_count: segments.len(),
    })
}

/// Concatenate `segments`, trim to the exact `[from, from+requested)` window, and remux (`-c copy`)
/// into an HLS VOD playlist (`index.m3u8` + `seg_*.ts`) inside `session_dir`. The temp concat list
/// (which holds absolute recording paths) is removed on every outcome so it is never served.
async fn generate_hls(
    state: &AppState,
    session_dir: &StdPath,
    segments: &[Segment],
    from: DateTime<Utc>,
    requested: f64,
    hls_time: i64,
) -> AppResult<()> {
    let list_path = session_dir.join("concat.txt");
    let mut list = String::new();
    for s in segments {
        let escaped = s.path.replace('\'', "'\\''");
        list.push_str(&format!("file '{escaped}'\n"));
    }
    tokio::fs::write(&list_path, list)
        .await
        .map_err(|e| AppError::Other(e.into()))?;

    let first_start = segments[0].start_time;
    let ss = ((from - first_start).num_milliseconds() as f64 / 1000.0).max(0.0);
    let playlist_path = session_dir.join("index.m3u8");
    let seg_pattern = session_dir.join("seg_%05d.ts");

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
        // Trim the concatenated input to the exact window (keyframe-aligned, like the clip exporter).
        .args(["-ss", &format!("{ss:.3}")])
        .args(["-t", &format!("{requested:.3}")])
        .args(["-c", "copy", "-avoid_negative_ts", "make_zero"])
        // HLS VOD: a complete, seekable playlist (vod forces an unbounded list_size = all segments).
        .args(["-f", "hls"])
        .args(["-hls_time", &hls_time.to_string()])
        .args(["-hls_playlist_type", "vod"])
        .arg("-hls_segment_filename")
        .arg(&seg_pattern)
        .arg(&playlist_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let result = tokio::time::timeout(BUILD_TIMEOUT, cmd.output()).await;
    // Always drop the temp concat list (it leaks absolute recording paths), on every outcome.
    let _ = tokio::fs::remove_file(&list_path).await;

    let out = match result {
        Err(_) => return Err(AppError::Other(anyhow::anyhow!("playback build timed out"))),
        Ok(Err(e)) => return Err(AppError::Other(e.into())),
        Ok(Ok(out)) => out,
    };
    if !out.status.success() {
        return Err(AppError::Other(anyhow::anyhow!(
            "ffmpeg playback build failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Delete a playback session: release its segment read-locks and remove its HLS dir. Idempotent in
/// the locks (best-effort); 404 if the session does not exist.
pub async fn delete_session(state: &AppState, session_id: &str) -> AppResult<()> {
    if !is_valid_session_id(session_id) {
        return Err(AppError::BadRequest("invalid session id".into()));
    }
    let session_dir = state.cfg.playback_dir.join(session_id);
    if !tokio::fs::try_exists(&session_dir).await.unwrap_or(false) {
        return Err(AppError::NotFound(format!(
            "playback session {session_id} not found"
        )));
    }
    if let Some(meta) = read_meta(&session_dir).await {
        crate::repo::set_segments_locked(&state.pool, &meta.segment_ids, false).await;
    }
    tokio::fs::remove_dir_all(&session_dir)
        .await
        .map_err(|e| AppError::Other(e.into()))?;
    tracing::info!(session = %session_id, "playback: deleted session");
    Ok(())
}

/// Read+parse a session's metadata sidecar, if present and well-formed.
async fn read_meta(session_dir: &StdPath) -> Option<SessionMeta> {
    let bytes = tokio::fs::read(session_dir.join(META_FILE)).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Background sweeper: remove session dirs older than the TTL and release their read-locks. Spawned
/// (supervised) from `main`.
pub async fn run(state: AppState) {
    let mut tick = tokio::time::interval(Duration::from_secs(CLEANUP_INTERVAL_S));
    loop {
        tick.tick().await;
        if let Err(e) = sweep(&state).await {
            tracing::error!(error = %e, "playback_session_cleanup: tick failed");
        }
    }
}

/// Remove every expired session once. Expiry is `created_at + TTL <= now` from the metadata sidecar;
/// a dir with no/corrupt metadata falls back to its directory mtime (and releases no locks — startup
/// already clears stale read-locks).
async fn sweep(state: &AppState) -> anyhow::Result<()> {
    let ttl = chrono::Duration::minutes(state.cfg.playback_session_ttl_minutes.max(1));
    let now = Utc::now();
    let mut entries = match tokio::fs::read_dir(&state.cfg.playback_dir).await {
        Ok(e) => e,
        // The playback dir may not exist yet (no session ever created); nothing to do.
        Err(_) => return Ok(()),
    };
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if !entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let meta = read_meta(&path).await;
        let expired = match &meta {
            Some(m) => m.created_at + ttl <= now,
            None => dir_mtime_before(&path, now - ttl).await,
        };
        if !expired {
            continue;
        }
        if let Some(m) = &meta {
            crate::repo::set_segments_locked(&state.pool, &m.segment_ids, false).await;
        }
        match tokio::fs::remove_dir_all(&path).await {
            Ok(()) => {
                tracing::debug!(dir = %path.display(), "playback_session_cleanup: removed expired session")
            }
            Err(e) => {
                tracing::warn!(error = %e, dir = %path.display(), "playback_session_cleanup: failed to remove session dir")
            }
        }
    }
    Ok(())
}

/// Whether a directory's last-modified time is before `cutoff` (fallback expiry when the metadata
/// sidecar is missing/unreadable). Conservative: returns false when the time can't be read.
async fn dir_mtime_before(path: &StdPath, cutoff: DateTime<Utc>) -> bool {
    match tokio::fs::metadata(path).await.and_then(|m| m.modified()) {
        Ok(modified) => DateTime::<Utc>::from(modified) < cutoff,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_validation_rejects_traversal() {
        assert!(is_valid_session_id("pbs_0123abcdef"));
        assert!(!is_valid_session_id("pbs_../../etc"));
        assert!(!is_valid_session_id("pbs_a/b"));
        assert!(!is_valid_session_id("../pbs_x"));
        assert!(!is_valid_session_id("clip_123")); // wrong prefix
        assert!(!is_valid_session_id("pbs_with.dot"));
        assert!(!is_valid_session_id(&format!("pbs_{}", "a".repeat(80)))); // too long
    }
}
