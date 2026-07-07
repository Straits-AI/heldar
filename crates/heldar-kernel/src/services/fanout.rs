//! Durable perception fan-out drainer.
//!
//! Detection ingest fans a committed batch out to consumers (zones / ANPR / movement) AFTER the
//! transaction commits — so a process crash between commit and fan-out would drop the consumer
//! notification entirely. This loop closes that gap: it periodically replays any `outbox` batch whose
//! `fanned_out_at` is still NULL (a batch that was committed but never fully fanned out), rebuilding
//! the batch from the persisted detections and driving the consumers.
//!
//! Replay is safe because [`consumer::fan_out`] claims each `(consumer, camera_id, frame_id)`
//! at-most-once via `consumer_fanout`: a consumer that already processed a frame is skipped, so a
//! replay never double-drives it. Only batches with a `frame_id` (the worker's idempotency key) are
//! replayable; batches without one are inline-only.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use sqlx::SqlitePool;

use crate::models::{Detection, DetectionIngest};
use crate::services::consumer::{fan_out, DetectionBatch, DetectionConsumer};

/// How long a batch must sit un-fanned before the drainer replays it. Keeps the drainer from racing
/// the inline ingest fan-out for batches that are simply mid-flight.
const REPLAY_GRACE_SECS: i64 = 10;
const DRAIN_INTERVAL_SECS: u64 = 15;
const DRAIN_BATCH: i64 = 200;

/// One un-fanned outbox batch to replay: `(seq, camera_id, site_id, frame_id, task_type)`.
type UnfannedBatch = (i64, String, Option<String>, String, Option<String>);

pub async fn run(pool: SqlitePool, consumers: Arc<Vec<Arc<dyn DetectionConsumer>>>) {
    let mut tick = tokio::time::interval(Duration::from_secs(DRAIN_INTERVAL_SECS));
    loop {
        tick.tick().await;
        if let Err(e) = drain(&pool, &consumers).await {
            tracing::warn!(error = %e, "fanout drainer: drain failed");
        }
    }
}

async fn drain(pool: &SqlitePool, consumers: &[Arc<dyn DetectionConsumer>]) -> anyhow::Result<()> {
    let cutoff = Utc::now() - chrono::Duration::seconds(REPLAY_GRACE_SECS);
    // Committed-but-not-fanned detection batches, replayable (have an idempotency key), past the grace.
    let rows: Vec<UnfannedBatch> = sqlx::query_as(
        "SELECT seq, camera_id, site_id, frame_id, task_type FROM outbox
         WHERE fanned_out_at IS NULL AND topic = 'detections'
           AND camera_id IS NOT NULL AND frame_id IS NOT NULL
           AND created_at < ?
         ORDER BY seq ASC
         LIMIT ?",
    )
    .bind(cutoff)
    .bind(DRAIN_BATCH)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(());
    }
    tracing::info!(
        count = rows.len(),
        "fanout drainer: replaying detection batches whose fan-out did not complete"
    );
    for (seq, camera_id, site_id, frame_id, task_type) in rows {
        let task_type = task_type.unwrap_or_default();
        let dets: Vec<Detection> = sqlx::query_as(
            "SELECT * FROM detections WHERE camera_id = ? AND frame_id = ? ORDER BY id ASC",
        )
        .bind(&camera_id)
        .bind(&frame_id)
        .fetch_all(pool)
        .await?;
        // Reconstruct the worker-shaped batch from persisted rows. Use the detections' own capture
        // time (all share the ingest `ts`); fall back to now for a detection-less (event-only) batch.
        let ts = dets.first().map(|d| d.timestamp).unwrap_or_else(Utc::now);
        let ingest: Vec<DetectionIngest> = dets
            .into_iter()
            .map(|d| DetectionIngest {
                label: d.label,
                confidence: d.confidence,
                bbox: d.bbox.map(|j| j.0),
                track_id: d.track_id,
                attributes: Some(d.attributes.0),
            })
            .collect();
        let batch = DetectionBatch {
            camera_id: &camera_id,
            site_id: site_id.as_deref(),
            task_type: &task_type,
            detections: &ingest,
            timestamp: ts,
        };
        let complete = fan_out(pool, consumers, &batch, Some(&frame_id)).await;
        if complete {
            let _ = sqlx::query("UPDATE outbox SET fanned_out_at = ? WHERE seq = ?")
                .bind(Utc::now())
                .bind(seq)
                .execute(pool)
                .await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counter {
        hits: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl DetectionConsumer for Counter {
        fn name(&self) -> &'static str {
            "test-counter"
        }
        fn interested_in(&self, _task_type: &str) -> bool {
            true
        }
        async fn consume(&self, _batch: &DetectionBatch<'_>) {
            self.hits.fetch_add(1, Ordering::SeqCst);
        }
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

    async fn seed_unfanned_batch(pool: &SqlitePool, camera: &str, frame: &str) {
        let old = Utc::now() - chrono::Duration::seconds(60); // past the replay grace
                                                              // detections.camera_id REFERENCES cameras(id) and sqlx enables foreign_keys by default.
        sqlx::query(
            "INSERT INTO cameras (id, name, retention_hours, storage_quota_bytes, created_at, updated_at)
             VALUES (?, ?, 168, NULL, ?, ?)",
        )
        .bind(camera).bind(camera).bind(old).bind(old).execute(pool).await.unwrap();
        sqlx::query(
            "INSERT INTO outbox (topic, camera_id, site_id, frame_id, task_type, detection_count, created_at)
             VALUES ('detections', ?, NULL, ?, 'object_detection', 1, ?)",
        )
        .bind(camera).bind(frame).bind(old).execute(pool).await.unwrap();
        sqlx::query(
            "INSERT INTO detections (id, camera_id, task_type, timestamp, label, confidence, bbox, track_id, attributes, frame_id, created_at)
             VALUES (?, ?, 'object_detection', ?, 'car', 0.9, NULL, NULL, '{}', ?, ?)",
        )
        .bind(format!("det_{frame}")).bind(camera).bind(old).bind(frame).bind(old)
        .execute(pool).await.unwrap();
    }

    #[tokio::test]
    async fn drain_replays_unfanned_batch_exactly_once() {
        let pool = mem_pool().await;
        seed_unfanned_batch(&pool, "cam1", "frameA").await;
        let hits = Arc::new(AtomicUsize::new(0));
        let consumers: Vec<Arc<dyn DetectionConsumer>> =
            vec![Arc::new(Counter { hits: hits.clone() })];

        // First drain: the un-fanned batch is replayed once.
        drain(&pool, &consumers).await.unwrap();
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "batch should be fanned once"
        );

        // The batch is now marked fanned, so a second drain finds nothing.
        drain(&pool, &consumers).await.unwrap();
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "no replay once marked fanned"
        );

        // Even if the batch is forced back to un-fanned (e.g. the mark write was lost on a crash), the
        // per-consumer dedup in consumer_fanout must still prevent a second consume of the same frame.
        sqlx::query("UPDATE outbox SET fanned_out_at = NULL")
            .execute(&pool)
            .await
            .unwrap();
        drain(&pool, &consumers).await.unwrap();
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "consumer_fanout dedup must prevent re-driving the same (consumer, frame)"
        );
    }
}
