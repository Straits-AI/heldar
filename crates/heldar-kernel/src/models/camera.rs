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
