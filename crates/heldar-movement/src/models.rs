//! Movement-intelligence data models.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct CameraLink {
    pub id: String,
    pub from_camera: String,
    pub to_camera: String,
    pub transit_seconds: i64,
    pub bidirectional: bool,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CameraLinkCreate {
    pub from_camera: String,
    pub to_camera: String,
    pub transit_seconds: Option<i64>,
    pub bidirectional: Option<bool>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct MovementCandidate {
    pub id: String,
    pub subject_type: String,
    pub anchor: Option<String>,
    pub from_camera: Option<String>,
    pub from_ref: Option<String>,
    pub from_time: Option<DateTime<Utc>>,
    pub to_camera: Option<String>,
    pub to_ref: Option<String>,
    pub to_time: Option<DateTime<Utc>>,
    pub transit_seconds: Option<f64>,
    pub score: f64,
    pub signals: Json<serde_json::Value>,
    pub status: String,
    pub reviewed_by: Option<String>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct BreachAlert {
    pub id: String,
    pub camera_id: Option<String>,
    pub zone_id: Option<String>,
    pub zone_name: Option<String>,
    pub zone_event_id: Option<String>,
    pub rule: String,
    pub subject_type: Option<String>,
    pub subject: Option<String>,
    pub track_id: Option<String>,
    pub severity: String,
    pub status: String,
    pub detail: Json<serde_json::Value>,
    pub evidence_path: Option<String>,
    pub created_at: DateTime<Utc>,
    pub resolved_by: Option<String>,
    pub resolved_at: Option<DateTime<Utc>>,
}
