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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct DeviceInfo {
    pub device_name: Option<String>,
    pub model: Option<String>,
    pub firmware_version: Option<String>,
    pub serial_number: Option<String>,
}

/// A streaming channel's video encoding configuration (`GET /ISAPI/Streaming/channels/{id}`).
/// `fps` is centi-fps as the device reports it (2000 = 20fps); `bitrate`/`vbr_upper_cap` are kbps.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct NtpConfig {
    /// `hostname` or `ipaddress`.
    pub addressing_format: String,
    pub host_name: String,
    pub port: i64,
}

/// Integration toggles from `GET/PUT /ISAPI/System/Network/Integrate`.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct OnvifSettings {
    pub onvif_enabled: bool,
    pub isapi_enabled: bool,
}

/// ONVIF user role (`/ISAPI/Security/ONVIF/users`). Carries the device's verbatim `userType` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum OnvifUserType {
    Administrator,
    Operator,
    MediaUser,
}

/// Request to ensure a dedicated ONVIF user exists on the device (create-if-absent).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct RebootRequest {
    pub confirm: bool,
}

/// Day/night (IR-cut filter) configuration (`GET/PUT /ISAPI/Image/channels/1/ircutFilter`).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct DayNightConfig {
    /// `auto` | `day` | `night` | `schedule` (verbatim ISAPI `IrcutFilterType`).
    pub mode: String,
    /// Auto-switch sensitivity where the device exposes one (typically 0–7).
    #[serde(default)]
    pub sensitivity: Option<i64>,
}

/// Partial update to a [`DayNightConfig`] (read-modify-write); every field is optional.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct DayNightPatch {
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub sensitivity: Option<i64>,
}

/// Image/lighting configuration, aggregated from the per-channel ISAPI image sub-resources
/// (`/ISAPI/Image/channels/1/{color,WDR,BLC,supplementLight}`). Fields the device does not expose
/// are `None` and are never written back.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ImageConfig {
    /// 0–100 (`color`).
    #[serde(default)]
    pub brightness: Option<i64>,
    /// 0–100 (`color`).
    #[serde(default)]
    pub contrast: Option<i64>,
    /// 0–100 (`color`).
    #[serde(default)]
    pub saturation: Option<i64>,
    /// Wide dynamic range: `open` | `close` | `auto` (`WDR`).
    #[serde(default)]
    pub wdr_mode: Option<String>,
    /// WDR strength 0–100 (`WDR`).
    #[serde(default)]
    pub wdr_level: Option<i64>,
    /// Backlight compensation enabled (`BLC`).
    #[serde(default)]
    pub blc_enabled: Option<bool>,
    /// Supplement-light mode where exposed (`supplementLight/supplementLightMode`). Verified live
    /// values: `irLight` (infrared B/W), `colorVuWhiteLight` (white light, full-color night),
    /// `eventIntelligence` ("smart" — IR normally, white light on detected events), `close`.
    /// The camera's actual option list is in the capability map (`supplement_light_modes`).
    #[serde(default)]
    pub supplement_light_mode: Option<String>,
    /// White-light brightness 0–100 (white-light-capable models only).
    #[serde(default)]
    pub white_light_brightness: Option<i64>,
    /// IR-light brightness 0–100.
    #[serde(default)]
    pub ir_light_brightness: Option<i64>,
    /// Supplement-light brightness regulation: `auto` | `manual` (brightness sliders apply in
    /// `manual`; in `auto` the camera manages them).
    #[serde(default)]
    pub supplement_brightness_mode: Option<String>,
}

/// Partial update to an [`ImageConfig`]; only present fields are written to the device.
pub type ImageConfigPatch = ImageConfig;

/// One built-in (on-camera) detection feature reported by the device's smart-event capability
/// document. `kind` is a stable snake_case token (`motion`, `line_crossing`, `intrusion`, …);
/// `enabled` is the device's current arm state where it can be read cheaply (None = not read).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BuiltinDetection {
    pub kind: String,
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// One line-crossing rule slot on the device (`/ISAPI/Smart/LineDetection/1` `LineItem`). Devices
/// expose a fixed set of slots (4 on the verified DS-2CD3T56WDV3-L); an unused slot is `enabled:
/// false` with a degenerate line. Coordinates are normalized 0..1 in our API (the device speaks
/// 0..1000).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct SmartLine {
    pub id: i64,
    pub enabled: bool,
    /// 1–100.
    pub sensitivity: i64,
    /// `any` | `left-right` | `right-left` (verbatim device tokens).
    pub direction: String,
    /// Exactly two endpoints, normalized 0..1.
    #[schema(value_type = Vec<Vec<f64>>)]
    pub points: Vec<[f64; 2]>,
}

/// The device's line-crossing configuration: a master arm switch + the rule slots.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct LineCrossingConfig {
    pub enabled: bool,
    pub lines: Vec<SmartLine>,
}

/// One intrusion (field-detection) region slot (`/ISAPI/Smart/FieldDetection/1`
/// `FieldDetectionRegion`). An unconfigured slot carries NO coordinates on the device.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct SmartRegion {
    pub id: i64,
    pub enabled: bool,
    /// 1–100.
    pub sensitivity: i64,
    /// Seconds a target must stay inside before the alarm fires (device `timeThreshold`).
    pub time_threshold: i64,
    /// Polygon vertices, normalized 0..1 (empty = slot unconfigured).
    #[schema(value_type = Vec<Vec<f64>>)]
    pub points: Vec<[f64; 2]>,
}

/// The device's intrusion-detection configuration: a master arm switch + the region slots.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct IntrusionConfig {
    pub enabled: bool,
    pub regions: Vec<SmartRegion>,
}

/// The device's basic motion-detection configuration. The grid layout itself is left on-device
/// (a full-frame grid is the common default); only the arm switch + sensitivity are exposed.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct MotionConfig {
    pub enabled: bool,
    /// 0–100 where exposed (`MotionDetectionLayout/sensitivityLevel`).
    #[serde(default)]
    pub sensitivity: Option<i64>,
}

/// One alarm/relay output port (`GET /ISAPI/System/IO/outputs`).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct IoOutput {
    pub id: i64,
    #[serde(default)]
    pub name: Option<String>,
    /// Idle/default state as the device reports it (e.g. `low`).
    #[serde(default)]
    pub default_state: Option<String>,
}

/// One plate read returned by the device's on-board ANPR engine
/// (`POST /ISAPI/Traffic/channels/{n}/vehicleDetect/plates`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct NativePlateRead {
    /// Raw plate text as the camera read it.
    pub plate: String,
    /// The device's verbatim `captureTime` string (used as the poll cursor; format varies by
    /// firmware — typically `yyyyMMddHHmmssSSS` digits).
    pub capture_time: String,
    /// `forward` | `reverse` | `unknown` (verbatim device direction).
    #[serde(default)]
    pub direction: Option<String>,
    /// Device picture name — unique per read; used for idempotency when present.
    #[serde(default)]
    pub pic_name: Option<String>,
    /// Country/region code where reported.
    #[serde(default)]
    pub country: Option<String>,
}

/// A single configuration action applied across one or more cameras.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct BulkConfigRequest {
    #[serde(default)]
    pub camera_ids: Option<Vec<String>>,
    pub action: BulkAction,
}

/// Per-camera outcome of a bulk action.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct BulkCameraResult {
    pub camera_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Aggregate result of a bulk action across all targeted cameras.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct BulkConfigResponse {
    pub results: Vec<BulkCameraResult>,
    pub succeeded: usize,
    pub failed: usize,
}
