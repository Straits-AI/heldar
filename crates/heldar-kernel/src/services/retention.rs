//! Retention sweeper: deletes recordings past each camera's age policy, and enforces a global
//! size cap by pruning the oldest deletable segments. Segments under a durable evidence hold
//! (`evidence_locked = 1`) are never deleted, and a segment with a transient export read-lock
//! (`locked = 1`) is skipped while the export is in flight. Both are excluded from every prune.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::SqlitePool;

use crate::config::Config;
use crate::repo;
use crate::services::storage;

/// Delete a segment's file and report whether its DB row should now be removed. The row is removed
/// only when the file is actually gone — deleted just now, or already absent (`NotFound`). If the
/// delete fails for any other reason (permissions, I/O error), we keep the DB row so the file is not
/// orphaned-yet-forgotten: the next sweep retries it, and the size/disk accounting stays truthful.
async fn unlink_segment(path: &str) -> bool {
    match tokio::fs::remove_file(path).await {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => {
            tracing::error!(path, error = %e, "retention: failed to delete segment file; keeping DB row to retry next sweep");
            false
        }
    }
}

/// Delete a snapshot's file and report whether its DB row should now be removed. Mirrors
/// [`unlink_segment`]: the row is removed only when the file is actually gone (deleted just now or
/// already absent); on any other delete error we keep the row so the next sweep retries.
async fn unlink_snapshot(path: &str) -> bool {
    match tokio::fs::remove_file(path).await {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => {
            tracing::error!(path, error = %e, "retention: failed to delete snapshot file; keeping DB row to retry next sweep");
            false
        }
    }
}

pub async fn run(pool: SqlitePool, cfg: Arc<Config>) {
    let mut tick = tokio::time::interval(Duration::from_secs(cfg.retention_interval_s.max(30)));
    loop {
        tick.tick().await;
        if let Err(e) = sweep(&pool, &cfg).await {
            tracing::error!(error = %e, "retention: sweep failed");
        }
    }
}

async fn sweep(pool: &SqlitePool, cfg: &Config) -> anyhow::Result<()> {
    // 1) Age-based retention, per-camera.
    let mut age_deleted: u64 = 0;
    let cams: Vec<(String, i64)> = sqlx::query_as("SELECT id, retention_hours FROM cameras")
        .fetch_all(pool)
        .await?;
    for (id, hours) in cams {
        let cutoff = Utc::now() - chrono::Duration::hours(hours.max(1));
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, path FROM segments WHERE camera_id = ? AND locked = 0 AND evidence_locked = 0 AND end_time < ?",
        )
        .bind(&id)
        .bind(cutoff)
        .fetch_all(pool)
        .await?;
        for (seg_id, path) in rows {
            if unlink_segment(&path).await {
                sqlx::query("DELETE FROM segments WHERE id = ?")
                    .bind(&seg_id)
                    .execute(pool)
                    .await?;
                age_deleted += 1;
            }
        }
    }
    if age_deleted > 0 {
        let _ = repo::log_event(
            pool,
            None,
            "retention_delete",
            "info",
            json!({ "deleted": age_deleted, "reason": "age" }),
        )
        .await;
        tracing::info!(deleted = age_deleted, "retention: age-based cleanup");
    }

    // 2) Per-camera storage quota. Mirrors the global size cap (step 3) but scoped to one camera:
    //    keep each capped camera's deletable footprint within its quota by pruning its oldest
    //    unlocked segments. Evidence-locked footage (`evidence_locked = 1`) is protected and counts
    //    against the quota; if it alone meets or exceeds the quota, we warn and delete nothing rather
    //    than wiping the camera's other footage. Only cameras with `storage_quota_bytes IS NOT NULL`
    //    are capped here; the rest are governed solely by the global cap + disk floor below.
    let mut quota_deleted: u64 = 0;
    let quota_cams: Vec<(String, i64)> = sqlx::query_as(
        "SELECT id, storage_quota_bytes FROM cameras WHERE storage_quota_bytes IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;
    for (cam_id, quota) in quota_cams {
        if quota <= 0 {
            continue;
        }
        let protected_bytes: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(size_bytes), 0) FROM segments WHERE camera_id = ? AND evidence_locked = 1",
        )
        .bind(&cam_id)
        .fetch_one(pool)
        .await?;
        let budget = quota - protected_bytes;
        if budget <= 0 {
            if protected_bytes > quota {
                tracing::warn!(
                    camera_id = %cam_id,
                    protected_bytes,
                    quota,
                    "retention: evidence-locked footage exceeds the camera quota; not deleting other footage"
                );
                let _ = repo::log_event(
                    pool,
                    Some(&cam_id),
                    "disk_pressure",
                    "warning",
                    json!({ "reason": "camera_quota", "camera_id": &cam_id, "protected_bytes": protected_bytes, "quota_bytes": quota }),
                )
                .await;
            }
            continue;
        }
        loop {
            let deletable_total: i64 = sqlx::query_scalar(
                "SELECT COALESCE(SUM(size_bytes), 0) FROM segments WHERE camera_id = ? AND locked = 0 AND evidence_locked = 0",
            )
            .bind(&cam_id)
            .fetch_one(pool)
            .await?;
            if deletable_total <= budget {
                break;
            }
            let batch: Vec<(String, String)> = sqlx::query_as(
                "SELECT id, path FROM segments WHERE camera_id = ? AND locked = 0 AND evidence_locked = 0 ORDER BY end_time ASC LIMIT 20",
            )
            .bind(&cam_id)
            .fetch_all(pool)
            .await?;
            if batch.is_empty() {
                break;
            }
            let mut progressed = 0u64;
            for (seg_id, path) in batch {
                if unlink_segment(&path).await {
                    sqlx::query("DELETE FROM segments WHERE id = ?")
                        .bind(&seg_id)
                        .execute(pool)
                        .await?;
                    quota_deleted += 1;
                    progressed += 1;
                }
            }
            if progressed == 0 {
                tracing::error!(camera_id = %cam_id, "retention: camera-quota prune made no progress (segment file deletes failing); stopping this camera");
                break;
            }
        }
    }
    if quota_deleted > 0 {
        let _ = repo::log_event(
            pool,
            None,
            "disk_pressure",
            "warning",
            json!({ "deleted": quota_deleted, "reason": "camera_quota" }),
        )
        .await;
        tracing::warn!(
            deleted = quota_deleted,
            "retention: per-camera quota cleanup"
        );
    }

    // 3) Global size cap: prune the oldest DELETABLE segments until the deletable footprint fits the
    //    budget. The budget is the cap minus the evidence-locked bytes we cannot delete — counting
    //    those in the comparison would otherwise make us delete every deletable segment. We measure
    //    the protected footprint by `evidence_locked = 1` (the DURABLE hold), not the transient
    //    `locked` read-lock: an in-flight export must not inflate the protected total and starve the
    //    cap. Deletable = `locked = 0 AND evidence_locked = 0` (skip both the read-lock and the hold).
    let max = cfg.max_recordings_bytes as i64;
    let protected_bytes: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(size_bytes), 0) FROM segments WHERE evidence_locked = 1",
    )
    .fetch_one(pool)
    .await?;
    let budget = max - protected_bytes;
    let mut size_deleted: u64 = 0;

    if budget <= 0 {
        // Evidence-locked footage alone meets or exceeds the cap; deleting other footage cannot
        // help. Warn instead of wiping everything.
        let unlocked: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(size_bytes), 0) FROM segments WHERE locked = 0 AND evidence_locked = 0",
        )
        .fetch_one(pool)
        .await?;
        if protected_bytes > max {
            tracing::warn!(
                protected_bytes,
                max,
                "retention: evidence-locked footage exceeds the size cap; not deleting other footage"
            );
            let _ = repo::log_event(
                pool,
                None,
                "disk_pressure",
                "warning",
                json!({ "reason": "locked_exceeds_cap", "protected_bytes": protected_bytes, "unlocked_bytes": unlocked, "max_bytes": max }),
            )
            .await;
        }
    } else {
        loop {
            let unlocked_total: i64 = sqlx::query_scalar(
                "SELECT COALESCE(SUM(size_bytes), 0) FROM segments WHERE locked = 0 AND evidence_locked = 0",
            )
            .fetch_one(pool)
            .await?;
            if unlocked_total <= budget {
                break;
            }
            let batch: Vec<(String, String)> = sqlx::query_as(
                "SELECT id, path FROM segments WHERE locked = 0 AND evidence_locked = 0 ORDER BY end_time ASC LIMIT 20",
            )
            .fetch_all(pool)
            .await?;
            if batch.is_empty() {
                break;
            }
            let mut progressed = 0u64;
            for (seg_id, path) in batch {
                if unlink_segment(&path).await {
                    sqlx::query("DELETE FROM segments WHERE id = ?")
                        .bind(&seg_id)
                        .execute(pool)
                        .await?;
                    size_deleted += 1;
                    progressed += 1;
                }
            }
            if progressed == 0 {
                // Every file in the batch failed to delete; we'd re-select the same rows forever.
                tracing::error!("retention: size-cap prune made no progress (segment file deletes failing); stopping this sweep");
                break;
            }
        }
    }

    if size_deleted > 0 {
        let _ = repo::log_event(
            pool,
            None,
            "disk_pressure",
            "warning",
            json!({ "deleted": size_deleted, "reason": "size_cap", "max_bytes": max }),
        )
        .await;
        tracing::warn!(deleted = size_deleted, "retention: size-cap cleanup");
    }

    // 4) Disk-free floor: if the recordings filesystem drops below the free-space floor, prune the
    //    oldest unlocked segments until back above it. Self-limiting: it stops if a delete batch
    //    does not actually recover free space (disk filled by non-recording data), and refuses to
    //    run if the floor exceeds the whole disk — so it never destroys the footprint for nothing.
    let floor = cfg.min_free_disk_bytes;
    let mut disk_deleted: u64 = 0;
    match storage::disk_stats_async(cfg.recordings_dir.clone()).await {
        None => {
            tracing::warn!(
                "retention: could not read disk stats; free-floor check skipped this sweep"
            );
            let _ = repo::log_event(
                pool,
                None,
                "disk_pressure",
                "warning",
                json!({ "reason": "disk_stats_unavailable" }),
            )
            .await;
        }
        Some(d) if floor >= d.total_bytes => {
            if d.free_bytes < floor {
                tracing::warn!(
                    floor,
                    total = d.total_bytes,
                    "retention: free-disk floor exceeds total disk size; refusing to prune (misconfigured?)"
                );
                let _ = repo::log_event(
                    pool,
                    None,
                    "disk_pressure",
                    "critical",
                    json!({ "reason": "floor_unsatisfiable", "min_free_bytes": floor, "total_bytes": d.total_bytes }),
                )
                .await;
            }
        }
        Some(mut prev) => {
            let mut guard = 0;
            let mut futile = false;
            while prev.free_bytes < floor && guard < 200 {
                guard += 1;
                let before = prev.free_bytes;
                let batch: Vec<(String, String)> = sqlx::query_as(
                    "SELECT id, path FROM segments WHERE locked = 0 AND evidence_locked = 0 ORDER BY end_time ASC LIMIT 20",
                )
                .fetch_all(pool)
                .await?;
                if batch.is_empty() {
                    tracing::warn!(
                        free_bytes = before,
                        floor,
                        "retention: below disk-free floor but no deletable segments remain to prune"
                    );
                    break;
                }
                for (seg_id, path) in batch {
                    if unlink_segment(&path).await {
                        sqlx::query("DELETE FROM segments WHERE id = ?")
                            .bind(&seg_id)
                            .execute(pool)
                            .await?;
                        disk_deleted += 1;
                    }
                }
                match storage::disk_stats_async(cfg.recordings_dir.clone()).await {
                    Some(d) if d.free_bytes > before => prev = d,
                    Some(_) => {
                        futile = true;
                        break;
                    }
                    None => break,
                }
            }
            if futile {
                tracing::error!(
                    free_bytes = prev.free_bytes,
                    floor,
                    "retention: pruning recordings is not recovering free space (disk filled by non-recording data?); stopping"
                );
                let _ = repo::log_event(
                    pool,
                    None,
                    "disk_pressure",
                    "critical",
                    json!({ "reason": "prune_not_recovering_space", "min_free_bytes": floor, "deleted": disk_deleted }),
                )
                .await;
            }
        }
    }
    if disk_deleted > 0 {
        let _ = repo::log_event(
            pool,
            None,
            "disk_pressure",
            "critical",
            json!({ "deleted": disk_deleted, "reason": "free_floor", "min_free_bytes": floor }),
        )
        .await;
        tracing::warn!(deleted = disk_deleted, "retention: disk-free-floor cleanup");
    }

    // 5) Prune old AI detections (the table grows unbounded otherwise).
    let det_cutoff = Utc::now() - chrono::Duration::hours(cfg.detection_retention_hours.max(1));
    let pruned = sqlx::query("DELETE FROM detections WHERE created_at < ?")
        .bind(det_cutoff)
        .execute(pool)
        .await?
        .rows_affected();
    if pruned > 0 {
        tracing::info!(deleted = pruned, "retention: pruned old detections");
    }
    // Prune the transactional outbox on the same TTL (until an edge→cloud relay acks + prunes by seq).
    let ob_pruned = sqlx::query("DELETE FROM outbox WHERE created_at < ?")
        .bind(det_cutoff)
        .execute(pool)
        .await?
        .rows_affected();
    if ob_pruned > 0 {
        tracing::info!(deleted = ob_pruned, "retention: pruned old outbox rows");
    }

    // 6) Prune old zone events and delete their evidence frames (same TTL as detections).
    let old_zone_events: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT id, evidence_path FROM zone_events WHERE created_at < ?")
            .bind(det_cutoff)
            .fetch_all(pool)
            .await?;
    if !old_zone_events.is_empty() {
        for (_id, evidence) in &old_zone_events {
            if let Some(name) = evidence.as_deref().and_then(|u| u.rsplit('/').next()) {
                let _ = tokio::fs::remove_file(cfg.snapshots_dir.join(name)).await;
            }
        }
        let zpruned = sqlx::query("DELETE FROM zone_events WHERE created_at < ?")
            .bind(det_cutoff)
            .execute(pool)
            .await?
            .rows_affected();
        tracing::info!(
            deleted = zpruned,
            "retention: pruned old zone events + evidence"
        );
    }

    // 7) Prune kernel auth bookkeeping: stale audit log + expired sessions. (Domain entry events +
    //    their evidence frames are pruned by the entry app's own retention loop, not the kernel.)
    let audit_cutoff = Utc::now() - chrono::Duration::days(cfg.audit_retention_days.max(1));
    let apruned = sqlx::query("DELETE FROM audit_log WHERE created_at < ?")
        .bind(audit_cutoff)
        .execute(pool)
        .await?
        .rows_affected();
    if apruned > 0 {
        tracing::info!(deleted = apruned, "retention: pruned old audit log entries");
    }
    let spruned = sqlx::query("DELETE FROM sessions WHERE expires_at < ?")
        .bind(Utc::now())
        .execute(pool)
        .await?
        .rows_affected();
    if spruned > 0 {
        tracing::debug!(deleted = spruned, "retention: pruned expired sessions");
    }

    // 8) Prune the generic event log (camera-status events, disk-pressure warnings, and the entry
    //    mirrors written by the ANPR engine). It is otherwise unbounded. The alert notifier advances
    //    a durable cursor over recent rows, so deleting rows older than the (long) entry TTL — which
    //    are far past delivery — is safe.
    let evpruned = sqlx::query("DELETE FROM events WHERE created_at < ?")
        .bind(audit_cutoff)
        .execute(pool)
        .await?
        .rows_affected();
    if evpruned > 0 {
        tracing::info!(deleted = evpruned, "retention: pruned old event-log rows");
    }

    // 9) Prune scheduled snapshots past their retention window. The cutoff is `taken_at` (capture
    //    time, not the row's `created_at`). Delete the file first; only drop the DB row when the
    //    file is gone (mirrors the segment unlink pattern). Skipped entirely when hours = 0.
    if cfg.snapshot_retention_hours > 0 {
        let snap_cutoff = Utc::now() - chrono::Duration::hours(cfg.snapshot_retention_hours);
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT id, path FROM snapshots WHERE taken_at < ?")
                .bind(snap_cutoff)
                .fetch_all(pool)
                .await?;
        let mut snap_deleted: u64 = 0;
        for (snap_id, path) in rows {
            if unlink_snapshot(&path).await {
                sqlx::query("DELETE FROM snapshots WHERE id = ?")
                    .bind(&snap_id)
                    .execute(pool)
                    .await?;
                snap_deleted += 1;
            }
        }
        if snap_deleted > 0 {
            tracing::info!(deleted = snap_deleted, "retention: pruned old snapshots");
        }
    }

    // 10) Prune on-demand archive exports + finished backup-job rows past the archive retention
    //     window. Delete the .zip files by mtime, then drop any backup_jobs that have finished before
    //     the cutoff (both policy runs and archive exports). Skipped entirely when hours = 0.
    if cfg.archive_retention_hours > 0 {
        let cutoff = Utc::now() - chrono::Duration::hours(cfg.archive_retention_hours);
        if let Ok(mut entries) = tokio::fs::read_dir(&cfg.archive_dir).await {
            let mut removed: u64 = 0;
            while let Ok(Some(ent)) = entries.next_entry().await {
                let path = ent.path();
                if path.extension().and_then(|e| e.to_str()) != Some("zip") {
                    continue;
                }
                let stale = ent
                    .metadata()
                    .await
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map(|t| DateTime::<Utc>::from(t) < cutoff)
                    .unwrap_or(false);
                if stale && tokio::fs::remove_file(&path).await.is_ok() {
                    removed += 1;
                }
            }
            if removed > 0 {
                tracing::info!(deleted = removed, "retention: pruned old archive exports");
            }
        }
        let jpruned = sqlx::query(
            "DELETE FROM backup_jobs WHERE finished_at IS NOT NULL AND finished_at < ?",
        )
        .bind(cutoff)
        .execute(pool)
        .await?
        .rows_affected();
        if jpruned > 0 {
            tracing::info!(
                deleted = jpruned,
                "retention: pruned old finished backup jobs"
            );
        }
    }
    Ok(())
}
