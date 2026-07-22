//! The single detection-ingest path, shared by the worker-facing HTTP route
//! (`routes::ai::ingest`) and kernel-internal producers (the camera-native ANPR poller,
//! `services::native_anpr`).
//!
//! Semantics are the batch contract the AI worker has always had: the batch is recorded in the
//! outbox FIRST (idempotent on `(camera_id, frame_id)` — a redelivery is a no-op), detections are
//! written all-or-nothing in one transaction, and the committed batch is fanned out to the
//! registered [`DetectionConsumer`]s (durably: a crash between commit and fan-out is replayed by
//! the fanout drainer).

use chrono::Utc;
use serde_json::json;
use sqlx::types::Json as SqlxJson;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::models::{AiIngest, Camera};
use crate::state::AppState;

/// Max detections accepted in a single ingest batch (DoS / write-amplification bound).
pub const MAX_INGEST_DETECTIONS: usize = 1000;

/// Columns bound per detection row in the batched INSERT.
const DETECTION_INSERT_COLS: usize = 11;
/// SQLite's compile-time bound-variable ceiling (SQLITE_MAX_VARIABLE_NUMBER). The batched insert is
/// chunked so a single statement never exceeds it, even at [`MAX_INGEST_DETECTIONS`].
const SQLITE_MAX_BIND_VARS: usize = 999;
/// Detection rows per INSERT statement (≈90), keeping bound variables under the ceiling.
const DETECTION_INSERT_CHUNK: usize = SQLITE_MAX_BIND_VARS / DETECTION_INSERT_COLS;

/// Result of one ingest call.
pub struct IngestOutcome {
    pub inserted: u64,
    /// True when the batch's `(camera_id, frame_id)` was already ingested (no-op redelivery).
    pub duplicate: bool,
}

/// Persist + fan out one detection batch. See the module doc for the exact semantics.
pub async fn ingest_batch(st: &AppState, body: &AiIngest) -> AppResult<IngestOutcome> {
    let cam = sqlx::query_as::<_, Camera>("SELECT * FROM cameras WHERE id = ?")
        .bind(&body.camera_id)
        .fetch_optional(&st.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("camera {} not found", body.camera_id)))?;
    if body.task_type.trim().is_empty() {
        return Err(AppError::BadRequest("`task_type` is required".into()));
    }
    if body.detections.len() > MAX_INGEST_DETECTIONS {
        return Err(AppError::BadRequest(format!(
            "too many detections in one request ({}); max {MAX_INGEST_DETECTIONS}",
            body.detections.len()
        )));
    }
    let ts = match &body.timestamp {
        Some(v) => crate::util::parse_rfc3339(v)
            .ok_or_else(|| AppError::BadRequest("invalid `timestamp`".into()))?,
        None => Utc::now(),
    };

    let mut inserted = 0u64;
    let mut tx = st.pool.begin().await?;
    // Idempotency + atomic capture: record the batch in the outbox FIRST, in the same transaction.
    // A duplicate (camera_id, frame_id) — i.e. an at-least-once redelivery — conflicts and inserts 0
    // rows; we then skip both the detection writes and the consumer fan-out, so a replayed batch can
    // never double-count ANPR votes or corrupt zone state. With no frame_id every batch is accepted.
    let outbox_res = sqlx::query(
        "INSERT INTO outbox (topic, camera_id, site_id, frame_id, task_type, detection_count, created_at)
         VALUES ('detections', ?, ?, ?, ?, ?, ?)
         ON CONFLICT DO NOTHING",
    )
    .bind(&body.camera_id)
    .bind(&cam.site_id)
    .bind(&body.frame_id)
    .bind(&body.task_type)
    .bind(body.detections.len() as i64)
    .bind(Utc::now())
    .execute(&mut *tx)
    .await?;
    if outbox_res.rows_affected() == 0 {
        // Duplicate frame already ingested — no-op (idempotent).
        tx.commit().await?;
        return Ok(IngestOutcome {
            inserted: 0,
            duplicate: true,
        });
    }
    // Batched multi-row insert: one INSERT per chunk instead of one statement per detection,
    // chunked so a single statement's bound-variable count stays under SQLite's limit.
    for chunk in body.detections.chunks(DETECTION_INSERT_CHUNK) {
        let tuples = vec!["(?,?,?,?,?,?,?,?,?,?,?)"; chunk.len()].join(",");
        let sql = format!(
            "INSERT INTO detections
               (id, camera_id, task_type, timestamp, label, confidence, bbox, track_id, attributes, frame_id, created_at)
             VALUES {tuples}"
        );
        let mut q = sqlx::query(&sql);
        for d in chunk {
            let bbox = d.bbox.clone().map(SqlxJson);
            let attrs = SqlxJson(d.attributes.clone().unwrap_or_else(|| json!({})));
            q = q
                .bind(format!("det_{}", Uuid::new_v4().simple()))
                .bind(&body.camera_id)
                .bind(&body.task_type)
                .bind(ts)
                .bind(&d.label)
                .bind(d.confidence)
                .bind(bbox)
                .bind(&d.track_id)
                .bind(attrs)
                .bind(&body.frame_id)
                .bind(Utc::now());
        }
        inserted += q.execute(&mut *tx).await?.rows_affected();
    }
    tx.commit().await?;

    // Fan the committed batch out to registered perception consumers (zones, ANPR/entry, future
    // apps). The kernel does not know or branch on which apps exist — each consumer self-selects by
    // task_type. Engines that need trustworthy timing use server time, not the producer timestamp.
    //
    // Durability: fan-out happens after commit, so a crash here would otherwise drop the consumer
    // notification. `fan_out` claims each (consumer, frame) at-most-once; on success we mark the
    // outbox batch fanned, and the `fanout` drainer replays any batch left un-fanned by a crash.
    let batch = crate::services::consumer::DetectionBatch {
        camera_id: &body.camera_id,
        site_id: cam.site_id.as_deref(),
        task_type: &body.task_type,
        detections: &body.detections,
        timestamp: ts,
    };
    let fanned = crate::services::consumer::fan_out(
        &st.pool,
        &st.consumers,
        &batch,
        body.frame_id.as_deref(),
    )
    .await;
    if fanned {
        if let Some(fid) = body.frame_id.as_deref() {
            let _ = sqlx::query(
                "UPDATE outbox SET fanned_out_at = ? \
                 WHERE topic = 'detections' AND camera_id = ? AND frame_id = ? AND fanned_out_at IS NULL",
            )
            .bind(Utc::now())
            .bind(&body.camera_id)
            .bind(fid)
            .execute(&st.pool)
            .await;
        }
    }

    if let Some(ev) = &body.event {
        let severity = ev.severity.clone().unwrap_or_else(|| "info".into());
        let payload = ev.payload.clone().unwrap_or_else(|| json!({}));
        crate::repo::log_event(
            &st.pool,
            Some(&body.camera_id),
            &ev.event_type,
            &severity,
            payload,
        )
        .await?;
    }

    Ok(IngestOutcome {
        inserted,
        duplicate: false,
    })
}
