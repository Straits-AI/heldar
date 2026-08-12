use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::types::Json;
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Event {
    pub id: String,
    pub camera_id: Option<String>,
    pub site_id: Option<String>,
    pub event_type: String,
    pub severity: String,
    pub timestamp: DateTime<Utc>,
    pub payload: Json<Value>,
    pub created_at: DateTime<Utc>,
}

/// A perception task to run on a camera (consumed by AI workers).
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AiTask {
    pub id: String,
    pub camera_id: String,
    pub task_type: String,
    pub enabled: bool,
    pub stream_profile: String,
    pub fps: f64,
    pub width: i64,
    pub config: Json<Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct AiTaskCreate {
    pub task_type: String,
    pub stream_profile: Option<String>,
    pub fps: Option<f64>,
    pub width: Option<i64>,
    pub config: Option<Value>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct AiTaskUpdate {
    pub task_type: Option<String>,
    pub stream_profile: Option<String>,
    pub fps: Option<f64>,
    pub width: Option<i64>,
    pub config: Option<Value>,
    pub enabled: Option<bool>,
}

/// A detection result posted by an AI worker.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Detection {
    pub id: String,
    pub camera_id: String,
    pub task_type: String,
    pub timestamp: DateTime<Utc>,
    pub label: Option<String>,
    pub confidence: Option<f64>,
    pub bbox: Option<Json<Value>>,
    pub track_id: Option<String>,
    pub attributes: Json<Value>,
    /// Worker-supplied per-camera frame id this detection belongs to (idempotency / batch grouping).
    pub frame_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// One detection inside an ingest request.
// `Serialize` so the Wasm plugin host (heldar-wasm) can marshal a batch to JSON for a sandboxed guest.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DetectionIngest {
    pub label: Option<String>,
    pub confidence: Option<f64>,
    pub bbox: Option<Value>,
    pub track_id: Option<String>,
    pub attributes: Option<Value>,
}

/// A kernel-internal detection producer.
///
/// A CLOSED set on purpose. `Provenance::Kernel` is what makes a batch authoritative — a native ANPR
/// read weighted to the gate's whole vote threshold — so the producer name must never be a string that
/// could originate, however indirectly, in a request body. There is exactly one variant today, and
/// adding one is a deliberate kernel change reviewed as such.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelProducer {
    /// The camera-native ANPR poller (`services::native_anpr`) — the ONLY producer of `camera_native`.
    NativeAnpr,
}

impl KernelProducer {
    pub fn as_str(self) -> &'static str {
        match self {
            KernelProducer::NativeAnpr => "native_anpr",
        }
    }

    /// The `attributes.source` marker this producer's detections are stamped with. This is the value
    /// `heldar-entry`'s ANPR engine keys on to treat a read as authoritative.
    pub fn source(self) -> &'static str {
        match self {
            KernelProducer::NativeAnpr => "camera_native",
        }
    }
}

/// WHO produced a detection batch — a PARAMETER of ingest, never a field of the payload.
///
/// This is the whole fix for forgeable provenance. `attributes.source` used to arrive inside
/// client-supplied detection attributes, so anything holding an integration key could assert
/// `source = "camera_native"` and have `heldar-entry` treat a single forged plate read as authoritative
/// enough to open a barrier. Now the ingest path REWRITES the attributes from this parameter before
/// persisting, and the external HTTP handler can only ever construct [`Provenance::Worker`].
#[derive(Debug, Clone)]
pub enum Provenance {
    /// Produced inside the kernel process by a named, closed-set producer.
    Kernel { producer: KernelProducer },
    /// Posted over the HTTP ingest API by a credential. `task_id` / `worker_id` are present only when
    /// the batch arrived with a valid frame ticket (i.e. under a live lease).
    Worker {
        api_key_id: String,
        task_id: Option<String>,
        worker_id: Option<String>,
    },
}

impl Provenance {
    /// The `attributes.source` value written for this producer. Server-authored, always.
    pub fn source(&self) -> &'static str {
        match self {
            Provenance::Kernel { producer } => producer.source(),
            Provenance::Worker { .. } => "worker",
        }
    }

    /// The value persisted in `detections.provenance` / `outbox.provenance` for SQL-queryable
    /// forensics. `'client'` is reserved for pre-migration rows and means UNTRUSTED.
    pub fn column(&self) -> String {
        match self {
            Provenance::Kernel { producer } => format!("kernel:{}", producer.as_str()),
            Provenance::Worker { .. } => "worker".to_string(),
        }
    }

    /// `outbox.produced_by`: which credential (or kernel producer) is answerable for this batch.
    pub fn produced_by(&self) -> String {
        match self {
            Provenance::Kernel { producer } => format!("kernel:{}", producer.as_str()),
            Provenance::Worker { api_key_id, .. } => format!("apikey:{api_key_id}"),
        }
    }

    /// Namespace a producer-supplied `frame_id` so the idempotency key `(camera_id, frame_id)` cannot
    /// be claimed across provenance boundaries.
    ///
    /// Kernel producers use PREDICTABLE ids — the native-ANPR poller derives `nanpr_<device picture
    /// name>` — so without this a client can guess one, post it first, and the genuine camera-native
    /// read that follows is silently absorbed as a redelivery. That suppresses precisely the reads the
    /// barrier trusts most, and it is available to any ingest-capable credential. Prefixing is
    /// structural: it needs no list of reserved names to stay correct as producers are added.
    ///
    /// Redeliveries still dedup, because the same producer always lands in the same namespace.
    pub fn namespaced_frame_id(&self, frame_id: &str) -> String {
        match self {
            Provenance::Kernel { producer } => format!("k:{}:{frame_id}", producer.as_str()),
            Provenance::Worker { .. } => format!("w:{frame_id}"),
        }
    }

    /// The `_prov` object embedded in every detection's attributes, so a consumer reading the batch
    /// (inline or on fan-out replay) sees the same server-authored trail the columns carry.
    pub fn detail(&self) -> Value {
        match self {
            Provenance::Kernel { producer } => serde_json::json!({ "producer": producer.as_str() }),
            Provenance::Worker {
                api_key_id,
                task_id,
                worker_id,
            } => serde_json::json!({
                "key": api_key_id,
                "task": task_id,
                "worker": worker_id,
            }),
        }
    }

    pub fn is_worker(&self) -> bool {
        matches!(self, Provenance::Worker { .. })
    }
}

/// Optional event an AI worker can raise alongside its detections.
#[derive(Debug, Deserialize)]
pub struct IngestEvent {
    pub event_type: String,
    pub severity: Option<String>,
    pub payload: Option<Value>,
}

/// Payload an AI worker POSTs to ingest detections (and optionally an event) for a camera.
#[derive(Debug, Deserialize)]
pub struct AiIngest {
    pub camera_id: String,
    pub task_type: String,
    pub timestamp: Option<String>,
    /// Optional per-camera monotonic frame id. When present, ingest is idempotent on
    /// (camera_id, frame_id): a duplicate redelivery is a no-op (no double-insert, no re-fire of
    /// consumer side effects). Omit it (e.g. the dependency-light client) to accept every batch.
    pub frame_id: Option<String>,
    #[serde(default)]
    pub detections: Vec<DetectionIngest>,
    pub event: Option<IngestEvent>,
    /// Server-issued frame ticket from `x-frame-ticket` on the frame this batch describes.
    ///
    /// When present and valid, `camera_id`, `task_type` and `frame_id` are all DERIVED from it and the
    /// body's own values are only cross-checked (409 on disagreement) — a worker can only speak about
    /// frames it was actually handed. Required under `HELDAR_INGEST_PROVENANCE=enforce`.
    pub frame_ticket: Option<String>,
}

/// A polygon region on a camera; tracked detections crossing it raise enter/exit/dwell events.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Zone {
    pub id: String,
    pub camera_id: String,
    pub name: String,
    pub kind: String,
    /// JSON array of [x, y] vertices, normalized 0..1.
    pub polygon: Json<Value>,
    pub dwell_seconds: f64,
    /// JSON array of detection labels that count toward this zone (empty = all labels).
    pub labels: Json<Value>,
    pub severity: String,
    pub config: Json<Value>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct ZoneCreate {
    pub name: String,
    pub kind: Option<String>,
    pub polygon: Value,
    pub dwell_seconds: Option<f64>,
    pub labels: Option<Value>,
    pub severity: Option<String>,
    pub config: Option<Value>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ZoneUpdate {
    pub name: Option<String>,
    pub kind: Option<String>,
    pub polygon: Option<Value>,
    pub dwell_seconds: Option<f64>,
    pub labels: Option<Value>,
    pub severity: Option<String>,
    pub config: Option<Value>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ZoneEvent {
    pub id: String,
    pub camera_id: String,
    pub zone_id: String,
    pub zone_name: String,
    pub track_id: Option<String>,
    pub event_type: String,
    pub label: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub dwell_seconds: Option<f64>,
    pub evidence_path: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod provenance_tests {
    use super::*;

    #[test]
    fn a_worker_cannot_claim_a_kernel_frame_id() {
        let kernel = Provenance::Kernel {
            producer: KernelProducer::NativeAnpr,
        };
        let worker = Provenance::Worker {
            api_key_id: "key_1".into(),
            task_id: None,
            worker_id: None,
        };
        // The suppression primitive this closes: the native poller's ids are derived from the device
        // picture name, so a client can guess one. Posting it first used to claim the shared
        // `(camera_id, frame_id)` dedup key and silently swallow the genuine camera-native read.
        let guessed = "nanpr_PIC_00123";
        assert_ne!(
            kernel.namespaced_frame_id(guessed),
            worker.namespaced_frame_id(guessed),
            "a worker must not be able to occupy a kernel producer's idempotency key"
        );
        // Redelivery still dedups — same producer, same key — or at-least-once delivery would
        // double-count every ANPR vote.
        assert_eq!(
            worker.namespaced_frame_id(guessed),
            worker.namespaced_frame_id(guessed)
        );
        // Two kernel producers would also be distinct if another is ever added.
        assert!(kernel.namespaced_frame_id(guessed).starts_with("k:"));
        assert!(worker.namespaced_frame_id(guessed).starts_with("w:"));
    }

    #[test]
    fn worker_provenance_never_reports_a_kernel_source() {
        for p in [
            Provenance::Worker {
                api_key_id: "k".into(),
                task_id: None,
                worker_id: None,
            },
            Provenance::Worker {
                api_key_id: "k".into(),
                task_id: Some("t".into()),
                worker_id: Some("w".into()),
            },
        ] {
            assert_eq!(p.source(), "worker");
            assert_ne!(p.source(), KernelProducer::NativeAnpr.source());
        }
    }
}
