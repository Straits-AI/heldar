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
    /// Run a SECOND ffmpeg pipeline writing identical segments to HELDAR_MIRROR_RECORDINGS_DIR
    /// (redundant DVR copy). No-op unless the mirror dir is configured.
    pub mirror_enabled: bool,
    /// Let the ANR loop re-fetch missed footage from the camera's onboard storage to fill gaps.
    pub anr_enabled: bool,
    /// Optional replay URL template for ANR ({start}/{end} placeholders); NULL = default Hikvision
    /// RTSP playback built from address+credentials.
    pub anr_replay_url_template: Option<String>,
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
    pub mirror_enabled: bool,
    pub anr_enabled: bool,
    pub anr_replay_url_template: Option<String>,
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
            mirror_enabled: c.mirror_enabled,
            anr_enabled: c.anr_enabled,
            anr_replay_url_template: c.anr_replay_url_template,
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
    pub mirror_enabled: Option<bool>,
    pub anr_enabled: Option<bool>,
    pub anr_replay_url_template: Option<String>,
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
    pub mirror_enabled: Option<bool>,
    pub anr_enabled: Option<bool>,
    pub anr_replay_url_template: Option<String>,
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

/// A recording gap detected by the indexer (a hole > 3s between consecutive segments). The ANR loop
/// (services/anr.rs) tries to re-fill pending gaps from the camera's onboard storage. `fill_state` is
/// `pending` | `filled` | `failed`.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct RecordingGap {
    pub id: String,
    pub camera_id: String,
    pub gap_start: DateTime<Utc>,
    pub gap_end: DateTime<Utc>,
    pub gap_seconds: i64,
    pub fill_state: String,
    pub fill_attempts: i64,
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub filled_at: Option<DateTime<Utc>>,
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

// ---- Backup subsystem: destinations, policies, jobs, archive export ----

/// Config keys that hold a secret. Masked in [`BackupDestinationView`] (and preserved across an
/// update when the client round-trips the `***` placeholder back).
pub const BACKUP_SECRET_KEYS: &[&str] = &["pass", "password", "secret_key", "secret"];

/// A backup transfer target. `config` is a kind-specific JSON blob (credentials live here and are
/// never serialized raw — use [`BackupDestinationView`]). Not `Serialize` for exactly that reason.
#[derive(Debug, Clone, FromRow)]
pub struct BackupDestination {
    pub id: String,
    pub name: String,
    /// `local` | `sftp` | `ftp` | `s3`.
    pub kind: String,
    pub config: Json<Value>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Client-facing destination: secret config values are replaced with `***`.
#[derive(Debug, Clone, Serialize)]
pub struct BackupDestinationView {
    pub id: String,
    pub name: String,
    pub kind: String,
    /// The config blob with any secret values masked to `***`.
    pub config: Value,
    /// Whether at least one secret credential is configured (so the UI can show "set" without the value).
    pub has_credentials: bool,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Mask the secret values in a config blob, returning the masked blob and whether any secret was set.
pub fn mask_backup_config(mut config: Value) -> (Value, bool) {
    let mut has_credentials = false;
    if let Some(obj) = config.as_object_mut() {
        for key in BACKUP_SECRET_KEYS {
            if let Some(v) = obj.get_mut(*key) {
                if v.as_str().map(|s| !s.is_empty()).unwrap_or(false) {
                    has_credentials = true;
                    *v = Value::String("***".to_string());
                }
            }
        }
    }
    (config, has_credentials)
}

impl From<BackupDestination> for BackupDestinationView {
    fn from(d: BackupDestination) -> Self {
        let (config, has_credentials) = mask_backup_config(d.config.0);
        BackupDestinationView {
            id: d.id,
            name: d.name,
            kind: d.kind,
            config,
            has_credentials,
            enabled: d.enabled,
            created_at: d.created_at,
            updated_at: d.updated_at,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct BackupDestinationCreate {
    pub name: String,
    /// `local` | `sftp` | `ftp` | `s3`.
    pub kind: String,
    pub config: Option<Value>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct BackupDestinationUpdate {
    pub name: Option<String>,
    pub kind: Option<String>,
    pub config: Option<Value>,
    pub enabled: Option<bool>,
}

/// Result of POST /api/v1/backup/destinations/{id}/test (a connectivity / writability probe).
#[derive(Debug, Clone, Serialize)]
pub struct BackupTestResult {
    pub ok: bool,
    pub error: Option<String>,
    pub latency_ms: i64,
}

/// A scheduled backup policy: ship a camera selection's recent footage to a destination on an interval.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct BackupPolicy {
    pub id: String,
    pub name: String,
    pub destination_id: String,
    /// JSON array of camera ids; empty array means all cameras.
    pub camera_ids: Json<Value>,
    pub incident_lock_only: bool,
    pub schedule_interval_s: i64,
    pub lookback_hours: i64,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_job_id: Option<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct BackupPolicyCreate {
    pub name: String,
    pub destination_id: String,
    pub camera_ids: Option<Value>,
    pub incident_lock_only: Option<bool>,
    pub schedule_interval_s: Option<i64>,
    pub lookback_hours: Option<i64>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct BackupPolicyUpdate {
    pub name: Option<String>,
    pub destination_id: Option<String>,
    pub camera_ids: Option<Value>,
    pub incident_lock_only: Option<bool>,
    pub schedule_interval_s: Option<i64>,
    pub lookback_hours: Option<i64>,
    pub enabled: Option<bool>,
}

/// A single backup run (policy-scheduled, manually triggered, or an on-demand archive export).
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct BackupJob {
    pub id: String,
    pub policy_id: Option<String>,
    pub destination_id: Option<String>,
    /// `policy` | `on_demand_archive`.
    pub kind: String,
    pub camera_ids: Json<Value>,
    pub from_time: Option<DateTime<Utc>>,
    pub to_time: Option<DateTime<Utc>>,
    pub incident_lock_only: bool,
    /// `pending` | `running` | `completed` | `error`.
    pub status: String,
    pub files_total: i64,
    pub files_copied: i64,
    pub bytes_copied: i64,
    pub error: Option<String>,
    /// Filesystem path of the produced artifact (archive .zip), if any.
    pub output_path: Option<String>,
    /// Browser-fetchable URL of the produced artifact (under /media/archives/...), if any.
    pub output_url: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

// ---- ONVIF (Profile S MVP): device profile + PTZ presets ----

/// Per-camera ONVIF device profile, populated by [`crate::services::onvif::probe`]. `scopes` is a
/// JSON array of ONVIF scope URIs. `ptz_enabled` is true when the device exposes a PTZ service and
/// the chosen media profile carries a PTZConfiguration.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct CameraOnvif {
    pub camera_id: String,
    pub device_url: String,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub firmware_version: Option<String>,
    pub serial_number: Option<String>,
    pub hardware_id: Option<String>,
    pub scopes: Json<Value>,
    pub media_url: Option<String>,
    pub ptz_url: Option<String>,
    pub profile_token: Option<String>,
    pub ptz_node_token: Option<String>,
    pub ptz_enabled: bool,
    pub probed_at: DateTime<Utc>,
}

/// A PTZ preset fetched from a camera's ONVIF PTZ service (GetPresets). One row per (camera, token).
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct PtzPreset {
    pub id: String,
    pub camera_id: String,
    pub token: String,
    pub name: Option<String>,
    pub fetched_at: DateTime<Utc>,
}

// ---- Camera configuration (HikVision ISAPI): device + integration state ----

/// Per-camera HikVision ISAPI configuration state, populated by the camera-config service. Mirrors
/// `GET /ISAPI/System/deviceInfo` (identity), `/System/Network/Integrate` (`onvif_enabled`), the
/// kernel-provisioned ONVIF user (`onvif_user_created`), and `/System/time` (`time_mode`/`ntp_server`).
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct CameraIsapi {
    pub camera_id: String,
    pub device_name: Option<String>,
    pub model: Option<String>,
    pub firmware_version: Option<String>,
    pub serial_number: Option<String>,
    pub onvif_enabled: bool,
    pub onvif_user_created: bool,
    pub time_mode: Option<String>,
    pub ntp_server: Option<String>,
    pub fetched_at: DateTime<Utc>,
}

// ---- Webhook helpers (URL masking + three-state field deserialization) ----

/// Deserialize a PRESENT field into `Some(inner)`. Combined with `#[serde(default)]` (which leaves a
/// missing field as `None`), this yields three states: omitted = `None`, null = `Some(None)`,
/// value = `Some(Some(v))`.
fn de_field_present<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<String>::deserialize(deserializer)?))
}

/// Mask a webhook URL for display: keep only `scheme://host[:port]` and append `/…` so the path/token
/// is never revealed. Returns None for an empty url; a url without a scheme is masked to `…` (it may
/// be a bare token).
pub fn mask_webhook_url(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            let authority = &rest[..authority_end];
            if authority_end < rest.len() {
                Some(format!("{scheme}://{authority}/…"))
            } else {
                Some(format!("{scheme}://{authority}"))
            }
        }
        None => Some("…".to_string()),
    }
}

// ---- Webhook subscriptions (the generic event-delivery substrate; supersedes single-URL alerting) ----

/// A webhook subscription row as stored. `secret` (the HMAC signing key) is never serialized; use
/// [`WebhookSubscriptionView`] for output. `event_types` is a JSON array of type names; the sentinel
/// `["*"]` matches every event type, otherwise it is an exact-membership set. `cursor_at` is the
/// per-subscription delivery cursor (an `events.created_at`); NULL means "start at now" (no backlog).
#[derive(Debug, Clone, FromRow)]
pub struct WebhookSubscription {
    pub id: String,
    pub name: String,
    pub url: String,
    pub event_types: Json<Vec<String>>,
    pub min_severity: String,
    pub secret: Option<String>,
    pub enabled: bool,
    pub cursor_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Client-facing subscription view: the `secret` is replaced by a `has_secret` flag and never echoed.
#[derive(Debug, Clone, Serialize)]
pub struct WebhookSubscriptionView {
    pub id: String,
    pub name: String,
    pub url: String,
    pub event_types: Vec<String>,
    pub min_severity: String,
    /// Whether an HMAC signing secret is configured (the value itself is never returned).
    pub has_secret: bool,
    pub enabled: bool,
    pub cursor_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<WebhookSubscription> for WebhookSubscriptionView {
    fn from(s: WebhookSubscription) -> Self {
        WebhookSubscriptionView {
            id: s.id,
            name: s.name,
            url: s.url,
            event_types: s.event_types.0,
            min_severity: s.min_severity,
            has_secret: s.secret.as_deref().map(|v| !v.is_empty()).unwrap_or(false),
            enabled: s.enabled,
            cursor_at: s.cursor_at,
            created_at: s.created_at,
            updated_at: s.updated_at,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct WebhookSubscriptionCreate {
    pub name: String,
    pub url: String,
    /// Omitted/empty = all types (`["*"]`).
    pub event_types: Option<Vec<String>>,
    /// `info` | `warning` | `critical` (default `info`).
    pub min_severity: Option<String>,
    /// Optional HMAC-SHA256 signing secret.
    pub secret: Option<String>,
    pub enabled: Option<bool>,
}

/// Partial update; an ABSENT field is left unchanged. `secret` is three-state: omitted = unchanged,
/// null = clear the secret, a value = set it (the outer `Option` distinguishes "field omitted" from
/// an explicit null — see [`de_field_present`]).
#[derive(Debug, Deserialize, Default)]
pub struct WebhookSubscriptionUpdate {
    pub name: Option<String>,
    pub url: Option<String>,
    pub event_types: Option<Vec<String>>,
    pub min_severity: Option<String>,
    #[serde(default, deserialize_with = "de_field_present")]
    pub secret: Option<Option<String>>,
    pub enabled: Option<bool>,
}

/// One webhook delivery attempt (the at-least-once retry ledger). `status` is `delivered` | `failed`.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct WebhookDelivery {
    pub id: String,
    pub subscription_id: String,
    pub event_id: Option<String>,
    pub event_type: Option<String>,
    pub status: String,
    pub attempts: i64,
    pub response_code: Option<i64>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
}

/// Request body for POST /api/v1/archive/export — zip a selection of recorded footage on demand.
#[derive(Debug, Deserialize)]
pub struct ArchiveExportRequest {
    /// Camera ids to include; empty/omitted means all cameras.
    #[serde(default)]
    pub camera_ids: Vec<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub incident_lock_only: Option<bool>,
    /// Trim each segment to the [from, to] window (re-mux with -c copy); requires both bounds.
    pub trim: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_webhook_url_hides_path_and_token() {
        // Path/query/fragment are dropped behind an ellipsis; scheme + host (and port) are kept.
        assert_eq!(
            mask_webhook_url("https://hooks.slack.com/services/T000/B000/XXXXSECRET"),
            Some("https://hooks.slack.com/…".to_string())
        );
        assert_eq!(
            mask_webhook_url("https://example.com:8443/alert?token=abc"),
            Some("https://example.com:8443/…".to_string())
        );
        // Host-only urls keep just scheme://host.
        assert_eq!(
            mask_webhook_url("https://example.com"),
            Some("https://example.com".to_string())
        );
        // Empty/whitespace => None; schemeless => fully masked (may be a bare token).
        assert_eq!(mask_webhook_url("   "), None);
        assert_eq!(mask_webhook_url("not-a-url"), Some("…".to_string()));
    }

    #[test]
    fn webhook_update_secret_is_three_state() {
        // Omitted => None (leave the signing secret unchanged).
        let u: WebhookSubscriptionUpdate = serde_json::from_str(r#"{"enabled": true}"#).unwrap();
        assert!(u.secret.is_none());
        assert_eq!(u.enabled, Some(true));
        // Explicit null => Some(None) (clear the secret).
        let u: WebhookSubscriptionUpdate = serde_json::from_str(r#"{"secret": null}"#).unwrap();
        assert_eq!(u.secret, Some(None));
        // A value => Some(Some(v)) (set the secret).
        let u: WebhookSubscriptionUpdate = serde_json::from_str(r#"{"secret": "s3cr3t"}"#).unwrap();
        assert_eq!(u.secret, Some(Some("s3cr3t".to_string())));
    }
}
