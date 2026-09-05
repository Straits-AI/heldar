//! Small shared data-access helpers used by background services and routes.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::types::Json;
use sqlx::SqlitePool;
use uuid::Uuid;

/// Upsert the camera status row, setting `state` and `last_error` (does not touch counters).
pub async fn set_state(
    pool: &SqlitePool,
    camera_id: &str,
    state: &str,
    last_error: Option<&str>,
) -> sqlx::Result<()> {
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO camera_status (camera_id, state, last_error, updated_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(camera_id) DO UPDATE SET
            state = excluded.state,
            last_error = excluded.last_error,
            updated_at = excluded.updated_at",
    )
    .bind(camera_id)
    .bind(state)
    .bind(last_error)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark the recorder process started: set state, pid, and last_started_at.
pub async fn set_running(
    pool: &SqlitePool,
    camera_id: &str,
    state: &str,
    pid: Option<i64>,
) -> sqlx::Result<()> {
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO camera_status (camera_id, state, recorder_pid, last_started_at, last_error, updated_at)
         VALUES (?, ?, ?, ?, NULL, ?)
         ON CONFLICT(camera_id) DO UPDATE SET
            state = excluded.state,
            recorder_pid = excluded.recorder_pid,
            last_started_at = excluded.last_started_at,
            last_error = NULL,
            updated_at = excluded.updated_at",
    )
    .bind(camera_id)
    .bind(state)
    .bind(pid)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Increment the reconnect counter, clear pid, and record the last error.
pub async fn bump_reconnect(
    pool: &SqlitePool,
    camera_id: &str,
    last_error: &str,
) -> sqlx::Result<()> {
    let now = Utc::now();
    let err = last_error.chars().rev().take(800).collect::<String>();
    let err: String = err.chars().rev().collect();
    sqlx::query(
        "INSERT INTO camera_status (camera_id, state, reconnect_count, last_error, recorder_pid, updated_at)
         VALUES (?, 'offline', 1, ?, NULL, ?)
         ON CONFLICT(camera_id) DO UPDATE SET
            state = 'offline',
            reconnect_count = camera_status.reconnect_count + 1,
            last_error = excluded.last_error,
            recorder_pid = NULL,
            updated_at = excluded.updated_at",
    )
    .bind(camera_id)
    .bind(err)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record that a new segment was indexed: bump count, set last_segment_at and observed bitrate.
pub async fn record_segment_indexed(
    pool: &SqlitePool,
    camera_id: &str,
    last_segment_at: DateTime<Utc>,
    bitrate_kbps: Option<f64>,
    fps_observed: Option<f64>,
) -> sqlx::Result<()> {
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO camera_status
           (camera_id, state, last_segment_at, segments_written, bitrate_kbps, fps_observed, updated_at)
         VALUES (?, 'recording', ?, 1, ?, ?, ?)
         ON CONFLICT(camera_id) DO UPDATE SET
            state = 'recording',
            last_segment_at = excluded.last_segment_at,
            segments_written = camera_status.segments_written + 1,
            bitrate_kbps = excluded.bitrate_kbps,
            fps_observed = excluded.fps_observed,
            last_error = NULL,
            updated_at = excluded.updated_at",
    )
    .bind(camera_id)
    .bind(last_segment_at)
    .bind(bitrate_kbps)
    .bind(fps_observed)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record a detected recording gap (a hole > 3s between consecutive segments) for ANR re-fill.
/// Ignore-on-conflict by `(camera_id, gap_start)` so re-scans never duplicate a gap. Best-effort:
/// a failure is the caller's to log, not fatal to indexing.
pub async fn upsert_recording_gap(
    pool: &SqlitePool,
    camera_id: &str,
    gap_start: DateTime<Utc>,
    gap_end: DateTime<Utc>,
    gap_seconds: i64,
) -> sqlx::Result<()> {
    let id = format!("gap_{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO recording_gaps
           (id, camera_id, gap_start, gap_end, gap_seconds, fill_state, fill_attempts, created_at)
         VALUES (?, ?, ?, ?, ?, 'pending', 0, ?)
         ON CONFLICT(camera_id, gap_start) DO NOTHING",
    )
    .bind(id)
    .bind(camera_id)
    .bind(gap_start)
    .bind(gap_end)
    .bind(gap_seconds)
    .bind(Utc::now())
    .execute(pool)
    .await?;
    Ok(())
}

/// Insert an event into the event log.
pub async fn log_event(
    pool: &SqlitePool,
    camera_id: Option<&str>,
    event_type: &str,
    severity: &str,
    payload: Value,
) -> sqlx::Result<()> {
    let now = Utc::now();
    // Captured here rather than threaded through 31 call sites, each of which would be a place to
    // pass the wrong thing. The task-local is set by the request middleware and does NOT cross
    // `tokio::spawn`, so a background emitter — a camera going offline, a disk warning, a retention
    // sweep — records NULL. That is the correct answer, not a gap: it says the box did this by
    // itself rather than naming whichever request happened to be in flight at the time.
    let request_id = crate::request_id::current();
    sqlx::query(
        "INSERT INTO events (id, camera_id, site_id, event_type, severity, timestamp, payload, created_at, request_id)
         VALUES (?, ?, NULL, ?, ?, ?, ?, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(camera_id)
    .bind(event_type)
    .bind(severity)
    .bind(now)
    .bind(Json(payload))
    .bind(now)
    .bind(request_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Adjust a transient read-lock on a set of segments so the retention sweeper (which only deletes
/// `locked = 0`) won't remove them while clip/snapshot/playback ffmpeg is reading them — closing the
/// TOCTOU between selecting segments and ffmpeg opening their files. `locked` is a **reference count**,
/// not a boolean: `acquire` increments it and `release` decrements it (floored at 0), so two
/// overlapping holders of the same segment don't release each other's lock — the segment stays pinned
/// until the LAST holder releases. (A plain `SET locked = 0/1` let the first finisher unpin footage the
/// other holder was still reading, so retention could delete it mid-export.) Best-effort: a failure is
/// logged, not fatal. Locks are reset to 0 at startup ([`crate::db::clear_segment_read_locks`]) so a
/// crash mid-read cannot pin segments forever.
pub async fn set_segments_locked(pool: &SqlitePool, ids: &[String], locked: bool) {
    if ids.is_empty() {
        return;
    }
    let placeholders = vec!["?"; ids.len()].join(",");
    // Increment on acquire; decrement (never below 0) on release. `MAX(locked - 1, 0)` keeps the count
    // non-negative if a release ever races the startup reset or an unbalanced double-release.
    let set_expr = if locked {
        "locked = locked + 1"
    } else {
        "locked = MAX(locked - 1, 0)"
    };
    let sql = format!("UPDATE segments SET {set_expr} WHERE id IN ({placeholders})");
    let mut q = sqlx::query(&sql);
    for id in ids {
        q = q.bind(id);
    }
    if let Err(e) = q.execute(pool).await {
        tracing::warn!(error = %e, locked, "failed to adjust segment read-lock");
    }
}

/// RAII read-lock over a set of segments. `acquire` sets `locked = 1`; `Drop` releases it
/// best-effort even if the holder is cancelled (e.g. the HTTP client disconnects and the handler
/// future is dropped) or panics — so an interrupted export never leaves footage permanently
/// un-prunable. `Drop` can't await, so it spawns the release; the startup `clear_segment_read_locks`
/// is the backstop if that spawn can't finish.
pub struct SegReadLock {
    pool: SqlitePool,
    ids: Vec<String>,
}

impl SegReadLock {
    pub async fn acquire(pool: &SqlitePool, ids: Vec<String>) -> Self {
        set_segments_locked(pool, &ids, true).await;
        Self {
            pool: pool.clone(),
            ids,
        }
    }
}

impl Drop for SegReadLock {
    fn drop(&mut self) {
        if self.ids.is_empty() {
            return;
        }
        let pool = self.pool.clone();
        let ids = std::mem::take(&mut self.ids);
        tokio::spawn(async move {
            set_segments_locked(&pool, &ids, false).await;
        });
    }
}

/// Set or clear the DURABLE evidence lock on a single segment (distinct from the transient `locked`
/// read-lock). When `incident_id` is supplied it is recorded; `COALESCE` preserves any existing tag
/// when `incident_id` is `None` (so unlocking — or locking without a tag — never erases the case
/// the segment was already attached to). Returns the number of rows affected (0 ⇒ no such segment).
pub async fn set_evidence_locked(
    pool: &SqlitePool,
    segment_id: &str,
    locked: bool,
    incident_id: Option<&str>,
) -> sqlx::Result<u64> {
    let res = sqlx::query(
        "UPDATE segments SET evidence_locked = ?, incident_id = COALESCE(?, incident_id) WHERE id = ?",
    )
    .bind(i64::from(locked))
    .bind(incident_id)
    .bind(segment_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// An event has to say whether an operator caused it, or the box noticed it by itself.
#[cfg(test)]
mod event_correlation_tests {
    use super::*;

    async fn pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        pool
    }

    async fn last_request_id(pool: &SqlitePool) -> Option<String> {
        sqlx::query_scalar("SELECT request_id FROM events ORDER BY rowid DESC LIMIT 1")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// The point of the column: an operator holding a request id from a bug report can find what the
    /// box EMITTED, not only what it recorded doing.
    #[tokio::test]
    async fn an_event_emitted_inside_a_request_carries_its_id() {
        let pool = pool().await;
        crate::request_id::CURRENT
            .scope("req_abc123".to_string(), async {
                log_event(
                    &pool,
                    Some("cam_a"),
                    "camera_online",
                    "info",
                    serde_json::json!({}),
                )
                .await
                .unwrap();
            })
            .await;
        assert_eq!(last_request_id(&pool).await.as_deref(), Some("req_abc123"));
    }

    /// NULL is the ANSWER here, not a gap. A camera going offline, a disk warning and a retention
    /// sweep are things the box noticed on its own; naming whichever request happened to be in
    /// flight would be worse than saying nothing.
    #[tokio::test]
    async fn an_event_emitted_outside_a_request_records_null() {
        let pool = pool().await;
        log_event(
            &pool,
            Some("cam_a"),
            "camera_offline",
            "warning",
            serde_json::json!({}),
        )
        .await
        .unwrap();
        assert_eq!(last_request_id(&pool).await, None);
    }

    /// The task-local deliberately does not cross `tokio::spawn` (see request_id.rs). A detached
    /// background job must therefore record NULL even when a request is in flight — otherwise a
    /// sweep triggered while somebody happened to be browsing would be attributed to them.
    #[tokio::test]
    async fn a_spawned_task_does_not_inherit_the_callers_id() {
        let pool = pool().await;
        crate::request_id::CURRENT
            .scope("req_inflight".to_string(), async {
                let p = pool.clone();
                tokio::spawn(async move {
                    log_event(&p, None, "retention_delete", "info", serde_json::json!({}))
                        .await
                        .unwrap();
                })
                .await
                .unwrap();
            })
            .await;
        assert_eq!(
            last_request_id(&pool).await,
            None,
            "a detached task inherited a request id it was not part of"
        );
    }

    /// Webhook deliveries get this for free rather than keeping a third copy of the id: the ledger
    /// already points at the event, so "which request caused this webhook to fire" is a join.
    #[tokio::test]
    async fn a_webhook_delivery_reaches_the_request_through_its_event() {
        let pool = pool().await;
        crate::request_id::CURRENT
            .scope("req_join".to_string(), async {
                log_event(
                    &pool,
                    Some("cam_a"),
                    "object_detected",
                    "info",
                    serde_json::json!({}),
                )
                .await
                .unwrap();
            })
            .await;
        let event_id: String =
            sqlx::query_scalar("SELECT id FROM events ORDER BY rowid DESC LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();

        sqlx::query(
            "INSERT INTO webhook_subscriptions (id, name, url, event_types, enabled,
                                                created_at, updated_at)
             VALUES ('sub_a', 'test', 'https://example.invalid/h', '[]', 1, ?, ?)",
        )
        .bind(Utc::now())
        .bind(Utc::now())
        .execute(&pool)
        .await
        .unwrap();
        crate::services::webhooks::record_delivery(
            &pool,
            "del_a",
            "sub_a",
            Some(&event_id),
            Some("object_detected"),
            true,
            1,
            Some(200),
            None,
        )
        .await;

        let joined: Option<String> = sqlx::query_scalar(
            "SELECT e.request_id FROM webhook_deliveries d JOIN events e ON e.id = d.event_id
              WHERE d.id = 'del_a'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            joined.as_deref(),
            Some("req_join"),
            "a delivery could not be traced back to the request that caused its event"
        );
    }
}
