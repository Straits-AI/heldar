//! Wire/response types for the HikVision ISAPI camera-configuration service.
//!
//! These mirror the ISAPI elements the service reads/writes (deviceInfo, Streaming/channels,
//! System/time, System/Network/Integrate, Security/ONVIF/users, overlays) and the kernel's own
//! request/response bodies. All are snake_case JSON; the device-facing enum values
//! (`administrator|operator|mediaUser`) are carried verbatim by `OnvifUserType`.

use serde::{Deserialize, Serialize};

/// Default ONVIF username the kernel provisions when enabling ONVIF on a camera.
fn default_onvif_username() -> String {
    "heldar_onvif".to_string()
}

/// Device identity from `GET /ISAPI/System/deviceInfo`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DeviceInfo {
    pub device_name: Option<String>,
    pub model: Option<String>,
    pub firmware_version: Option<String>,
    pub serial_number: Option<String>,
}

/// A streaming channel's video encoding configuration (`GET /ISAPI/Streaming/channels/{id}`).
/// `fps` is centi-fps as the device reports it (2000 = 20fps); `bitrate`/`vbr_upper_cap` are kbps.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct VideoConfig {
    pub channel_id: i64,
    pub channel_name: Option<String>,
    pub codec: String,
    pub width: i64,
    pub height: i64,
    pub fps: i64,
    pub quality_control: String,
    pub bitrate: i64,
    pub vbr_upper_cap: i64,
    pub gop: i64,
}

/// Partial update to a [`VideoConfig`] (read-modify-write); every field is optional.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct VideoConfigPatch {
    #[serde(default)]
    pub codec: Option<String>,
    #[serde(default)]
    pub width: Option<i64>,
    #[serde(default)]
    pub height: Option<i64>,
    #[serde(default)]
    pub fps: Option<i64>,
    #[serde(default)]
    pub quality_control: Option<String>,
    #[serde(default)]
    pub bitrate: Option<i64>,
    #[serde(default)]
    pub vbr_upper_cap: Option<i64>,
    #[serde(default)]
    pub gop: Option<i64>,
}

/// Device clock configuration (`GET/PUT /ISAPI/System/time`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TimeConfig {
    /// `manual` or `NTP`.
    pub time_mode: String,
    /// ISO8601 local time with tz offset.
    pub local_time: String,
    /// e.g. `CST-8:00:00`.
    pub time_zone: String,
}

/// NTP server configuration (`GET/PUT /ISAPI/System/time/ntpServers/1`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct NtpConfig {
    /// `hostname` or `ipaddress`.
    pub addressing_format: String,
    pub host_name: String,
    pub port: i64,
}

/// Integration toggles from `GET/PUT /ISAPI/System/Network/Integrate`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OnvifSettings {
    pub onvif_enabled: bool,
    pub isapi_enabled: bool,
}

/// ONVIF user role (`/ISAPI/Security/ONVIF/users`). Carries the device's verbatim `userType` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OnvifUserType {
    Administrator,
    Operator,
    MediaUser,
}

/// Request to ensure a dedicated ONVIF user exists on the device (create-if-absent).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct EnsureOnvifUserRequest {
    #[serde(default = "default_onvif_username")]
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub user_type: Option<OnvifUserType>,
}

/// On-screen-display overlay configuration
/// (`GET/PUT /ISAPI/System/Video/inputs/channels/1/overlays`). Style fields are optional.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OsdConfig {
    pub datetime_enabled: bool,
    pub channel_name_enabled: bool,
    #[serde(default)]
    pub date_style: Option<String>,
    #[serde(default)]
    pub time_style: Option<String>,
    #[serde(default)]
    pub display_week: Option<bool>,
}

/// Reboot request body (`PUT /ISAPI/System/reboot` — DISRUPTIVE; requires explicit confirmation).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RebootRequest {
    pub confirm: bool,
}

/// A single configuration action applied across one or more cameras.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BulkAction {
    /// Enable ONVIF + ISAPI integration and provision a dedicated ONVIF user.
    EnableOnvif {
        #[serde(default = "default_onvif_username")]
        onvif_username: String,
        onvif_password: String,
    },
    /// Switch the clock to NTP (optionally setting the NTP server first).
    SyncTime {
        #[serde(default)]
        ntp_server: Option<String>,
    },
    /// Set the NTP server hostname/address.
    SetNtp { ntp_server: String },
    /// Apply a video-encoding patch to a channel (None = the camera's main channel).
    SetVideo {
        #[serde(default)]
        channel: Option<i64>,
        patch: VideoConfigPatch,
    },
}

/// Apply a [`BulkAction`] to a set of cameras (`camera_ids` None = every camera).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BulkConfigRequest {
    #[serde(default)]
    pub camera_ids: Option<Vec<String>>,
    pub action: BulkAction,
}

/// Per-camera outcome of a bulk action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BulkCameraResult {
    pub camera_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Aggregate result of a bulk action across all targeted cameras.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BulkConfigResponse {
    pub results: Vec<BulkCameraResult>,
    pub succeeded: usize,
    pub failed: usize,
}
