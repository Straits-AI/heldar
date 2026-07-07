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

/// A file untouched for at least this long is treated as closed (not mid-write).
const SETTLE_SECS: u64 = 5;

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

    let now = SystemTime::now();
    for (name, path, mtime, size) in files {
        // Skip files still being written (recently modified).
        if let Ok(age) = now.duration_since(mtime) {
            if age < Duration::from_secs(SETTLE_SECS) {
                continue;
            }
        }
        let path_str = path.to_string_lossy().to_string();
        let already: Option<(String,)> = sqlx::query_as("SELECT id FROM segments WHERE path = ?")
            .bind(&path_str)
            .fetch_optional(pool)
            .await?;
        if already.is_some() {
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
