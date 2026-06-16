//! Storage / disk observability: free space on the recordings filesystem (via statvfs), the
//! recordings footprint, and a projected retention horizon from the recent write rate.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sqlx::SqlitePool;

use crate::config::Config;

#[derive(Debug, Clone, Copy, Serialize, Default)]
pub struct DiskStats {
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub used_bytes: u64,
    pub used_percent: f64,
}

/// Free/total bytes of the filesystem backing `path` (statvfs). Returns None if it can't be read.
pub fn disk_stats(path: &Path) -> Option<DiskStats> {
    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: c_path is a valid NUL-terminated C string; statvfs only reads through the pointer and
    // writes into the zeroed stack-allocated struct.
    let stat = unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c_path.as_ptr(), &mut stat) != 0 {
            return None;
        }
        stat
    };
    let block = stat.f_frsize as u64;
    let total = stat.f_blocks as u64 * block;
    // free_bytes = f_bavail (blocks writable by an unprivileged user): the real write headroom.
    let free = stat.f_bavail as u64 * block;
    // used/used_percent use f_bfree (free blocks incl. root-reserved) for a consistent basis with
    // total, so used_percent matches `df` rather than over-counting the reserved blocks as used.
    let free_total = stat.f_bfree as u64 * block;
    let used = total.saturating_sub(free_total);
    let used_percent = if total > 0 {
        used as f64 / total as f64 * 100.0
    } else {
        0.0
    };
    Some(DiskStats {
        total_bytes: total,
        free_bytes: free,
        used_bytes: used,
        used_percent,
    })
}

/// Async wrapper: run the (potentially blocking on network filesystems) statvfs off the runtime.
pub async fn disk_stats_async(path: PathBuf) -> Option<DiskStats> {
    tokio::task::spawn_blocking(move || disk_stats(&path))
        .await
        .ok()
        .flatten()
}

#[derive(Debug, Clone, Serialize)]
pub struct StorageReport {
    pub disk: Option<DiskStats>,
    pub recordings_bytes: i64,
    pub segment_count: i64,
    pub oldest_segment: Option<DateTime<Utc>>,
    pub newest_segment: Option<DateTime<Utc>>,
    /// Bytes/day written over the last 24h of indexed segments (recent write rate).
    pub write_rate_bytes_per_day: i64,
    /// Projected days of free space remaining at the recent write rate (None if unknown/idle).
    pub projected_days_remaining: Option<f64>,
}

/// Compute a storage report combining disk stats with the recordings footprint and write rate.
pub async fn storage_report(pool: &SqlitePool, cfg: &Config) -> sqlx::Result<StorageReport> {
    let recordings_bytes: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(size_bytes), 0) FROM segments")
            .fetch_one(pool)
            .await?;
    let segment_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM segments")
        .fetch_one(pool)
        .await?;
    let oldest_segment: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT MIN(start_time) FROM segments")
            .fetch_one(pool)
            .await?;
    let newest_segment: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT MAX(end_time) FROM segments")
            .fetch_one(pool)
            .await?;

    // Write rate over the last 24h of *recorded* footage (by end_time, not index time, so a
    // post-restart backfill of old segments doesn't spike the projection).
    let since = Utc::now() - Duration::hours(24);
    let last_day_bytes: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(size_bytes), 0) FROM segments WHERE end_time >= ?")
            .bind(since)
            .fetch_one(pool)
            .await?;

    let disk = disk_stats_async(cfg.recordings_dir.clone()).await;
    let projected_days_remaining = match (disk, last_day_bytes) {
        (Some(d), rate) if rate > 0 => Some(d.free_bytes as f64 / rate as f64),
        _ => None,
    };

    Ok(StorageReport {
        disk,
        recordings_bytes,
        segment_count,
        oldest_segment,
        newest_segment,
        write_rate_bytes_per_day: last_day_bytes,
        projected_days_remaining,
    })
}
