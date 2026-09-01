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

/// Exported clips (`clip_<uuid>.mp4` + their `.txt` concat lists) live only on disk with no DB row, so
/// they escape every table-driven prune; retain them this long before reclaiming. A clip is a
/// download-and-go export, so a short window is plenty. (Constant rather than config to avoid churn;
/// promote to `HELDAR_CLIP_RETENTION_HOURS` if operators need to tune it.)
const CLIP_RETENTION: std::time::Duration = std::time::Duration::from_secs(48 * 3600);

/// Recursively delete files under `root` whose mtime is older than `max_age`. Best-effort: unreadable
/// entries are skipped, directories are descended (bounded by an explicit stack), and empty dirs are
/// left in place. Used to reclaim on-disk artifacts that carry no DB row (exported clips, mirror
/// segments), which the table-driven prunes and the size-cap can't see.
async fn prune_tree_older_than(root: &std::path::Path, max_age: std::time::Duration) -> u64 {
    let now = std::time::SystemTime::now();
    let mut deleted = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut rd = match tokio::fs::read_dir(&dir).await {
            Ok(r) => r,
            Err(_) => continue, // missing/unreadable dir (e.g. mirror not yet written) — skip
        };
        while let Ok(Some(entry)) = rd.next_entry().await {
            let Ok(ft) = entry.file_type().await else {
                continue;
            };
            if ft.is_dir() {
                stack.push(entry.path());
                continue;
            }
            let is_old = entry
                .metadata()
                .await
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|mtime| now.duration_since(mtime).ok())
                .map(|age| age > max_age)
                .unwrap_or(false);
            if is_old && tokio::fs::remove_file(entry.path()).await.is_ok() {
                deleted += 1;
            }
        }
    }
    deleted
}

/// Delete aged rows from a high-rate table in bounded batches, so a large backlog (worker catch-up,
/// clock skew, a stalled sweep) can't delete millions of rows in ONE transaction that holds SQLite's
/// single writer — stalling all ingest, zone-event and recording-metadata writes for its whole
/// duration. `where_clause` is a trusted static string with exactly one `?` bound to `cutoff`; the
/// table name is a trusted static too (never user input). Yields between batches so other writers
/// interleave. Returns the total rows deleted.
async fn delete_aged_in_batches(
    pool: &SqlitePool,
    table: &str,
    where_clause: &str,
    cutoff: DateTime<Utc>,
) -> sqlx::Result<u64> {
    const BATCH: i64 = 5_000;
    let sql = format!(
        "DELETE FROM {table} WHERE rowid IN (SELECT rowid FROM {table} WHERE {where_clause} LIMIT {BATCH})"
    );
    let mut total = 0u64;
    loop {
        let n = sqlx::query(&sql)
            .bind(cutoff)
            .execute(pool)
            .await?
            .rows_affected();
        total += n;
        if (n as i64) < BATCH {
            break;
        }
        tokio::task::yield_now().await;
    }
    Ok(total)
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

/// The camera that should give up footage next, when the box is over its size cap.
///
/// Eviction used to take the globally oldest deletable segment, which let ONE camera's authorised
/// state destroy another camera's recordings. Both routes were executed against a seeded box:
///
///   * a camera-scoped credential PATCHes `retention_hours` on its OWN camera (allowed, 200) so its
///     footage stops aging out; the cap is still exceeded, and the sweeper deletes the next-oldest
///     segments — which belong to a camera the caller never had access to. Observed: 100% of the
///     other camera's footage gone, files unlinked, ~30s after an authorised request.
///   * a camera-scoped credential evidence-locks its OWN segments (allowed, 200). `protected_bytes`
///     was summed FLEET-WIDE and subtracted from the shared budget, so the locks starved every other
///     camera's budget to zero.
///
/// Neither is a missing guard — every guard answered correctly. The damage happened afterwards, in a
/// loop that holds no principal and therefore has no scope to enforce. The fix has to be in the
/// eviction POLICY: the disk is genuinely shared, so something must go when it fills, but a camera
/// must only be able to spend its OWN share of it.
///
/// Each camera's share is the cap split across the cameras holding footage (weighted by
/// `storage_quota_bytes` where an operator has set one). A camera's protected (evidence-locked) bytes
/// count against ITS OWN share rather than everyone's, so locking evidence can no longer externalise
/// cost. The camera furthest over its share, and that still has something deletable, pays first.
///
/// Returns `None` when no camera is over its share — the caller then falls back to oldest-first,
/// which is correct precisely because nobody is behaving unfairly.
#[derive(Debug, PartialEq, Eq)]
enum ShareVerdict {
    /// This camera is furthest over its share and has deletable footage — take from it.
    Evict(String),
    /// Someone is over their share but ALL of it is evidence-locked. Deleting anything else would
    /// take footage from a camera that is behaving, to pay for one that is not, which is the exact
    /// cross-camera destruction this policy exists to prevent. Warn and stop instead — the same
    /// fail-safe the cap already takes when locked footage alone exceeds it.
    OverButProtected(String),
    /// Nobody is over their share: the box is simply full, and oldest-first is fair.
    Balanced,
}

async fn most_over_share(pool: &SqlitePool, cap: i64) -> ShareVerdict {
    let rows: Vec<(String, i64, i64, Option<i64>)> = sqlx::query_as(
        "SELECT c.id,
                COALESCE(SUM(CASE WHEN s.evidence_locked = 1 THEN s.size_bytes ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN s.locked = 0 AND s.evidence_locked = 0 THEN s.size_bytes ELSE 0 END), 0),
                c.storage_quota_bytes
           FROM cameras c JOIN segments s ON s.camera_id = c.id
          GROUP BY c.id",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    if rows.is_empty() {
        return ShareVerdict::Balanced;
    }
    // Weights: an operator-set quota expresses the intended split; otherwise every camera is equal.
    let total_weight: f64 = rows
        .iter()
        .map(|(_, _, _, q)| q.filter(|v| *v > 0).map(|v| v as f64).unwrap_or(1.0))
        .sum();
    let mut worst: Option<(String, f64, bool)> = None;
    for (id, protected, deletable, quota) in &rows {
        let weight = quota.filter(|v| *v > 0).map(|v| v as f64).unwrap_or(1.0);
        let share = (cap as f64) * (weight / total_weight.max(1.0));
        let over = (*protected + *deletable) as f64 - share;
        if over <= 0.0 {
            continue;
        }
        // Track the worst offender REGARDLESS of whether it has deletable bytes, so that a camera
        // which is over its share entirely in evidence locks is reported rather than skipped over in
        // favour of punishing someone else.
        if worst.as_ref().map(|(_, w, _)| over > *w).unwrap_or(true) {
            worst = Some((id.clone(), over, *deletable > 0));
        }
    }
    match worst {
        Some((id, _, true)) => ShareVerdict::Evict(id),
        Some((id, _, false)) => ShareVerdict::OverButProtected(id),
        None => ShareVerdict::Balanced,
    }
}

async fn sweep(pool: &SqlitePool, cfg: &Config) -> anyhow::Result<()> {
    // 0) Reclaim on-disk artifacts that carry NO DB row, so they can't quietly fill the volume and
    //    (worse) drive the disk-free floor below to evict real recordings to make room for them. This
    //    runs FIRST so the freed space is reflected before the floor decides how much footage to prune.
    //    (a) Exported clips: `clip_<uuid>.mp4`/`.txt` in clips_dir, older than CLIP_RETENTION.
    let clips_pruned = prune_tree_older_than(&cfg.clips_dir, CLIP_RETENTION).await;
    // Idempotency keys are bounded the same way everything else here is: they are a replay window,
    // not a second copy of the API's history.
    let keys_pruned = crate::idempotency::prune(pool).await;
    if keys_pruned > 0 {
        tracing::info!(keys_pruned, "retention: pruned expired idempotency keys");
    }

    // Forget the attribution rows whose artifact is gone. Runs AFTER the prunes above and before
    // the snapshot/archive prunes below purely to bound growth; correctness does not depend on the
    // order, because the sweep only drops rows whose file has already left the disk.
    let forgotten = crate::services::media_scope::sweep_orphans(
        pool,
        cfg,
        // Well clear of any in-flight export: producers attribute before the bytes land.
        chrono::Duration::hours(1),
    )
    .await;
    if forgotten > 0 {
        tracing::info!(
            forgotten,
            "retention: forgot media attribution rows for deleted artifacts"
        );
    }
    if clips_pruned > 0 {
        tracing::info!(
            deleted = clips_pruned,
            "retention: pruned old exported clips"
        );
    }
    //    (b) Mirror (dual-DVR) segments: unindexed second copies under mirror_recordings_dir/{camera}/.
    //        Prune each camera's subtree to that camera's own retention_hours (mirroring the primary);
    //        the segments/size-cap sweeps never see these files.
    if let Some(mirror_root) = &cfg.mirror_recordings_dir {
        let cams: Vec<(String, i64)> = sqlx::query_as("SELECT id, retention_hours FROM cameras")
            .fetch_all(pool)
            .await
            .unwrap_or_default();
        let mut mirror_pruned = 0u64;
        for (cam_id, hours) in cams {
            let max_age = std::time::Duration::from_secs(hours.max(1) as u64 * 3600);
            mirror_pruned += prune_tree_older_than(&mirror_root.join(&cam_id), max_age).await;
        }
        if mirror_pruned > 0 {
            tracing::info!(
                deleted = mirror_pruned,
                "retention: pruned old mirror segments"
            );
        }
    }

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
            // Whose footage goes. Oldest-first WITHIN the camera that is furthest over its share,
            // so one camera's retention setting or evidence locks cannot spend another's budget.
            // Re-evaluated every batch: as a camera drops back to its share the target moves on.
            let batch: Vec<(String, String, i64)> = match most_over_share(pool, max).await {
                ShareVerdict::OverButProtected(cam) => {
                    tracing::warn!(
                        camera_id = %cam,
                        "retention: over the size cap, but the camera over its share holds only \
                         evidence-locked footage; refusing to delete OTHER cameras' recordings to \
                         pay for it"
                    );
                    let _ = repo::log_event(
                        pool,
                        Some(&cam),
                        "disk_pressure",
                        "critical",
                        json!({ "reason": "over_share_all_protected", "max_bytes": max }),
                    )
                    .await;
                    break;
                }
                ShareVerdict::Evict(cam) => {
                    sqlx::query_as(
                        "SELECT id, path, size_bytes FROM segments
                          WHERE locked = 0 AND evidence_locked = 0 AND camera_id = ?
                          ORDER BY end_time ASC LIMIT 20",
                    )
                    .bind(&cam)
                    .fetch_all(pool)
                    .await?
                }
                // Nobody is over their share, so there is no unfairness to correct — the box is
                // simply full. Oldest-first across the fleet is the right answer here.
                ShareVerdict::Balanced => {
                    sqlx::query_as(
                        "SELECT id, path, size_bytes FROM segments
                          WHERE locked = 0 AND evidence_locked = 0
                          ORDER BY end_time ASC LIMIT 20",
                    )
                    .fetch_all(pool)
                    .await?
                }
            };
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
                // Same fair-share policy as the size cap: the floor is a shared resource too, and
                // pruning the globally oldest here would reopen the identical cross-camera hole.
                let batch: Vec<(String, String)> = match most_over_share(pool, max).await {
                    ShareVerdict::OverButProtected(cam) => {
                        tracing::error!(
                            camera_id = %cam,
                            free_bytes = before,
                            floor,
                            "retention: below the disk-free floor, but the camera over its share is \
                             entirely evidence-locked; refusing to delete other cameras' footage. \
                             Operator action required."
                        );
                        let _ = repo::log_event(
                            pool,
                            Some(&cam),
                            "disk_pressure",
                            "critical",
                            json!({ "reason": "floor_blocked_by_protected_share", "min_free_bytes": floor }),
                        )
                        .await;
                        break;
                    }
                    ShareVerdict::Evict(cam) => {
                        sqlx::query_as(
                            "SELECT id, path FROM segments
                              WHERE locked = 0 AND evidence_locked = 0 AND camera_id = ?
                              ORDER BY end_time ASC LIMIT ?",
                        )
                        .bind(&cam)
                        .bind(want)
                        .fetch_all(pool)
                        .await?
                    }
                    ShareVerdict::Balanced => {
                        sqlx::query_as(
                            "SELECT id, path FROM segments
                              WHERE locked = 0 AND evidence_locked = 0
                              ORDER BY end_time ASC LIMIT ?",
                        )
                        .bind(want)
                        .fetch_all(pool)
                        .await?
                    }
                };
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
    let pruned = delete_aged_in_batches(pool, "detections", "created_at < ?", det_cutoff).await?;
    if pruned > 0 {
        tracing::info!(deleted = pruned, "retention: pruned old detections");
    }
    // Prune the transactional outbox on the same TTL (until an edge→cloud relay acks + prunes by seq).
    let ob_pruned = delete_aged_in_batches(pool, "outbox", "created_at < ?", det_cutoff).await?;
    if ob_pruned > 0 {
        tracing::info!(deleted = ob_pruned, "retention: pruned old outbox rows");
    }
    // Prune the per-consumer fan-out ledger on the SAME TTL. `consumer_fanout` is a dedup/at-most-once
    // claim (consumer, camera_id, frame_id) written on every ingest and — before this — never deleted,
    // so it grew without bound at fps×cameras×consumers, defeated the heldar.db size cap (which only
    // sheds `detections`), and slowed every ingest INSERT. A claim only needs to outlive the un-fanned
    // outbox window it guards against; the outbox itself is pruned above at `det_cutoff`, so any claim
    // older than that can never be re-driven and is safe to drop.
    let cf_pruned =
        delete_aged_in_batches(pool, "consumer_fanout", "fanned_at < ?", det_cutoff).await?;
    if cf_pruned > 0 {
        tracing::info!(
            deleted = cf_pruned,
            "retention: pruned old consumer-fanout rows"
        );
    }
    // Prune crop embeddings on the SAME TTL as the detections they derive from (issue #38), and
    // delete their crop-thumb evidence files. Batched like delete_aged_in_batches, but each batch
    // unlinks its evidence before deleting the rows (the zone_events pattern below, bounded).
    let mut emb_pruned = 0u64;
    loop {
        let batch: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT id, evidence_path FROM embeddings WHERE created_at < ? LIMIT 5000",
        )
        .bind(det_cutoff)
        .fetch_all(pool)
        .await?;
        if batch.is_empty() {
            break;
        }
        for (_id, evidence) in &batch {
            if let Some(name) = evidence.as_deref().and_then(|u| u.rsplit('/').next()) {
                let _ = tokio::fs::remove_file(cfg.snapshots_dir.join(name)).await;
            }
        }
        let last = batch.len() < 5000;
        for chunk in batch.chunks(500) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!("DELETE FROM embeddings WHERE id IN ({placeholders})");
            let mut q = sqlx::query(&sql);
            for (id, _) in chunk {
                q = q.bind(id);
            }
            emb_pruned += q.execute(pool).await?.rows_affected();
        }
        if last {
            break;
        }
        tokio::task::yield_now().await;
    }
    if emb_pruned > 0 {
        tracing::info!(
            deleted = emb_pruned,
            "retention: pruned old embeddings + crop thumbs"
        );
    }
    // Prune the transient query-embedding queue (payloads can be multi-MB images; rows are useless
    // minutes after the enqueuing search returned).
    let q_pruned = crate::services::embeddings::prune_queries(pool).await?;
    if q_pruned > 0 {
        tracing::info!(deleted = q_pruned, "retention: pruned old embed queries");
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
    let evpruned = delete_aged_in_batches(pool, "events", "created_at < ?", audit_cutoff).await?;
    if evpruned > 0 {
        tracing::info!(deleted = evpruned, "retention: pruned old event-log rows");
    }

    // 8b) Prune the webhook delivery ledger (one row per delivery attempt, per subscription, per event)
    //     past the audit horizon. The delivery cursor lives on the subscription, not these rows, so
    //     deleting old attempt records is safe — they are an at-rest audit trail, not delivery state.
    let wdpruned =
        delete_aged_in_batches(pool, "webhook_deliveries", "created_at < ?", audit_cutoff).await?;
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

    // 11) Metadata-DB size cap: after the row-retention deletes above have freed pages, checkpoint
    //     the WAL, reclaim, and if the DB file is still over its cap shed the oldest detections
    //     (events/audit are protected). Self-bounds heldar.db the way step 3 bounds recordings.
    match crate::services::db_maintenance::enforce_db_cap(pool, cfg).await {
        Ok(rep) if rep.total_deleted() > 0 => {
            tracing::info!(
                embed_queries_deleted = rep.embed_queries_deleted,
                embeddings_deleted = rep.embeddings_deleted,
                detections_deleted = rep.detections_deleted,
                bytes = rep.final_bytes,
                over_cap = rep.over_cap,
                "retention: db size-cap prune"
            );
            let _ = repo::log_event(
                pool,
                None,
                "db_retention",
                if rep.over_cap { "warning" } else { "info" },
                json!({
                    "embed_queries_deleted": rep.embed_queries_deleted,
                    "embeddings_deleted": rep.embeddings_deleted,
                    "detections_deleted": rep.detections_deleted,
                    "db_bytes": rep.final_bytes,
                    "over_cap": rep.over_cap
                }),
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

    /// The embeddings prune (issue #38) shares the detections TTL, unlinks crop-thumb evidence,
    /// and the embed-queries queue is swept on its own 1 h TTL.
    #[tokio::test]
    async fn sweep_prunes_old_embeddings_with_thumbs_and_stale_queries() {
        let pool = mem_pool().await;
        let mut cfg = test_cfg(); // detection_retention_hours = 168
        let snaps = unique_path("heldar-emb-ret");
        tokio::fs::create_dir_all(&snaps).await.unwrap();
        cfg.snapshots_dir = snaps.clone();

        insert_camera(&pool, "cam_e", 24, None).await;
        let now = Utc::now();
        let thumb = snaps.join("emb_old.jpg");
        tokio::fs::write(&thumb, b"jpeg").await.unwrap();
        for (id, age_h, evidence) in [
            ("emb_old", 200_i64, Some("/media/snapshots/emb_old.jpg")),
            ("emb_new", 1, None),
        ] {
            sqlx::query(
                "INSERT INTO embeddings (id, camera_id, ts, model, dim, vec, evidence_path, created_at)
                 VALUES (?, 'cam_e', ?, 'm', 2, ?, ?, ?)",
            )
            .bind(id)
            .bind(now - chrono::Duration::hours(age_h))
            .bind(crate::services::embeddings::encode_vec(&[1.0, 0.0]))
            .bind(evidence)
            .bind(now - chrono::Duration::hours(age_h))
            .execute(&pool)
            .await
            .unwrap();
        }
        // One stale queue row (over the 1 h queue TTL) and one fresh one.
        for (id, age_min) in [("embq_old", 90_i64), ("embq_new", 1)] {
            sqlx::query(
                "INSERT INTO embed_queries (id, kind, payload, status, created_at) VALUES (?, 'text', 'q', 'done', ?)",
            )
            .bind(id)
            .bind(now - chrono::Duration::minutes(age_min))
            .execute(&pool)
            .await
            .unwrap();
        }

        sweep(&pool, &cfg).await.unwrap();

        let kept: Vec<String> = sqlx::query_scalar("SELECT id FROM embeddings")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(kept, vec!["emb_new".to_string()]);
        assert!(
            !thumb.exists(),
            "the pruned embedding's crop thumb must be unlinked"
        );
        let queries: Vec<String> = sqlx::query_scalar("SELECT id FROM embed_queries")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(queries, vec!["embq_new".to_string()]);
        let _ = tokio::fs::remove_dir_all(&snaps).await;
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

#[cfg(test)]
mod fair_share_tests {
    use super::*;
    use chrono::Utc;

    async fn pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        pool
    }

    async fn cam(pool: &SqlitePool, id: &str, retention_hours: i64) {
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO cameras (id, name, retention_hours, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(id)
        .bind(retention_hours)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn seg(pool: &SqlitePool, id: &str, camera: &str, age_h: i64, size: i64, ev_lock: i64) {
        let end = Utc::now() - chrono::Duration::hours(age_h);
        sqlx::query(
            "INSERT INTO segments
                (id, camera_id, path, start_time, end_time, duration_s, size_bytes, locked,
                 evidence_locked, created_at)
             VALUES (?, ?, ?, ?, ?, 60.0, ?, 0, ?, ?)",
        )
        .bind(id)
        .bind(camera)
        .bind(format!("/nonexistent/{id}.mp4"))
        .bind(end - chrono::Duration::minutes(1))
        .bind(end)
        .bind(size)
        .bind(ev_lock)
        .bind(Utc::now())
        .execute(pool)
        .await
        .unwrap();
    }

    /// THE CRITICAL ONE. A camera-scoped credential PATCHes `retention_hours` on its OWN camera —
    /// fully authorised, 200 OK — so its footage stops aging out. The box is still over its cap, and
    /// eviction used to take the globally OLDEST segments, which by then belong to a camera the
    /// caller has no access to. Executed against a seeded box, that deleted 100% of the other
    /// camera's recordings, files unlinked, seconds after an authorised request.
    ///
    /// The guard is not the defect and never was: the sweeper holds no principal. The invariant this
    /// pins is that a camera can only spend its OWN share of the disk.
    #[tokio::test]
    async fn a_cameras_own_retention_setting_cannot_evict_another_cameras_footage() {
        let p = pool().await;
        // camera_a opted out of age-pruning (the authorised write); camera_b is behaving.
        cam(&p, "camera_a", 87_600).await;
        cam(&p, "camera_b", 87_600).await;
        // camera_a hogs: 6 old segments. camera_b: 3 NEWER ones, well inside any fair share.
        for i in 1..=6 {
            seg(&p, &format!("a{i}"), "camera_a", 100 - i, 1000, 0).await;
        }
        for i in 1..=3 {
            seg(&p, &format!("b{i}"), "camera_b", 10 - i, 1000, 0).await;
        }
        // camera_b's segments are the NEWEST, so oldest-first would eat camera_a first here; make
        // camera_b's the OLDEST to reproduce the real attack, where the hog's footage is protected by
        // its own retention setting and the victim's is simply older.
        for i in 1..=3 {
            sqlx::query("UPDATE segments SET end_time = ? WHERE id = ?")
                .bind(Utc::now() - chrono::Duration::hours(200 + i))
                .bind(format!("b{i}"))
                .execute(&p)
                .await
                .unwrap();
        }

        let mut cfg = Config::from_env();
        cfg.max_recordings_bytes = 5000;
        cfg.min_free_disk_bytes = 0;
        cfg.recordings_dir = std::env::temp_dir();
        cfg.snapshot_retention_hours = 0;
        cfg.archive_retention_hours = 0;
        sweep(&p, &cfg).await.unwrap();

        let b_left: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM segments WHERE camera_id = 'camera_b'")
                .fetch_one(&p)
                .await
                .unwrap();
        let a_left: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM segments WHERE camera_id = 'camera_a'")
                .fetch_one(&p)
                .await
                .unwrap();
        assert_eq!(
            b_left, 3,
            "camera_b lost footage to pay for camera_a's retention setting (a_left={a_left})"
        );
        assert!(
            a_left < 6,
            "the cap was never enforced at all — the test proves nothing (a_left={a_left})"
        );
    }

    /// The same shape through evidence locks: `protected_bytes` was summed FLEET-WIDE and subtracted
    /// from the shared budget, so a camera locking its OWN footage starved everyone else's.
    #[tokio::test]
    async fn evidence_locks_are_charged_to_the_locking_camera_not_the_fleet() {
        let p = pool().await;
        cam(&p, "camera_a", 87_600).await;
        cam(&p, "camera_b", 87_600).await;
        // camera_a locks nearly the whole cap; camera_b holds a little, and it is OLDER.
        for i in 1..=4 {
            seg(&p, &format!("a{i}"), "camera_a", 50 - i, 1000, 1).await;
        }
        seg(&p, "a_del", "camera_a", 60, 1000, 0).await;
        for i in 1..=3 {
            seg(&p, &format!("b{i}"), "camera_b", 300 + i, 500, 0).await;
        }

        let mut cfg = Config::from_env();
        cfg.max_recordings_bytes = 5000;
        cfg.min_free_disk_bytes = 0;
        cfg.recordings_dir = std::env::temp_dir();
        cfg.snapshot_retention_hours = 0;
        cfg.archive_retention_hours = 0;
        sweep(&p, &cfg).await.unwrap();

        let b_left: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM segments WHERE camera_id = 'camera_b'")
                .fetch_one(&p)
                .await
                .unwrap();
        assert_eq!(
            b_left, 3,
            "camera_a's evidence locks consumed camera_b's budget and deleted its footage"
        );
    }

    /// The policy must not become "nobody ever gets pruned". When every camera is within its share
    /// the box is simply full, and oldest-first across the fleet is the fair answer.
    #[tokio::test]
    async fn a_balanced_box_still_prunes_oldest_first() {
        let p = pool().await;
        cam(&p, "camera_a", 87_600).await;
        cam(&p, "camera_b", 87_600).await;
        for i in 1..=4 {
            seg(&p, &format!("a{i}"), "camera_a", 100 - i, 1000, 0).await;
            seg(&p, &format!("b{i}"), "camera_b", 100 - i, 1000, 0).await;
        }
        let mut cfg = Config::from_env();
        cfg.max_recordings_bytes = 4000;
        cfg.min_free_disk_bytes = 0;
        cfg.recordings_dir = std::env::temp_dir();
        cfg.snapshot_retention_hours = 0;
        cfg.archive_retention_hours = 0;
        sweep(&p, &cfg).await.unwrap();
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM segments")
            .fetch_one(&p)
            .await
            .unwrap();
        assert!(
            total < 8,
            "the cap must still be enforced on a balanced box; nothing was pruned"
        );
    }
}
