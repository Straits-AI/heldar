//! Timeline indexer: periodically scans each camera's recordings directory, turning closed
//! segment files into rows in the `segments` table (the timeline index) and detecting gaps.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::config::Config;
use crate::models::Camera;
use crate::repo;
use crate::util;

/// Grace period past the expected segment length before the newest file — the one the recorder still
/// holds open — is treated as closed. Only used for the LAST file in a camera's directory; every
/// earlier file is proven closed by the existence of a successor (see `index_camera_dir`).
///
/// This is deliberately not a general "settled" test. mtime is NOT a reliable liveness signal for a
/// buffered writer: ffmpeg flushes mp4 data in 256 KiB bursts, so at a few tens of KB/s the file's
/// mtime can sit untouched for many seconds mid-write. A short mtime-age check therefore admits
/// half-written segments, which is exactly how this indexer used to record a 60-second segment as
/// "10 seconds / 262,172 bytes" (= one 256 KiB flush + the mp4 header) and never correct it.
const LAST_FILE_GRACE_SECS: u64 = 15;

/// Is this segment file finished being written, and therefore safe to index?
///
/// Decided by ORDER, not by timing. The recorder's segmenter writes strictly sequentially with
/// timestamped names, so a file that has a lexicographically-later sibling is *provably* closed —
/// no timing assumption required. Only the final file can still be open.
///
/// The final file gets the one time-based exception: if the recorder stopped (camera offline,
/// shutdown) no successor will ever appear, so it is admitted once it has been untouched for longer
/// than a whole segment plus a grace period. Without that, the last segment of every recording
/// session would never be indexed.
///
/// This deliberately replaced a short mtime-age "settle" window applied to *every* file. mtime is not
/// a reliable liveness signal for a buffered writer — ffmpeg flushes mp4 data in 256 KiB bursts, so a
/// file being actively written routinely sits untouched for longer than such a window. That admitted
/// half-written segments, whose understated `end_time` manufactured phantom recording gaps and whose
/// understated `size_bytes` made the recordings cap evict against a large undercount.
fn segment_is_closed(
    idx: usize,
    last_index: usize,
    quiet_for: Duration,
    segment_seconds: i64,
) -> bool {
    if idx != last_index {
        return true; // a successor exists → definitively closed
    }
    quiet_for >= Duration::from_secs(segment_seconds.max(1) as u64 + LAST_FILE_GRACE_SECS)
}

pub async fn run(pool: SqlitePool, cfg: Arc<Config>) {
    let mut tick = tokio::time::interval(Duration::from_secs(cfg.indexer_interval_s.max(2)));
    loop {
        tick.tick().await;
        if let Err(e) = scan_once(&pool, &cfg).await {
            tracing::error!(error = %e, "indexer: scan failed");
        }
    }
}

async fn scan_once(pool: &SqlitePool, cfg: &Config) -> anyhow::Result<()> {
    let cams: Vec<Camera> = sqlx::query_as::<_, Camera>("SELECT * FROM cameras")
        .fetch_all(pool)
        .await?;
    for cam in cams {
        let dir = cfg.camera_recordings_dir(&cam.id);
        if !dir.exists() {
            continue;
        }
        if let Err(e) = index_camera_dir(pool, cfg, &cam.id, &dir).await {
            tracing::error!(camera_id = %cam.id, error = %e, "indexer: dir scan failed");
        }
    }
    Ok(())
}

async fn index_camera_dir(
    pool: &SqlitePool,
    cfg: &Config,
    camera_id: &str,
    dir: &Path,
) -> anyhow::Result<()> {
    let mut entries = tokio::fs::read_dir(dir).await?;
    let mut files: Vec<(String, std::path::PathBuf, SystemTime, u64)> = Vec::new();
    while let Some(ent) = entries.next_entry().await? {
        let path = ent.path();
        if path.extension().and_then(|e| e.to_str()) != Some("mp4") {
            continue;
        }
        let Ok(meta) = ent.metadata().await else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let name = ent.file_name().to_string_lossy().to_string();
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        files.push((name, path, mtime, meta.len()));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));

    // Completeness is decided by ORDER, not by mtime. The recorder's segmenter writes strictly
    // sequentially with timestamped names, so any file that has a lexicographically-later sibling is
    // definitively closed. Only the final file can still be open.
    let last_index = files.len().saturating_sub(1);

    let now = SystemTime::now();
    for (idx, (name, path, mtime, size)) in files.into_iter().enumerate() {
        let quiet_for = now.duration_since(mtime).unwrap_or_default();
        if !segment_is_closed(idx, last_index, quiet_for, cfg.default_segment_seconds) {
            continue;
        }
        let path_str = path.to_string_lossy().to_string();
        let already: Option<(String, i64)> =
            sqlx::query_as("SELECT id, size_bytes FROM segments WHERE path = ?")
                .bind(&path_str)
                .fetch_optional(pool)
                .await?;
        if let Some((id, indexed_size)) = already {
            // Reconciliation: repair rows written by an older build that indexed this file mid-write.
            // The on-disk file only grows while open and is immutable once closed, so a stored size
            // that disagrees with the file means the row captured a partial write. Re-probe and correct
            // it — otherwise the understated end_time keeps manufacturing phantom gaps, and the
            // understated size keeps the recordings cap evicting against a number far below reality.
            if indexed_size != size as i64 && size > 0 {
                // Size first, and unconditionally: it comes from the filesystem, needs no probe, and is
                // what the recordings cap sums. Gating this on ffprobe would leave the cap under-counting
                // whenever a probe fails — the failure mode this repair exists to end.
                let _ = sqlx::query("UPDATE segments SET size_bytes = ? WHERE id = ?")
                    .bind(size as i64)
                    .bind(&id)
                    .execute(pool)
                    .await;
                // Duration/end_time additionally need the real media length, so those follow a probe.
                if let (Some(start), Ok(probe)) = (
                    util::parse_segment_time(&name),
                    util::ffprobe_file(&cfg.ffprobe_bin, &path).await,
                ) {
                    if probe.duration_s.is_finite() && probe.duration_s > 0.05 {
                        let end = start
                            + chrono::Duration::milliseconds((probe.duration_s * 1000.0) as i64);
                        let _ = sqlx::query(
                            "UPDATE segments SET end_time = ?, duration_s = ? WHERE id = ?",
                        )
                        .bind(end)
                        .bind(probe.duration_s)
                        .bind(&id)
                        .execute(pool)
                        .await;
                    }
                }
                tracing::info!(
                    %camera_id, file = %name, was = indexed_size, now = size,
                    "indexer: repaired a segment row indexed mid-write"
                );
            }
            continue;
        }
        let Some(start) = util::parse_segment_time(&name) else {
            tracing::warn!(%camera_id, file = %name, "indexer: unparseable filename, skipping");
            continue;
        };
        let probe = match util::ffprobe_file(&cfg.ffprobe_bin, &path).await {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(%camera_id, file = %name, error = %e, "indexer: probe failed (retry later)");
                continue;
            }
        };
        if !probe.duration_s.is_finite() || probe.duration_s <= 0.05 || size == 0 {
            continue; // empty/just-rotated stub, or a bogus (NaN/inf) probed duration
        }
        let end = start + chrono::Duration::milliseconds((probe.duration_s * 1000.0) as i64);
        let bitrate_kbps = if probe.duration_s > 0.0 {
            Some((size as f64 * 8.0) / probe.duration_s / 1000.0)
        } else {
            None
        };

        let prev_end: Option<(DateTime<Utc>,)> = sqlx::query_as(
            "SELECT end_time FROM segments WHERE camera_id = ? ORDER BY end_time DESC LIMIT 1",
        )
        .bind(camera_id)
        .fetch_optional(pool)
        .await?;

        let id = format!("seg_{}", Uuid::new_v4().simple());
        sqlx::query(
            "INSERT INTO segments
               (id, camera_id, path, start_time, end_time, duration_s, codec, width, height,
                size_bytes, container, locked, incident_id, created_at)
             VALUES (?,?,?,?,?,?,?,?,?,?, 'mp4', 0, NULL, ?)",
        )
        .bind(&id)
        .bind(camera_id)
        .bind(&path_str)
        .bind(start)
        .bind(end)
        .bind(probe.duration_s)
        .bind(&probe.codec)
        .bind(probe.width)
        .bind(probe.height)
        .bind(size as i64)
        .bind(Utc::now())
        .execute(pool)
        .await?;

        let _ = repo::record_segment_indexed(pool, camera_id, end, bitrate_kbps, probe.fps).await;

        if let Some((pe,)) = prev_end {
            // Second-resolution segment filenames can make the previous segment's end overlap this
            // one's start. Clamp any prior segment that overlaps this start so segments never overlap
            // in time (A.end <= B.start) — keeps playback/timeline coverage unambiguous.
            if pe > start {
                let _ = sqlx::query(
                    "UPDATE segments SET end_time = ? WHERE camera_id = ? AND end_time > ? AND start_time < ?",
                )
                .bind(start)
                .bind(camera_id)
                .bind(start)
                .bind(start)
                .execute(pool)
                .await;
            }
            let gap = (start - pe).num_seconds();
            if gap > 3 {
                let _ = repo::log_event(
                    pool,
                    Some(camera_id),
                    "recording_gap",
                    "warning",
                    json!({ "gap_seconds": gap, "prev_end": pe, "next_start": start }),
                )
                .await;
                // Persist the gap for ANR edge re-fill (ignore-on-conflict by camera_id + start).
                let _ = repo::upsert_recording_gap(pool, camera_id, pe, start, gap).await;
            }
        }
        tracing::debug!(%camera_id, file = %name, dur = probe.duration_s, "indexer: indexed segment");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn mem_pool_migrated() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        pool
    }

    fn test_cfg(dir: &Path) -> Config {
        let mut cfg = Config::from_env();
        cfg.recordings_dir = dir.to_path_buf();
        cfg.default_segment_seconds = 60;
        cfg
    }

    /// Write a file of `size` bytes whose name encodes `hhmmss` on a fixed date.
    fn write_segment(dir: &Path, hhmmss: &str, size: usize) -> std::path::PathBuf {
        let p = dir.join(format!("20260803_{hhmmss}.mp4"));
        std::fs::write(&p, vec![0u8; size]).unwrap();
        p
    }

    /// The core regression, as a decision table so it needs no ffmpeg and no timing luck.
    ///
    /// The old rule was "mtime older than ~5s ⇒ closed". ffmpeg flushes in 256 KiB bursts, so an
    /// actively-written file routinely goes quiet for longer than that — the old rule then indexed a
    /// half-written segment, recording a 60s segment as ~10s / 262,172 bytes and never correcting it.
    #[test]
    fn only_the_last_file_can_be_open_and_it_needs_a_full_segment_of_quiet() {
        let seg = 60;
        // Any file with a successor is closed — even one written microseconds ago. The OLD mtime rule
        // got this wrong in the opposite direction, needlessly deferring fresh-but-closed files.
        assert!(segment_is_closed(0, 2, Duration::from_millis(1), seg));
        assert!(segment_is_closed(1, 2, Duration::from_millis(1), seg));

        // The last file is the one being written. Quiet for 30s looks "settled" to any short window —
        // this is exactly the case the old 5s rule admitted and got wrong — but it is NOT closed.
        assert!(!segment_is_closed(2, 2, Duration::from_secs(30), seg));
        assert!(!segment_is_closed(2, 2, Duration::from_secs(6), seg));

        // ...unless the recorder has clearly stopped: quiet for longer than a segment + grace, so no
        // successor is ever coming. Otherwise the final segment of a session would never be indexed.
        assert!(segment_is_closed(2, 2, Duration::from_secs(60 + 15), seg));
        assert!(segment_is_closed(2, 2, Duration::from_secs(600), seg));

        // A lone file (idx == last == 0) follows the same rule.
        assert!(!segment_is_closed(0, 0, Duration::from_secs(10), seg));
        assert!(segment_is_closed(0, 0, Duration::from_secs(120), seg));
    }

    /// A stale row from an older build (indexed mid-write) must be repaired once the file is closed,
    /// not left understating its size forever — otherwise the size cap keeps under-counting.
    #[tokio::test]
    async fn a_row_indexed_mid_write_is_repaired_when_the_size_disagrees() {
        let tmp = std::env::temp_dir().join(format!("heldar_idx_{}", Uuid::new_v4().simple()));
        let cam_dir = tmp.join("cam_t");
        std::fs::create_dir_all(&cam_dir).unwrap();
        let f = write_segment(&cam_dir, "120000", 4096);
        write_segment(&cam_dir, "120100", 4096); // successor -> f is closed

        let pool = mem_pool_migrated().await;
        sqlx::query(
            "INSERT INTO cameras (id, name, created_at, updated_at) VALUES ('cam_t','cam_t',?,?)",
        )
        .bind(Utc::now())
        .bind(Utc::now())
        .execute(&pool)
        .await
        .unwrap();
        // Simulate the old bug: a row claiming a fraction of the real file size.
        sqlx::query(
            "INSERT INTO segments (id, camera_id, path, start_time, end_time, duration_s, size_bytes,
                                   container, locked, created_at)
             VALUES ('seg_stale','cam_t',?,?,?,10.0,262172,'mp4',0,?)",
        )
        .bind(f.to_string_lossy().to_string())
        .bind(Utc::now())
        .bind(Utc::now())
        .bind(Utc::now())
        .execute(&pool)
        .await
        .unwrap();

        let cfg = test_cfg(&tmp);
        let _ = index_camera_dir(&pool, &cfg, "cam_t", &cam_dir).await;

        let size: i64 =
            sqlx::query_scalar("SELECT size_bytes FROM segments WHERE id = 'seg_stale'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_ne!(
            size, 262172,
            "a stale mid-write row must be reconciled against the real file, not left understated"
        );

        std::fs::remove_dir_all(&tmp).ok();
    }
}
