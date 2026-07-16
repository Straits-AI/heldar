//! Camera-native ANPR ingestion (issue #43): poll each enabled camera's ON-BOARD plate-recognition
//! engine and feed the reads into the same detection pipeline the AI worker uses.
//!
//! Dedicated ANPR barrier cameras (e.g. HikVision iDS-series) recognize plates on-device with
//! specialized optics/illumination — usually better at a gate lane than server-side OCR, and free.
//! This poller turns those on-board reads into ordinary `task_type = "anpr"` detection batches via
//! [`crate::services::perception_ingest::ingest_batch`], so the access-control engine (voting,
//! identity resolution, guard workflow) consumes them unchanged — only the producer differs.
//!
//! Mechanics per camera with `native_anpr_enabled` (and a supported vendor):
//! - Poll the vendor plate-results endpoint with a durable cursor (`camera_native_anpr_state`,
//!   the device's verbatim `captureTime` format) so a restart resumes where it left off.
//! - Each read becomes one detection: `label = "vehicle"`, `attributes.plate` + a
//!   `source = "camera_native"` marker (the entry engine weights native reads as authoritative —
//!   the device already did its own multi-frame consolidation, so one read = one vote threshold).
//! - `frame_id` is derived from the device picture name (or captureTime+plate), so the ingest
//!   outbox dedupes any replay after a crash between ingest and cursor advance.
//!
//! Poll failures are recorded on the state row (`last_error`) and surfaced as camera health — they
//! never stop the loop or affect other cameras.

use std::time::Duration;

use chrono::Utc;
use serde_json::json;

use crate::models::{AiIngest, Camera, DetectionIngest};
use crate::services::camera_config::types::NativePlateRead;
use crate::services::camera_config::{self};
use crate::state::AppState;

/// Marker value carried in `attributes.source` for on-board reads (the entry engine keys on it).
pub const SOURCE_CAMERA_NATIVE: &str = "camera_native";

/// Run the poller loop forever (spawned supervised by the composing server). Self-idles when no
/// camera has native ANPR enabled.
pub async fn run(st: AppState) {
    let interval = Duration::from_millis(st.cfg.native_anpr_poll_ms.max(250));
    loop {
        let cameras: Vec<Camera> = sqlx::query_as::<_, Camera>(
            "SELECT * FROM cameras WHERE enabled = 1 AND native_anpr_enabled = 1 ORDER BY id ASC",
        )
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default();

        for cam in &cameras {
            if let Err(e) = poll_camera(&st, cam).await {
                let msg = e.to_string();
                tracing::warn!(camera_id = %cam.id, error = %msg, "native_anpr: poll failed");
                let _ = sqlx::query(
                    "INSERT INTO camera_native_anpr_state (camera_id, last_error, updated_at)
                     VALUES (?, ?, ?)
                     ON CONFLICT(camera_id) DO UPDATE SET last_error = excluded.last_error,
                        updated_at = excluded.updated_at",
                )
                .bind(&cam.id)
                .bind(&msg)
                .bind(Utc::now())
                .execute(&st.pool)
                .await;
            }
        }
        tokio::time::sleep(interval).await;
    }
}

/// Poll one camera's on-board engine and ingest anything newer than the cursor.
async fn poll_camera(st: &AppState, cam: &Camera) -> crate::error::AppResult<()> {
    let provider = camera_config::for_camera(cam, &st.http, st.cfg.isapi_request_timeout_ms)?;

    let cursor: String =
        sqlx::query_scalar("SELECT cursor_time FROM camera_native_anpr_state WHERE camera_id = ?")
            .bind(&cam.id)
            .fetch_optional(&st.pool)
            .await?
            .unwrap_or_default();

    // First poll ever: don't replay the device's whole buffer — start from "now" by asking with an
    // empty window and only advancing the cursor (the backlog predates Heldar's involvement).
    let first_poll = cursor.is_empty();
    let reads = provider.fetch_anpr_plates(&cursor).await?;
    if reads.is_empty() {
        return Ok(());
    }

    // Device captureTime strings are digit-timestamps, so lexicographic max = newest.
    let newest = reads
        .iter()
        .map(|r| r.capture_time.as_str())
        .max()
        .unwrap_or("")
        .to_string();

    let mut ingested = 0usize;
    if !first_poll {
        for read in &reads {
            // Defensive: some firmwares return >= cursor; skip anything not strictly newer.
            if !cursor.is_empty() && read.capture_time.as_str() <= cursor.as_str() {
                continue;
            }
            ingest_read(st, cam, read).await;
            ingested += 1;
        }
    }

    let now = Utc::now();
    sqlx::query(
        "INSERT INTO camera_native_anpr_state (camera_id, cursor_time, last_event_at, last_error, updated_at)
         VALUES (?, ?, ?, NULL, ?)
         ON CONFLICT(camera_id) DO UPDATE SET
            cursor_time = excluded.cursor_time,
            last_event_at = CASE WHEN ? > 0 THEN excluded.last_event_at ELSE camera_native_anpr_state.last_event_at END,
            last_error = NULL,
            updated_at = excluded.updated_at",
    )
    .bind(&cam.id)
    .bind(&newest)
    .bind(now)
    .bind(now)
    .bind(ingested as i64)
    .execute(&st.pool)
    .await?;
    Ok(())
}

/// Turn one on-board plate read into a single-detection `anpr` batch through the shared ingest
/// path. Ingest errors are logged, never propagated — one bad read must not wedge the cursor.
async fn ingest_read(st: &AppState, cam: &Camera, read: &NativePlateRead) {
    let direction = match read.direction.as_deref() {
        Some("forward") => "inbound",
        Some("reverse") => "outbound",
        _ => "unknown",
    };
    // Idempotency key: the device picture name is unique per read; fall back to time+plate.
    let frame_id = match &read.pic_name {
        Some(p) if !p.trim().is_empty() => format!("nanpr_{p}"),
        _ => format!("nanpr_{}_{}", read.capture_time, read.plate),
    };
    let batch = AiIngest {
        camera_id: cam.id.clone(),
        task_type: "anpr".into(),
        timestamp: None, // server time; the entry engine uses server time anyway
        frame_id: Some(frame_id),
        detections: vec![DetectionIngest {
            label: Some("vehicle".into()),
            confidence: None,
            bbox: None,
            // No worker track id: the entry engine falls back to keying on the plate itself,
            // which also consolidates a burst of duplicate device reads within its TTL window.
            track_id: None,
            attributes: Some(json!({
                "plate": read.plate,
                "source": SOURCE_CAMERA_NATIVE,
                "direction": direction,
                "country": read.country,
                "model_versions": { "anpr": "camera_native" },
            })),
        }],
        event: None,
    };
    match crate::services::perception_ingest::ingest_batch(st, &batch).await {
        Ok(outcome) if outcome.duplicate => {
            tracing::debug!(camera_id = %cam.id, plate = %read.plate, "native_anpr: duplicate read skipped");
        }
        Ok(_) => {
            tracing::info!(camera_id = %cam.id, plate = %read.plate, direction, "native_anpr: plate ingested");
        }
        Err(e) => {
            tracing::warn!(camera_id = %cam.id, plate = %read.plate, error = %e, "native_anpr: ingest failed");
        }
    }
}
