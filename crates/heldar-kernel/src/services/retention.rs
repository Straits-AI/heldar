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
use crate::services::{settings, storage};

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

/// Remove one segment row IF it is still unlocked, then best-effort delete its file. Returns
/// whether the row was removed.
///
/// The conditional `DELETE ... WHERE locked = 0 AND evidence_locked = 0` is a TOCTOU guard: SQLite
/// serializes it against the incident/export lock `UPDATE`s, so an evidence-hold or export
/// read-lock that commits AFTER this segment was selected for pruning wins the race — `rows_affected`
/// is 0 and the file is never touched. Only when the row is actually removed do we unlink the file.
/// A rare unlink failure then orphans the file (the `path` column is UNIQUE, so an orphan sweep can
/// reclaim it) — strictly preferable to ever deleting protected evidence.
async fn delete_segment_if_unlocked(
    pool: &SqlitePool,
    seg_id: &str,
    path: &str,
) -> anyhow::Result<bool> {
    let removed =
        sqlx::query("DELETE FROM segments WHERE id = ? AND locked = 0 AND evidence_locked = 0")
            .bind(seg_id)
            .execute(pool)
            .await?
            .rows_affected();
    if removed == 1 {
        unlink_segment(path).await;
        Ok(true)
    } else {
        Ok(false)
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
    // Orphan reclamation walks the recordings tree, so run it on a slower (~hourly) cadence than the
    // row-based sweep, not on every tick.
    let orphan_every = (3600u64 / cfg.retention_interval_s.max(30)).max(1);
    let mut sweeps: u64 = 0;
    loop {
        tick.tick().await;
        if let Err(e) = sweep(&pool, &cfg).await {
            tracing::error!(error = %e, "retention: sweep failed");
        }
        if sweeps % orphan_every == 0 {
            if let Err(e) = reclaim_orphans(&pool, &cfg).await {
                tracing::warn!(error = %e, "retention: orphan reclamation failed");
            }
        }
        sweeps = sweeps.wrapping_add(1);
    }
}

/// Reclaim orphaned recording files — `.mp4` segments on disk under `recordings_dir/<camera>/` with no
/// row in `segments.path`. These escape the row-based retention tiers, so left alone they accumulate
/// and push the disk-free floor into evicting *tracked* recordings. Conservative by design: a file is
/// eligible only when it is older than `ORPHAN_MIN_AGE` (well past the indexer's settle window), so an
/// in-flight or just-finished-but-not-yet-indexed segment is never touched. Emits a `recording_orphans`
/// divergence event whenever any are reclaimed.
async fn reclaim_orphans(pool: &SqlitePool, cfg: &Config) -> anyhow::Result<()> {
    /// Minimum file age before an unindexed segment is treated as a true orphan rather than in-flight.
    const ORPHAN_MIN_AGE: Duration = Duration::from_secs(3600);

    // Exact path strings as the indexer stores them (`ent.path().to_string_lossy()`), so on-disk paths
    // built the same way below compare correctly.
    let known: std::collections::HashSet<String> =
        sqlx::query_scalar::<_, String>("SELECT path FROM segments")
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();

    let cams: Vec<(String,)> = sqlx::query_as("SELECT id FROM cameras")
        .fetch_all(pool)
        .await?;
    let now = std::time::SystemTime::now();
    let mut count = 0u64;
    let mut bytes = 0u64;
    for (cam_id,) in cams {
        let dir = cfg.camera_recordings_dir(&cam_id);
        let mut rd = match tokio::fs::read_dir(&dir).await {
            Ok(rd) => rd,
            Err(_) => continue, // no directory yet for this camera
        };
        while let Ok(Some(ent)) = rd.next_entry().await {
            let path = ent.path();
            if path.extension().and_then(|e| e.to_str()) != Some("mp4") {
                continue;
            }
            let path_str = path.to_string_lossy().to_string();
            if known.contains(&path_str) {
                continue; // tracked by a segment row — cheap skip before stat
            }
            let md = match ent.metadata().await {
                Ok(md) if md.is_file() => md,
                _ => continue,
            };
            let age = md.modified().ok().and_then(|m| now.duration_since(m).ok());
            if !orphan_is_reclaimable(&path_str, &known, age, ORPHAN_MIN_AGE) {
                continue;
            }
            // Re-check membership immediately before deleting: a segment row inserted between the
            // snapshot above and now must keep its file (closes the snapshot-then-walk race). On a
            // query error, assume tracked (fail safe) and skip.
            let still_orphan: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM segments WHERE path = ?")
                    .bind(&path_str)
                    .fetch_one(pool)
                    .await
                    .unwrap_or(1);
            if still_orphan != 0 {
                continue;
            }
            let sz = md.len();
            if unlink_segment(&path_str).await {
                count += 1;
                bytes = bytes.saturating_add(sz);
            }
        }
    }

    if count > 0 {
        tracing::warn!(
            orphans = count,
            bytes,
            "retention: reclaimed orphaned recording files (on disk but not indexed)"
        );
        let _ = repo::log_event(
            pool,
            None,
            "recording_orphans",
            "warning",
            json!({ "reclaimed": count, "bytes": bytes }),
        )
        .await;
    }
    Ok(())
}

/// Whether an on-disk recording file is a reclaimable orphan: not tracked by any segment row AND older
/// than `min_age` (so an in-flight or just-finished-but-not-yet-indexed file is never eligible). `age`
/// is the file's age, or `None` when its mtime could not be read — treated as NOT reclaimable, so an
/// unreadable timestamp fails safe (we keep the file).
fn orphan_is_reclaimable(
    path: &str,
    known: &std::collections::HashSet<String>,
    age: Option<Duration>,
    min_age: Duration,
) -> bool {
    !known.contains(path) && age.map(|a| a >= min_age).unwrap_or(false)
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
            if delete_segment_if_unlocked(pool, &seg_id, &path).await? {
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
            let batch: Vec<(String, String, i64)> = sqlx::query_as(
                "SELECT id, path, size_bytes FROM segments WHERE camera_id = ? AND locked = 0 AND evidence_locked = 0 ORDER BY end_time ASC LIMIT 20",
            )
            .bind(&cam_id)
            .fetch_all(pool)
            .await?;
            if batch.is_empty() {
                break;
            }
            let mut remaining = deletable_total;
            let mut progressed = 0u64;
            for (seg_id, path, size) in batch {
                // Stop the instant the budget is met — never over-prune within a batch. The oldest
                // segments are deleted first; once enough have gone to bring the deletable footprint
                // to-or-under budget, the rest are within quota and must be kept (footage is
                // unrecoverable on a DVR).
                if remaining <= budget {
                    break;
                }
                if delete_segment_if_unlocked(pool, &seg_id, &path).await? {
                    remaining -= size;
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
    // Operator-tunable from the dashboard (settings table); a positive override wins, else the env
    // default (`HELDAR_MAX_RECORDINGS_GB`). Non-positive overrides are ignored so a stray 0 can't
    // silently disable the cap.
    let max = settings::get_i64(pool, settings::RECORDING_MAX_BYTES)
        .await
        .filter(|&v| v > 0)
        .unwrap_or(cfg.max_recordings_bytes as i64);
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
            let batch: Vec<(String, String, i64)> = sqlx::query_as(
                "SELECT id, path, size_bytes FROM segments WHERE locked = 0 AND evidence_locked = 0 ORDER BY end_time ASC LIMIT 20",
            )
            .fetch_all(pool)
            .await?;
            if batch.is_empty() {
                break;
            }
            let mut remaining = unlocked_total;
            let mut progressed = 0u64;
            for (seg_id, path, size) in batch {
                // Stop the instant the global cap is satisfied — never over-prune within a batch.
                if remaining <= budget {
                    break;
                }
                if delete_segment_if_unlocked(pool, &seg_id, &path).await? {
                    remaining -= size;
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
    // Operator-tunable free-disk floor (settings table); env default `HELDAR_MIN_FREE_DISK_GB` otherwise.
    // 0 is a valid override meaning "no floor".
    let floor = settings::get_i64(pool, settings::RECORDING_MIN_FREE_BYTES)
        .await
        .filter(|&v| v >= 0)
        .map(|v| v as u64)
        .unwrap_or(cfg.min_free_disk_bytes);
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
            // Scale each prune batch to the remaining deficit (bounded) so a large fill recovers in a
            // few passes instead of 20 segments at a time. The free-space re-check after each batch
            // keeps over-prune to roughly one batch of the oldest segments.
            let avg_seg: i64 = sqlx::query_scalar(
                "SELECT CAST(COALESCE(AVG(size_bytes), 0) AS INTEGER) FROM segments WHERE locked = 0 AND evidence_locked = 0",
            )
            .fetch_one(pool)
            .await
            .unwrap_or(0);
            let avg_seg = (avg_seg.max(1)) as u64;
            while prev.free_bytes < floor && guard < 200 {
                guard += 1;
                let before = prev.free_bytes;
                let deficit = floor.saturating_sub(before);
                let want = (deficit / avg_seg).clamp(20, 256) as i64;
                let batch: Vec<(String, String)> = sqlx::query_as(
                    "SELECT id, path FROM segments WHERE locked = 0 AND evidence_locked = 0 ORDER BY end_time ASC LIMIT ?",
                )
                .bind(want)
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
                    if delete_segment_if_unlocked(pool, &seg_id, &path).await? {
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

    // 8b) Prune the webhook delivery ledger (one row per delivery attempt, per subscription, per event)
    //     past the audit horizon. The delivery cursor lives on the subscription, not these rows, so
    //     deleting old attempt records is safe — they are an at-rest audit trail, not delivery state.
    let wdpruned = sqlx::query("DELETE FROM webhook_deliveries WHERE created_at < ?")
        .bind(audit_cutoff)
        .execute(pool)
        .await?
        .rows_affected();
    if wdpruned > 0 {
        tracing::info!(
            deleted = wdpruned,
            "retention: pruned old webhook-delivery rows"
        );
    }

    // 8c) Prune RESOLVED recording-gap rows (filled/failed) past the audit horizon. Pending gaps are
    //     left for the ANR re-fill engine to act on (they age out of its query via anr_max_gap_hours).
    let gpruned = sqlx::query(
        "DELETE FROM recording_gaps WHERE fill_state IN ('filled','failed') AND created_at < ?",
    )
    .bind(audit_cutoff)
    .execute(pool)
    .await?
    .rows_affected();
    if gpruned > 0 {
        tracing::info!(
            deleted = gpruned,
            "retention: pruned resolved recording-gap rows"
        );
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

    // 10) Metadata-DB size cap: after the row-retention deletes above have freed pages, checkpoint
    //     the WAL, reclaim, and if the DB file is still over its cap shed the oldest detections
    //     (events/audit are protected). Self-bounds heldar.db the way step 3 bounds recordings.
    match crate::services::db_maintenance::enforce_db_cap(pool, cfg).await {
        Ok(rep) if rep.detections_deleted > 0 => {
            tracing::info!(
                deleted = rep.detections_deleted,
                bytes = rep.final_bytes,
                over_cap = rep.over_cap,
                "retention: db size-cap prune"
            );
            let _ = repo::log_event(
                pool,
                None,
                "db_retention",
                if rep.over_cap { "warning" } else { "info" },
                json!({ "detections_deleted": rep.detections_deleted, "db_bytes": rep.final_bytes, "over_cap": rep.over_cap }),
            )
            .await;
        }
        Ok(_) => {}
        Err(e) => tracing::error!(error = %e, "retention: db size-cap step failed"),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- helpers -------------------------------------------------------

    fn unique_path(prefix: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("{prefix}-{}-{n}", std::process::id()))
    }

    async fn mem_pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        pool
    }

    /// A Config wired so that ONLY age-retention (step 1) and per-camera quota (step 2) can act:
    /// the global size cap is effectively infinite, the disk-free floor is 0 (step 4 never deletes,
    /// regardless of whether statvfs succeeds), and snapshot/archive prunes are disabled.
    fn test_cfg() -> Config {
        let mut cfg = Config::from_env();
        cfg.max_recordings_bytes = u64::MAX / 4;
        cfg.min_free_disk_bytes = 0;
        cfg.recordings_dir = std::env::temp_dir();
        cfg.snapshot_retention_hours = 0;
        cfg.archive_retention_hours = 0;
        cfg.detection_retention_hours = 168;
        cfg.audit_retention_days = 365;
        cfg
    }

    async fn insert_camera(pool: &SqlitePool, id: &str, retention_hours: i64, quota: Option<i64>) {
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO cameras (id, name, retention_hours, storage_quota_bytes, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(id)
        .bind(retention_hours)
        .bind(quota)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_segment(
        pool: &SqlitePool,
        id: &str,
        camera_id: &str,
        end: DateTime<Utc>,
        size_bytes: i64,
        locked: i64,
        evidence_locked: i64,
    ) {
        sqlx::query(
            "INSERT INTO segments
                (id, camera_id, path, start_time, end_time, duration_s, size_bytes, locked, evidence_locked, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(camera_id)
        // points at a file that does not exist -> unlink_segment hits the NotFound->true branch.
        .bind(format!("/nonexistent/heldar-test/{id}.mp4"))
        .bind(end)
        .bind(end)
        .bind(60.0_f64)
        .bind(size_bytes)
        .bind(locked)
        .bind(evidence_locked)
        .bind(end)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn seg_exists(pool: &SqlitePool, id: &str) -> bool {
        let c: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM segments WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
        c == 1
    }

    async fn seg_count(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM segments")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn event_type_count(pool: &SqlitePool, event_type: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE event_type = ?")
            .bind(event_type)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn camera_quota_event_count(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE event_type = 'disk_pressure' AND payload LIKE '%camera_quota%'",
        )
        .fetch_one(pool)
        .await
        .unwrap()
    }

    // ----- unlink helpers ------------------------------------------------

    #[tokio::test]
    async fn unlink_segment_reports_removable_for_missing_path() {
        // Already-absent file: the DB row should be removed (returns true).
        assert!(unlink_segment("/nonexistent/heldar/definitely-not-here.mp4").await);
    }

    #[tokio::test]
    async fn unlink_segment_deletes_existing_file() {
        let p = unique_path("heldar-seg");
        tokio::fs::write(&p, b"x").await.unwrap();
        assert!(p.exists());
        assert!(unlink_segment(p.to_str().unwrap()).await);
        assert!(!p.exists());
    }

    #[tokio::test]
    async fn unlink_segment_keeps_row_for_directory() {
        // remove_file on a directory fails with a non-NotFound error -> keep the row (false).
        let d = unique_path("heldar-dir");
        tokio::fs::create_dir(&d).await.unwrap();
        assert!(!unlink_segment(d.to_str().unwrap()).await);
        assert!(d.exists());
        let _ = tokio::fs::remove_dir(&d).await;
    }

    #[tokio::test]
    async fn unlink_snapshot_handles_missing_and_existing() {
        // Mirrors unlink_segment: missing -> true; existing -> deleted + true.
        assert!(unlink_snapshot("/nonexistent/heldar/none.jpg").await);
        let p = unique_path("heldar-snap");
        tokio::fs::write(&p, b"x").await.unwrap();
        assert!(unlink_snapshot(p.to_str().unwrap()).await);
        assert!(!p.exists());
    }

    #[test]
    fn orphan_is_reclaimable_only_old_and_untracked() {
        // The data-loss-safety predicate behind reclaim_orphans: only a file that is BOTH untracked
        // (no segment row) AND older than the age guard may be deleted.
        let mut known = std::collections::HashSet::new();
        known.insert("/rec/cam/tracked.mp4".to_string());
        let min = Duration::from_secs(3600);
        let old = Some(Duration::from_secs(7200)); // 2h
        let recent = Some(Duration::from_secs(60)); // in-flight / just finished

        assert!(
            !orphan_is_reclaimable("/rec/cam/tracked.mp4", &known, old, min),
            "a tracked file is never reclaimed, even when old"
        );
        assert!(
            !orphan_is_reclaimable("/rec/cam/inflight.mp4", &known, recent, min),
            "an untracked but recent file is spared (not yet indexed)"
        );
        assert!(
            orphan_is_reclaimable("/rec/cam/orphan.mp4", &known, old, min),
            "untracked + old is the only reclaimable case"
        );
        assert!(
            !orphan_is_reclaimable("/rec/cam/nomtime.mp4", &known, None, min),
            "an unreadable mtime fails safe (kept)"
        );
        assert!(
            orphan_is_reclaimable("/rec/cam/edge.mp4", &known, Some(min), min),
            "exactly at the threshold is old enough"
        );
    }

    // ----- sweep: age retention -----------------------------------------

    #[tokio::test]
    async fn sweep_age_retention_deletes_only_old_unlocked() {
        let pool = mem_pool().await;
        let cfg = test_cfg();
        let now = Utc::now();

        insert_camera(&pool, "cam_age", 24, None).await;
        // Recent unlocked segment: kept (newer than the 24h cutoff).
        insert_segment(
            &pool,
            "seg_recent",
            "cam_age",
            now - chrono::Duration::hours(1),
            100,
            0,
            0,
        )
        .await;
        // Old unlocked segment: deleted by age policy.
        insert_segment(
            &pool,
            "seg_old",
            "cam_age",
            now - chrono::Duration::hours(48),
            100,
            0,
            0,
        )
        .await;
        // Old but read-locked (transient export lock): excluded from age prune -> kept.
        insert_segment(
            &pool,
            "seg_old_locked",
            "cam_age",
            now - chrono::Duration::hours(48),
            100,
            1,
            0,
        )
        .await;
        // Old but evidence-locked (durable hold): excluded from age prune -> kept.
        insert_segment(
            &pool,
            "seg_old_ev",
            "cam_age",
            now - chrono::Duration::hours(48),
            100,
            0,
            1,
        )
        .await;

        sweep(&pool, &cfg).await.unwrap();

        assert!(seg_exists(&pool, "seg_recent").await);
        assert!(
            !seg_exists(&pool, "seg_old").await,
            "old unlocked segment should be pruned by age"
        );
        assert!(
            seg_exists(&pool, "seg_old_locked").await,
            "read-locked segment must survive age prune"
        );
        assert!(
            seg_exists(&pool, "seg_old_ev").await,
            "evidence-locked segment must survive age prune"
        );
        assert_eq!(seg_count(&pool).await, 3);
        // age_deleted > 0 logs exactly one retention_delete event for the sweep.
        assert_eq!(event_type_count(&pool, "retention_delete").await, 1);
    }

    // ----- sweep: per-camera quota --------------------------------------

    #[tokio::test]
    async fn sweep_camera_quota_prunes_only_to_budget_keeps_evidence() {
        let pool = mem_pool().await;
        let cfg = test_cfg();
        let now = Utc::now();

        // Huge retention so age policy never fires; only the quota acts here.
        insert_camera(&pool, "cam_q", 100_000, Some(1000)).await;
        // Protected (evidence-locked) footage counts against the quota but is never deleted.
        insert_segment(
            &pool,
            "sL",
            "cam_q",
            now - chrono::Duration::hours(5),
            600,
            0,
            1,
        )
        .await;
        // Three deletable segments (total 1200) over the budget (quota 1000 - protected 600 = 400).
        insert_segment(
            &pool,
            "s1",
            "cam_q",
            now - chrono::Duration::hours(3),
            400,
            0,
            0,
        )
        .await;
        insert_segment(
            &pool,
            "s2",
            "cam_q",
            now - chrono::Duration::hours(2),
            400,
            0,
            0,
        )
        .await;
        insert_segment(
            &pool,
            "s3",
            "cam_q",
            now - chrono::Duration::hours(1),
            400,
            0,
            0,
        )
        .await;

        sweep(&pool, &cfg).await.unwrap();

        // Correctness invariant: prune ONLY enough oldest segments to reach budget (400), then stop.
        // Deleting s1+s2 brings the deletable footprint to exactly 400 == budget, so s3 is within
        // quota and MUST be kept; pruning it would needlessly destroy recoverable footage.
        assert!(
            seg_exists(&pool, "sL").await,
            "evidence-locked footage must survive the quota prune"
        );
        assert!(
            !seg_exists(&pool, "s1").await,
            "oldest over-budget segment is pruned"
        );
        assert!(
            !seg_exists(&pool, "s2").await,
            "second-oldest pruned to reach budget"
        );
        assert!(
            seg_exists(&pool, "s3").await,
            "s3 is within quota once s1+s2 are gone and must NOT be over-deleted"
        );
        assert_eq!(
            seg_count(&pool).await,
            2,
            "only s1,s2 pruned to reach budget; sL+s3 remain"
        );
        assert!(
            camera_quota_event_count(&pool).await >= 1,
            "a camera_quota disk_pressure event should be logged"
        );
    }

    #[tokio::test]
    async fn delete_segment_if_unlocked_spares_locked_rows() {
        // The TOCTOU guard: the conditional DELETE must refuse a row that became evidence-locked
        // (or read-locked) since it was selected for pruning, and remove an unlocked one. This is
        // the atomic primitive that makes pruning safe against a hold committing mid-sweep.
        let pool = mem_pool().await;
        let now = Utc::now();
        insert_camera(&pool, "cam_t", 100_000, None).await;
        insert_segment(&pool, "held", "cam_t", now, 100, 0, 1).await; // evidence_locked = 1
        insert_segment(&pool, "rlok", "cam_t", now, 100, 1, 0).await; // locked = 1 (export read-lock)
        insert_segment(&pool, "free", "cam_t", now, 100, 0, 0).await; // deletable

        assert!(
            !delete_segment_if_unlocked(&pool, "held", "/nonexistent/held.mp4")
                .await
                .unwrap(),
            "evidence-locked row must not be removable"
        );
        assert!(
            !delete_segment_if_unlocked(&pool, "rlok", "/nonexistent/rlok.mp4")
                .await
                .unwrap(),
            "read-locked row must not be removable"
        );
        assert!(seg_exists(&pool, "held").await);
        assert!(seg_exists(&pool, "rlok").await);

        assert!(
            delete_segment_if_unlocked(&pool, "free", "/nonexistent/free.mp4")
                .await
                .unwrap(),
            "unlocked row is removed"
        );
        assert!(!seg_exists(&pool, "free").await);
    }

    #[tokio::test]
    async fn sweep_camera_quota_protected_exceeds_deletes_nothing() {
        let pool = mem_pool().await;
        let cfg = test_cfg();
        let now = Utc::now();

        // Protected footage alone (500) exceeds the quota (100): deleting other footage cannot help,
        // so nothing is pruned and a warning is logged instead.
        insert_camera(&pool, "cam_over", 100_000, Some(100)).await;
        insert_segment(
            &pool,
            "ovL",
            "cam_over",
            now - chrono::Duration::hours(5),
            500,
            0,
            1,
        )
        .await;
        insert_segment(
            &pool,
            "ov1",
            "cam_over",
            now - chrono::Duration::hours(1),
            50,
            0,
            0,
        )
        .await;

        sweep(&pool, &cfg).await.unwrap();

        assert!(seg_exists(&pool, "ovL").await);
        assert!(
            seg_exists(&pool, "ov1").await,
            "other footage must not be wiped when protected footage exceeds the quota"
        );
        assert_eq!(seg_count(&pool).await, 2);
        assert!(
            camera_quota_event_count(&pool).await >= 1,
            "a camera_quota warning should be logged"
        );
    }

    // ----- sweep: detection pruning -------------------------------------

    #[tokio::test]
    async fn sweep_prunes_old_detections() {
        let pool = mem_pool().await;
        let cfg = test_cfg(); // detection_retention_hours = 168

        insert_camera(&pool, "cam_d", 24, None).await;
        let now = Utc::now();
        // Older than the 168h TTL -> pruned.
        sqlx::query(
            "INSERT INTO detections (id, camera_id, task_type, timestamp, created_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind("det_old")
        .bind("cam_d")
        .bind("object")
        .bind(now - chrono::Duration::hours(200))
        .bind(now - chrono::Duration::hours(200))
        .execute(&pool)
        .await
        .unwrap();
        // Recent -> kept.
        sqlx::query(
            "INSERT INTO detections (id, camera_id, task_type, timestamp, created_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind("det_new")
        .bind("cam_d")
        .bind("object")
        .bind(now - chrono::Duration::hours(1))
        .bind(now - chrono::Duration::hours(1))
        .execute(&pool)
        .await
        .unwrap();

        sweep(&pool, &cfg).await.unwrap();

        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM detections")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 1);
        let kept: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM detections WHERE id = ?")
            .bind("det_new")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(kept, 1, "the recent detection must be retained");
    }

    // ----- sweep: DB size-cap (step 10) ------------------------------------

    /// Verify that step 10 sheds oldest detections when the DB is over its cap, without
    /// touching protected tables (audit_log used here as the canary — no FK constraints).
    ///
    /// Key isolation choices:
    ///   - detection_retention_hours = 1_000_000  → step 5 age-prune deletes nothing (rows are recent)
    ///   - max_db_bytes = 1 MB                    → tiny cap so step 10 fires
    ///   - In-memory SQLite (auto_vacuum=NONE): incremental_vacuum is a no-op, so enforce_db_cap's
    ///     no-progress guard stops after the first batch — but 2500 rows < 5000 batch size, so the
    ///     whole set is deleted in one shot and the count assertion holds.
    #[tokio::test]
    async fn sweep_db_cap_sheds_detections_keeps_events() {
        let pool = mem_pool().await;
        let mut cfg = test_cfg();

        // Step 5 age-prune must be inert (rows are recent).
        cfg.detection_retention_hours = 1_000_000;
        // Tiny cap so step 10 fires.
        cfg.max_db_bytes = 1024 * 1024; // 1 MB

        // FK target for detections; huge camera retention so segment steps are inert.
        insert_camera(&pool, "cam1", 100_000, None).await;

        let now = Utc::now();

        // Seed ~2500 recent detections, each with a ~2 KB attributes blob.
        // 2500 rows × ~2 KB ≈ 5 MB of payload, well above the 1 MB cap.
        for i in 0_u32..2500 {
            sqlx::query(
                "INSERT INTO detections \
                 (id, camera_id, task_type, timestamp, attributes, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(format!("d{i:06}"))
            .bind("cam1")
            .bind("detection")
            .bind(now)
            .bind("x".repeat(2000))
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        }

        // Seed the protected table (audit_log has no FKs; step 10 never touches it).
        for i in 0_u32..5 {
            sqlx::query(
                "INSERT INTO audit_log (id, actor, action, detail, created_at) \
                 VALUES (?, 'system', 'test', '{}', ?)",
            )
            .bind(format!("al{i:04}"))
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        }

        let audit_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_log")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(audit_before, 5);

        sweep(&pool, &cfg).await.unwrap();

        let det_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM detections")
            .fetch_one(&pool)
            .await
            .unwrap();
        let audit_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_log")
            .fetch_one(&pool)
            .await
            .unwrap();

        assert!(
            det_after < 2500,
            "step 10 should have pruned detections (got {det_after})"
        );
        assert_eq!(
            audit_after, audit_before,
            "audit_log must be untouched by the DB size-cap step"
        );
    }
}
