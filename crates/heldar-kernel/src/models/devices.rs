use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::types::Json;
use sqlx::FromRow;

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
