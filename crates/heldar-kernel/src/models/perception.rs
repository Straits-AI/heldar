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
