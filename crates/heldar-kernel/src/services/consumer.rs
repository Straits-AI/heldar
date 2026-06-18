//! The kernel's perception-consumer seam.
//!
//! The media + AI-task kernel is domain-agnostic: it samples frames, accepts detections from workers,
//! and persists them. Anything that *interprets* those detections into domain events — the zone engine
//! (spatial rules), the ANPR/access-control engine (plate authorization), and other domain
//! verticals — plug in here as a [`DetectionConsumer`] rather than being wired
//! into the ingest handler or [`crate::state::AppState`] directly.
//!
//! This inverts the dependency: ingest fans a committed batch out to a registry of consumers; a
//! consumer self-declares which `task_type`s it cares about. New apps are *added* to the registry (and,
//! after the crate split, linked in by the composing binary or fed over the event stream) — the kernel
//! never gains an `if task_type == "..."` branch.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::models::DetectionIngest;

/// One committed batch of detections handed to consumers after it is persisted. Carries the
/// site/camera/task context so a consumer needs no extra lookups (and so the seam is tenant-aware for
/// distributed deployments).
pub struct DetectionBatch<'a> {
    pub camera_id: &'a str,
    pub site_id: Option<&'a str>,
    /// The task type that produced this batch (consumers self-select on it; a vertical may also
    /// inspect it). Part of the stable seam contract.
    pub task_type: &'a str,
    pub detections: &'a [DetectionIngest],
    /// Worker-supplied capture time (engines that need trustworthy timing use server time instead;
    /// time-windowing consumers use this).
    pub timestamp: DateTime<Utc>,
}

/// A pluggable interpreter of detection batches. Implementors live in their own module/crate (zones is
/// kernel-open; ANPR/entry is a proprietary app) and are registered into [`crate::state::AppState`].
#[async_trait::async_trait]
pub trait DetectionConsumer: Send + Sync {
    /// Stable name for logs/metrics.
    fn name(&self) -> &'static str;

    /// Whether this consumer wants batches of the given `task_type`. Return `true` for all types when
    /// the consumer is task-agnostic (e.g. the zone engine evaluates any tracked detection).
    fn interested_in(&self, task_type: &str) -> bool;

    /// Process a persisted batch. Must not panic; errors are the consumer's own to log/handle.
    async fn consume(&self, batch: &DetectionBatch<'_>);
}

/// Drive every interested consumer for one committed batch, AT MOST ONCE per
/// `(consumer, camera_id, frame_id)`.
///
/// This is the single fan-out path used by both the ingest handler (inline, low-latency) and the
/// durable drainer ([`crate::services::fanout`], which replays batches whose fan-out didn't complete
/// because the process crashed between commit and fan-out). The per-consumer claim row in
/// `consumer_fanout` is what makes replay safe: a consumer that already saw this frame is skipped, so
/// a replayed batch never double-drives it (no double-counted ANPR votes / zone state).
///
/// A batch without a `frame_id` cannot be deduped, so its consumers are driven directly and it is not
/// eligible for durable replay (the worker opted out of the idempotency key).
///
/// Returns `true` when every interested consumer was driven or was already done — i.e. the batch is
/// fully fanned out and may be marked complete. Returns `false` if a dedup-claim write failed for some
/// consumer, so the caller leaves the batch un-fanned for the drainer to retry (the consumers that
/// already succeeded stay claimed and will not re-run).
pub async fn fan_out(
    pool: &SqlitePool,
    consumers: &[Arc<dyn DetectionConsumer>],
    batch: &DetectionBatch<'_>,
    frame_id: Option<&str>,
) -> bool {
    let mut complete = true;
    for consumer in consumers {
        if !consumer.interested_in(batch.task_type) {
            continue;
        }
        if let Some(fid) = frame_id {
            // Claim (consumer, camera, frame) before driving. ON CONFLICT DO NOTHING => rows_affected
            // is 1 only for the first claim; a replay sees 0 and skips. SQLite serializes the insert,
            // so two racing fan-outs of the same frame can't both claim it.
            match sqlx::query(
                "INSERT INTO consumer_fanout (consumer, camera_id, frame_id, fanned_at)
                 VALUES (?, ?, ?, ?) ON CONFLICT DO NOTHING",
            )
            .bind(consumer.name())
            .bind(batch.camera_id)
            .bind(fid)
            .bind(Utc::now())
            .execute(pool)
            .await
            {
                Ok(r) if r.rows_affected() == 1 => {} // freshly claimed -> drive below
                Ok(_) => continue, // already fanned to this consumer for this frame
                Err(e) => {
                    // Couldn't claim; skip to avoid double-processing and leave the batch un-fanned so
                    // the drainer retries (the consumer never ran, so no work is lost).
                    tracing::warn!(consumer = consumer.name(), error = %e, "fan-out dedup claim failed");
                    complete = false;
                    continue;
                }
            }
        }
        tracing::trace!(consumer = consumer.name(), task_type = %batch.task_type, "fan-out");
        consumer.consume(batch).await;
    }
    complete
}
