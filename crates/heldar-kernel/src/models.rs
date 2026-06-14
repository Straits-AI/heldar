use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::types::Json;
use sqlx::FromRow;

use crate::camera_url;

/// Camera row as stored. `password` is never serialized to clients; use [`CameraView`] for output.
#[derive(Debug, Clone, FromRow)]
pub struct Camera {
    pub id: String,
    pub site_id: Option<String>,
    pub name: String,
    pub vendor: String,
    pub model: Option<String>,
    pub address: Option<String>,
    pub rtsp_port: i64,
    pub username: Option<String>,
    pub password: Option<String>,
    pub main_stream_url: Option<String>,
    pub sub_stream_url: Option<String>,
    pub record_stream: String,
    pub codec: Option<String>,
    pub resolution_main: Option<String>,
    pub resolution_sub: Option<String>,
    pub fps_main: Option<i64>,
    pub fps_sub: Option<i64>,
    pub capabilities: Json<Value>,
    pub record_enabled: bool,
    pub segment_seconds: i64,
    pub retention_hours: i64,
    /// Per-camera storage quota in bytes; NULL means no per-camera cap.
    pub storage_quota_bytes: Option<i64>,
    /// Record the camera's audio stream (pass-through) instead of dropping it.
    pub record_audio: bool,
    /// When the recorder runs: `continuous` | `scheduled` | `event` | `scheduled_event`.
    pub record_mode: String,
    /// Event recording: footage desired BEFORE a trigger (best-effort, see recorder service).
    pub pre_roll_seconds: i64,
    /// Event recording: how long the recorder keeps writing after a trigger (the trigger window).
    pub post_roll_seconds: i64,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Camera {
    /// Whether the recorder should be running a process for this camera.
    pub fn should_record(&self) -> bool {
        self.enabled && self.record_enabled
    }
}

/// Client-facing camera representation: credentials stripped, stream URLs masked.
#[derive(Debug, Clone, Serialize)]
pub struct CameraView {
    pub id: String,
    pub site_id: Option<String>,
    pub name: String,
    pub vendor: String,
    pub model: Option<String>,
    pub address: Option<String>,
    pub rtsp_port: i64,
    pub username: Option<String>,
    pub has_password: bool,
    pub record_stream: String,
    /// Effective RTSP URL for the recorded stream, with credentials masked.
    pub record_url_masked: Option<String>,
    pub codec: Option<String>,
    pub resolution_main: Option<String>,
    pub resolution_sub: Option<String>,
    pub fps_main: Option<i64>,
    pub fps_sub: Option<i64>,
    pub capabilities: Value,
    pub record_enabled: bool,
    pub segment_seconds: i64,
    pub retention_hours: i64,
    pub storage_quota_bytes: Option<i64>,
    pub record_audio: bool,
    pub record_mode: String,
    pub pre_roll_seconds: i64,
    pub post_roll_seconds: i64,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<Camera> for CameraView {
    fn from(c: Camera) -> Self {
        let record_url_masked = camera_url::record_url(&c).map(|u| camera_url::mask_url(&u));
        CameraView {
            id: c.id,
            site_id: c.site_id,
            name: c.name,
            vendor: c.vendor,
            model: c.model,
            address: c.address,
            rtsp_port: c.rtsp_port,
            username: c.username,
            has_password: c
                .password
                .as_deref()
                .map(|p| !p.is_empty())
                .unwrap_or(false),
            record_stream: c.record_stream,
            record_url_masked,
            codec: c.codec,
            resolution_main: c.resolution_main,
            resolution_sub: c.resolution_sub,
            fps_main: c.fps_main,
            fps_sub: c.fps_sub,
            capabilities: c.capabilities.0,
            record_enabled: c.record_enabled,
            segment_seconds: c.segment_seconds,
            retention_hours: c.retention_hours,
            storage_quota_bytes: c.storage_quota_bytes,
            record_audio: c.record_audio,
            record_mode: c.record_mode,
            pre_roll_seconds: c.pre_roll_seconds,
            post_roll_seconds: c.post_roll_seconds,
            enabled: c.enabled,
            created_at: c.created_at,
            updated_at: c.updated_at,
        }
    }
}

/// Payload to create a camera. `id` may be omitted (slug auto-derived from name).
#[derive(Debug, Deserialize)]
pub struct CameraCreate {
    pub id: Option<String>,
    pub name: String,
    pub site_id: Option<String>,
    #[serde(default = "default_vendor")]
    pub vendor: String,
    pub model: Option<String>,
    pub address: Option<String>,
    pub rtsp_port: Option<i64>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub main_stream_url: Option<String>,
    pub sub_stream_url: Option<String>,
    pub record_stream: Option<String>,
    pub capabilities: Option<Value>,
    pub record_enabled: Option<bool>,
    pub segment_seconds: Option<i64>,
    pub retention_hours: Option<i64>,
    pub storage_quota_bytes: Option<i64>,
    pub record_audio: Option<bool>,
    pub record_mode: Option<String>,
    pub pre_roll_seconds: Option<i64>,
    pub post_roll_seconds: Option<i64>,
    pub enabled: Option<bool>,
}

fn default_vendor() -> String {
    "generic".to_string()
}

/// Partial update; only present fields are changed.
#[derive(Debug, Deserialize, Default)]
pub struct CameraUpdate {
    pub name: Option<String>,
    pub site_id: Option<String>,
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub address: Option<String>,
    pub rtsp_port: Option<i64>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub main_stream_url: Option<String>,
    pub sub_stream_url: Option<String>,
    pub record_stream: Option<String>,
    pub capabilities: Option<Value>,
    pub record_enabled: Option<bool>,
    pub segment_seconds: Option<i64>,
    pub retention_hours: Option<i64>,
    pub storage_quota_bytes: Option<i64>,
    pub record_audio: Option<bool>,
    pub record_mode: Option<String>,
    pub pre_roll_seconds: Option<i64>,
    pub post_roll_seconds: Option<i64>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Segment {
    pub id: String,
    pub camera_id: String,
    pub path: String,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub duration_s: f64,
    pub codec: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub size_bytes: i64,
    pub container: String,
    /// Transient read-lock held by clip/snapshot export; cleared at startup. Not durable.
    pub locked: bool,
    /// Durable evidence hold: when true the segment is never pruned by retention. Set via the
    /// incident API; survives restarts (unlike `locked`).
    pub evidence_locked: bool,
    pub incident_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct CameraStatus {
    pub camera_id: String,
    pub state: String,
    pub last_segment_at: Option<DateTime<Utc>>,
    pub last_started_at: Option<DateTime<Utc>>,
    pub reconnect_count: i64,
    pub segments_written: i64,
    pub fps_observed: Option<f64>,
    pub bitrate_kbps: Option<f64>,
    pub last_error: Option<String>,
    pub recorder_pid: Option<i64>,
    pub updated_at: DateTime<Utc>,
}

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

// ---- Stage 2: AI frame sampling ----

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
#[derive(Debug, Deserialize)]
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

// ---- Stage 3: zones + zone events ----

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

// ---- Stage 4: Access control — RBAC ----

/// Operator account. `password_hash` is never serialized; use [`UserView`] for output.
#[derive(Debug, Clone, FromRow)]
pub struct User {
    pub id: String,
    pub username: String,
    pub password_hash: String,
    pub role: String,
    pub display_name: Option<String>,
    pub active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserView {
    pub id: String,
    pub username: String,
    pub role: String,
    pub display_name: Option<String>,
    pub active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<User> for UserView {
    fn from(u: User) -> Self {
        UserView {
            id: u.id,
            username: u.username,
            role: u.role,
            display_name: u.display_name,
            active: u.active,
            created_at: u.created_at,
            updated_at: u.updated_at,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct UserCreate {
    pub username: String,
    pub password: String,
    pub role: Option<String>,
    pub display_name: Option<String>,
    pub active: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct UserUpdate {
    pub password: Option<String>,
    pub role: Option<String>,
    pub display_name: Option<String>,
    pub active: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct ApiKey {
    pub id: String,
    pub name: String,
    /// Mapped from the row for completeness; never exposed (see [`ApiKeyView`]).
    pub key_hash: String,
    pub key_prefix: String,
    pub role: String,
    pub active: bool,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiKeyView {
    pub id: String,
    pub name: String,
    pub key_prefix: String,
    pub role: String,
    pub active: bool,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl From<ApiKey> for ApiKeyView {
    fn from(k: ApiKey) -> Self {
        ApiKeyView {
            id: k.id,
            name: k.name,
            key_prefix: k.key_prefix,
            role: k.role,
            active: k.active,
            last_used_at: k.last_used_at,
            created_at: k.created_at,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ApiKeyCreate {
    pub name: String,
    pub role: Option<String>,
}

// ---- Scheduled interval snapshots ----

/// A per-camera schedule that captures a live JPEG every `interval_seconds`.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct SnapshotSchedule {
    pub id: String,
    pub camera_id: String,
    pub interval_seconds: i64,
    pub enabled: bool,
    pub last_fired_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct SnapshotScheduleCreate {
    pub interval_seconds: Option<i64>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct SnapshotScheduleUpdate {
    pub interval_seconds: Option<i64>,
    pub enabled: Option<bool>,
}

/// A captured snapshot frame on disk (one file under snapshots_dir/{camera_id}/).
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct PersistedSnapshot {
    pub id: String,
    pub camera_id: String,
    pub schedule_id: Option<String>,
    pub path: String,
    pub taken_at: DateTime<Utc>,
    pub size_bytes: i64,
    pub created_at: DateTime<Utc>,
}

// ---- Per-camera recording schedule (time-of-day windows) ----

/// A recurring per-camera recording window, applied when the camera's `record_mode` is `scheduled`
/// or `scheduled_event`. `days` is a JSON array of weekday ints (0=Mon..6=Sun); `time_start` /
/// `time_end` are "HH:MM" 24h in the SERVER's LOCAL timezone (chrono::Local). When `time_start` >
/// `time_end` the window wraps past midnight (its early-morning portion is attributed to the day it
/// started on).
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct RecordSchedule {
    pub id: String,
    pub camera_id: String,
    pub days: Json<Value>,
    pub time_start: String,
    pub time_end: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct RecordScheduleCreate {
    /// JSON array of weekday ints (0=Mon..6=Sun).
    pub days: Value,
    /// "HH:MM" 24h, server local time.
    pub time_start: String,
    /// "HH:MM" 24h, server local time (start > end means an overnight window).
    pub time_end: String,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct RecordScheduleUpdate {
    pub days: Option<Value>,
    pub time_start: Option<String>,
    pub time_end: Option<String>,
    pub enabled: Option<bool>,
}
