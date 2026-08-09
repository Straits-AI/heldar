//! The single detection-ingest path, shared by the worker-facing HTTP route
//! (`routes::ai::ingest`) and kernel-internal producers (the camera-native ANPR poller,
//! `services::native_anpr`).
//!
//! Semantics are the batch contract the AI worker has always had: the batch is recorded in the
//! outbox FIRST (idempotent on `(camera_id, frame_id)` — a redelivery is a no-op), detections are
//! written all-or-nothing in one transaction, and the committed batch is fanned out to the
//! registered [`DetectionConsumer`]s (durably: a crash between commit and fan-out is replayed by
//! the fanout drainer).
//!
//! # Provenance is a parameter, not a payload field
//!
//! [`Provenance`] is passed in by the CALLER and every detection's `attributes` blob is REWRITTEN from
//! it before the INSERT — any client-supplied `source` / `_prov` is stripped and the server's own value
//! written in its place. That is what makes `source = "camera_native"` inexpressible through the
//! external API: `routes::ai::ingest` can only construct [`Provenance::Worker`], and
//! `services::native_anpr` is the sole caller that can name a [`KernelProducer`].
//!
//! The rewrite happens HERE rather than in the HTTP handler on purpose. The rewritten attributes are
//! what get persisted, so `services::fanout`'s crash-replay — which rebuilds a batch from the
//! `detections` rows and bypasses the handler entirely — carries the same server-authored value as the
//! inline fan-out. Move the rewrite up into the route and the replay path silently loses it.
//!
//! The rewrite, and the reserved-event-prefix denylist below, are UNCONDITIONAL in every enforcement
//! tier including auth-off: no legitimate client has ever depended on asserting either.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use chrono::Utc;
use serde_json::{json, Value};
use sqlx::types::Json as SqlxJson;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::models::{AiIngest, Camera, DetectionIngest, Provenance};
use crate::state::AppState;

/// Max detections accepted in a single ingest batch (DoS / write-amplification bound).
pub const MAX_INGEST_DETECTIONS: usize = 1000;

/// Columns bound per detection row in the batched INSERT.
const DETECTION_INSERT_COLS: usize = 12;
/// SQLite's compile-time bound-variable ceiling (SQLITE_MAX_VARIABLE_NUMBER). The batched insert is
/// chunked so a single statement never exceeds it, even at [`MAX_INGEST_DETECTIONS`].
const SQLITE_MAX_BIND_VARS: usize = 999;
/// Detection rows per INSERT statement (≈83), keeping bound variables under the ceiling.
const DETECTION_INSERT_CHUNK: usize = SQLITE_MAX_BIND_VARS / DETECTION_INSERT_COLS;

/// Event-type prefixes a worker-provenance batch may NOT raise.
///
/// A denylist, not an allowlist: third-party sidecars invent their own event types and must keep
/// working. What they must not do is FORGE a kernel-domain event — `gate_opened` reaches webhooks
/// (`services::webhooks`) and operator email (`services::email`), and an operator reading an alert has
/// no way to tell a forged one from a real barrier actuation.
const RESERVED_EVENT_PREFIXES: &[&str] = &["gate_", "entry_", "zone_", "camera_", "disk_", "raid_"];

/// Severities a worker-provenance batch may raise. Anything higher is clamped, not rejected: a worker
/// mislabelling its own event should not lose the event, only the escalation.
const WORKER_MAX_SEVERITY: &[&str] = &["info", "warning"];

/// Result of one ingest call.
#[derive(Debug)]
pub struct IngestOutcome {
    pub inserted: u64,
    /// True when the batch's `(camera_id, frame_id)` was already ingested (no-op redelivery).
    pub duplicate: bool,
}

/// Validate a worker-supplied event type against the reserved-prefix denylist.
pub fn validate_worker_event_type(event_type: &str) -> AppResult<()> {
    let ok_shape = !event_type.is_empty()
        && event_type.len() <= 64
        && event_type
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if !ok_shape {
        return Err(AppError::BadRequest(format!(
            "`event.event_type` must match ^[a-z0-9_]{{1,64}}$ (got `{event_type}`)"
        )));
    }
    if let Some(p) = RESERVED_EVENT_PREFIXES
        .iter()
        .find(|p| event_type.starts_with(**p))
    {
        return Err(AppError::BadRequest(format!(
            "`event.event_type` prefix `{p}` is reserved for kernel-produced events and cannot be \
             raised over the ingest API"
        )));
    }
    Ok(())
}

/// Clamp a worker-supplied severity into what a machine credential may assert.
fn clamp_worker_severity(sev: &str) -> String {
    if WORKER_MAX_SEVERITY.contains(&sev) {
        sev.to_string()
    } else {
        "warning".to_string()
    }
}

/// Rewrite one detection's attributes so `source` / `_prov` are SERVER-authored.
///
/// Any client-supplied value for either key is discarded. A non-object attributes blob (legal JSON, but
/// unusable by every consumer in tree) is wrapped rather than dropped, so no client data is lost.
fn stamp_attributes(client: Option<&Value>, prov: &Provenance) -> Value {
    let mut obj = match client {
        Some(Value::Object(map)) => Value::Object(map.clone()),
        None | Some(Value::Null) => json!({}),
        Some(other) => json!({ "_client": other }),
    };
    let map = obj.as_object_mut().expect("constructed as an object above");
    map.remove("source");
    map.remove("_prov");
    map.insert("source".into(), Value::String(prov.source().to_string()));
    map.insert("_prov".into(), prov.detail());
    obj
}

/// Did the client try to assert its own provenance? Reported (rate-limited) so a forgery attempt is
/// visible rather than silently absorbed by the rewrite.
fn client_asserted_source(d: &DetectionIngest) -> Option<String> {
    d.attributes
        .as_ref()?
        .as_object()?
        .get("source")?
        .as_str()
        .map(|s| s.to_string())
}

/// Rate-limit the forged-provenance warning to once per producer per hour: it fires on the ingest path,
/// which a hostile client controls the rate of.
fn should_log_forgery(producer: &str) -> bool {
    static SEEN: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
    let now = Utc::now().timestamp();
    let Ok(mut map) = SEEN.get_or_init(Default::default).lock() else {
        return false;
    };
    match map.get(producer) {
        Some(last) if now - *last < 3600 => false,
        _ => {
            if map.len() > 1024 {
                map.retain(|_, last| now - *last < 3600);
            }
            map.insert(producer.to_string(), now);
            true
        }
    }
}

/// Persist + fan out one detection batch. See the module doc for the exact semantics.
///
/// `prov` decides the server-authored `attributes.source` every detection is stamped with, and is the
/// reason this signature takes a parameter the HTTP layer cannot forge.
pub async fn ingest_batch(
    st: &AppState,
    body: &AiIngest,
    prov: &Provenance,
) -> AppResult<IngestOutcome> {
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
    // Reject a forged kernel-domain event BEFORE anything is written, so a rejected batch leaves no
    // partial trace. Kernel producers are unrestricted (they ARE the domain).
    if prov.is_worker() {
        if let Some(ev) = &body.event {
            validate_worker_event_type(&ev.event_type)?;
        }
    }
    let ts = match &body.timestamp {
        Some(v) => crate::util::parse_rfc3339(v)
            .ok_or_else(|| AppError::BadRequest("invalid `timestamp`".into()))?,
        None => Utc::now(),
    };

    // Normalize ONCE, into an owned vec used for BOTH the INSERT and the fan-out batch, so the
    // consumers that run inline see exactly the bytes that were persisted (and therefore exactly what
    // the crash-replay drainer will hand them later).
    if prov.is_worker() {
        if let Some(claimed) = body.detections.iter().find_map(client_asserted_source) {
            let who = prov.produced_by();
            if should_log_forgery(&who) {
                tracing::warn!(
                    target: "heldar::security",
                    producer = %who,
                    camera_id = %body.camera_id,
                    claimed_source = %claimed,
                    "ingest: discarding client-asserted `attributes.source`; provenance is \
                     server-authored (a batch posted over the API is always `worker`)"
                );
            }
        }
    }
    let detections: Vec<DetectionIngest> = body
        .detections
        .iter()
        .map(|d| DetectionIngest {
            label: d.label.clone(),
            confidence: d.confidence,
            bbox: d.bbox.clone(),
            track_id: d.track_id.clone(),
            attributes: Some(stamp_attributes(d.attributes.as_ref(), prov)),
        })
        .collect();
    let prov_col = prov.column();
    let produced_by = prov.produced_by();

    let mut inserted = 0u64;
    let mut tx = st.pool.begin().await?;
    // Idempotency + atomic capture: record the batch in the outbox FIRST, in the same transaction.
    // A duplicate (camera_id, frame_id) — i.e. an at-least-once redelivery — conflicts and inserts 0
    // rows; we then skip both the detection writes and the consumer fan-out, so a replayed batch can
    // never double-count ANPR votes or corrupt zone state. With no frame_id every batch is accepted.
    let outbox_res = sqlx::query(
        "INSERT INTO outbox (topic, camera_id, site_id, frame_id, task_type, detection_count, created_at, provenance, produced_by)
         VALUES ('detections', ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT DO NOTHING",
    )
    .bind(&body.camera_id)
    .bind(&cam.site_id)
    .bind(&body.frame_id)
    .bind(&body.task_type)
    .bind(body.detections.len() as i64)
    .bind(Utc::now())
    .bind(&prov_col)
    .bind(&produced_by)
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
    for chunk in detections.chunks(DETECTION_INSERT_CHUNK) {
        let tuples = vec!["(?,?,?,?,?,?,?,?,?,?,?,?)"; chunk.len()].join(",");
        let sql = format!(
            "INSERT INTO detections
               (id, camera_id, task_type, timestamp, label, confidence, bbox, track_id, attributes, frame_id, created_at, provenance)
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
                .bind(Utc::now())
                .bind(&prov_col);
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
        detections: &detections,
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
        let raw_sev = ev.severity.clone().unwrap_or_else(|| "info".into());
        let severity = if prov.is_worker() {
            clamp_worker_severity(&raw_sev)
        } else {
            raw_sev
        };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{IngestEvent, KernelProducer};

    fn worker() -> Provenance {
        Provenance::Worker {
            api_key_id: "key_a".into(),
            task_id: Some("ai_1".into()),
            worker_id: Some("w1".into()),
        }
    }

    fn native() -> Provenance {
        Provenance::Kernel {
            producer: KernelProducer::NativeAnpr,
        }
    }

    async fn test_state() -> AppState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO cameras (id, name, enabled, created_at, updated_at) VALUES (?,?,1,?,?)",
        )
        .bind("cam1")
        .bind("cam1")
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let cfg = std::sync::Arc::new(crate::config::Config::from_env());
        AppState {
            recorder: crate::services::recorder::RecorderManager::new(pool.clone(), cfg.clone()),
            sampler: crate::services::sampler::SamplerManager::new(pool.clone(), cfg.clone()),
            live: crate::services::live_publisher::LivePublisherManager::new(
                pool.clone(),
                cfg.clone(),
                reqwest::Client::new(),
            ),
            mirror: None,
            consumers: std::sync::Arc::new(Vec::new()),
            modules: std::sync::Arc::new(Vec::new()),
            catalog: std::sync::Arc::new(crate::services::registry::CatalogService::new(&cfg)),
            http: reqwest::Client::new(),
            started_at: Utc::now(),
            pool,
            cfg,
        }
    }

    fn batch(frame: &str, attrs: Value, event: Option<IngestEvent>) -> AiIngest {
        AiIngest {
            camera_id: "cam1".into(),
            task_type: "anpr".into(),
            timestamp: None,
            frame_id: Some(frame.into()),
            detections: vec![DetectionIngest {
                label: Some("vehicle".into()),
                confidence: Some(0.9),
                bbox: None,
                track_id: None,
                attributes: Some(attrs),
            }],
            event,
            frame_ticket: None,
        }
    }

    async fn stored_source(st: &AppState, frame: &str) -> (String, String) {
        let (attrs, prov): (SqlxJson<Value>, String) =
            sqlx::query_as("SELECT attributes, provenance FROM detections WHERE frame_id = ?")
                .bind(frame)
                .fetch_one(&st.pool)
                .await
                .unwrap();
        (
            attrs.0["source"].as_str().unwrap_or_default().to_string(),
            prov,
        )
    }

    /// THE NEGATIVE CONTROL. A credential posting `attributes.source = "camera_native"` over the API
    /// gets `"worker"` persisted. Revert the rewrite and this fails.
    #[tokio::test]
    async fn a_forged_camera_native_source_is_rewritten_to_worker() {
        let st = test_state().await;
        let body = batch(
            "f1",
            json!({ "plate": "ABC1234", "source": "camera_native", "_prov": { "producer": "native_anpr" } }),
            None,
        );
        ingest_batch(&st, &body, &worker()).await.unwrap();

        let (source, provenance) = stored_source(&st, "f1").await;
        assert_eq!(
            source, "worker",
            "a batch posted over the API is always worker-provenance"
        );
        assert_eq!(provenance, "worker");

        // The forged `_prov` is replaced too, not merged into.
        let attrs: SqlxJson<Value> =
            sqlx::query_scalar("SELECT attributes FROM detections WHERE frame_id = 'f1'")
                .fetch_one(&st.pool)
                .await
                .unwrap();
        assert_eq!(attrs.0["_prov"]["key"], "key_a");
        assert_eq!(attrs.0["_prov"]["task"], "ai_1");
        assert!(attrs.0["_prov"].get("producer").is_none());
        // Everything the client legitimately said survives.
        assert_eq!(attrs.0["plate"], "ABC1234");
    }

    /// THE POSITIVE CONTROL. Guards against over-correcting into breaking camera-native ANPR: the
    /// kernel producer still stamps `camera_native`, which is what the entry engine weights.
    #[tokio::test]
    async fn the_kernel_producer_still_stamps_camera_native() {
        let st = test_state().await;
        ingest_batch(
            &st,
            &batch("f2", json!({ "plate": "WXY8888" }), None),
            &native(),
        )
        .await
        .unwrap();
        let (source, provenance) = stored_source(&st, "f2").await;
        assert_eq!(source, "camera_native");
        assert_eq!(provenance, "kernel:native_anpr");
    }

    /// The outbox forensics columns carry the same answer, so an incident responder can ask
    /// "which credential produced this?" in SQL.
    #[tokio::test]
    async fn outbox_records_who_produced_the_batch() {
        let st = test_state().await;
        ingest_batch(&st, &batch("f3", json!({}), None), &worker())
            .await
            .unwrap();
        let (prov, by): (String, Option<String>) =
            sqlx::query_as("SELECT provenance, produced_by FROM outbox WHERE frame_id = 'f3'")
                .fetch_one(&st.pool)
                .await
                .unwrap();
        assert_eq!(prov, "worker");
        assert_eq!(by.as_deref(), Some("apikey:key_a"));
    }

    /// Reserved kernel-domain event types cannot be raised by a worker — they reach webhooks and
    /// operator email, where a forged one is indistinguishable from a real barrier actuation.
    #[tokio::test]
    async fn worker_events_cannot_forge_kernel_domain_types() {
        let st = test_state().await;
        for forged in ["gate_opened", "entry_matched", "zone_foo", "disk_failed"] {
            let body = batch(
                &format!("ev_{forged}"),
                json!({}),
                Some(IngestEvent {
                    event_type: forged.into(),
                    severity: Some("critical".into()),
                    payload: None,
                }),
            );
            let err = ingest_batch(&st, &body, &worker()).await.unwrap_err();
            assert!(
                matches!(err, AppError::BadRequest(_)),
                "{forged} should be refused, got {err:?}"
            );
        }
        // Nothing was written for the refused batches.
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM detections")
            .fetch_one(&st.pool)
            .await
            .unwrap();
        assert_eq!(rows, 0);

        // A third-party sidecar's own event type still works, with severity clamped.
        let body = batch(
            "ev_ok",
            json!({}),
            Some(IngestEvent {
                event_type: "my_custom_thing".into(),
                severity: Some("critical".into()),
                payload: None,
            }),
        );
        ingest_batch(&st, &body, &worker()).await.unwrap();
        let sev: String =
            sqlx::query_scalar("SELECT severity FROM events WHERE event_type = 'my_custom_thing'")
                .fetch_one(&st.pool)
                .await
                .unwrap();
        assert_eq!(sev, "warning", "a worker cannot self-escalate to critical");
    }

    /// A kernel producer is unrestricted — it IS the domain.
    #[tokio::test]
    async fn kernel_events_are_not_denylisted() {
        let st = test_state().await;
        let body = batch(
            "ev_kernel",
            json!({}),
            Some(IngestEvent {
                event_type: "entry_matched".into(),
                severity: Some("critical".into()),
                payload: None,
            }),
        );
        ingest_batch(&st, &body, &native()).await.unwrap();
        let sev: String =
            sqlx::query_scalar("SELECT severity FROM events WHERE event_type = 'entry_matched'")
                .fetch_one(&st.pool)
                .await
                .unwrap();
        assert_eq!(sev, "critical");
    }

    #[test]
    fn event_type_shape_is_enforced() {
        assert!(validate_worker_event_type("my_thing_2").is_ok());
        assert!(validate_worker_event_type("").is_err());
        assert!(validate_worker_event_type("Has-Caps").is_err());
        assert!(validate_worker_event_type(&"x".repeat(65)).is_err());
        assert!(validate_worker_event_type("gate_opened").is_err());
    }

    #[test]
    fn a_non_object_attributes_blob_is_wrapped_not_dropped() {
        let out = stamp_attributes(Some(&json!("just a string")), &worker());
        assert_eq!(out["source"], "worker");
        assert_eq!(out["_client"], "just a string");
    }
}
