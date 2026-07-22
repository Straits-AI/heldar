use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::types::Json;
use sqlx::FromRow;

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
