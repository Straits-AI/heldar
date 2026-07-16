//! Camera configuration (HikVision ISAPI) service.
//!
//! Wire/response types ([`types`]) and the hand-rolled RFC 2617 HTTP Digest auth ([`digest`]) the
//! ISAPI calls require, the vendor-agnostic [`CameraConfigProvider`] trait, and its HikVision ISAPI
//! implementation ([`hikvision`]). Construct a provider for a camera with [`for_camera`].

pub mod digest;
pub mod hikvision;
pub mod types;

use async_trait::async_trait;

use crate::error::{AppError, AppResult};
use crate::models::Camera;
use types::{
    BuiltinDetection, DayNightConfig, DayNightPatch, DeviceInfo, ImageConfig, ImageConfigPatch,
    IoOutput, NativePlateRead, NtpConfig, OnvifSettings, OnvifUserType, OsdConfig, TimeConfig,
    VideoConfig,
};

/// The default answer for a device-control surface the vendor implementation does not provide.
fn unsupported<T>(what: &str) -> AppResult<T> {
    Err(AppError::BadRequest(format!(
        "{what} is not supported for this camera vendor"
    )))
}

/// A vendor-agnostic surface for reading and writing a camera's on-device configuration. The kernel
/// owns the persistence/audit; an implementor only talks the device's native protocol (HikVision
/// ISAPI today). All methods are best-effort against a live device and surface [`AppError`] on
/// transport/protocol failure.
#[async_trait]
pub trait CameraConfigProvider: Send + Sync {
    /// Device identity (name/model/firmware/serial).
    async fn get_device_info(&self) -> AppResult<DeviceInfo>;

    /// Every streaming channel's video-encoding configuration (main + sub + any extras).
    async fn list_video_configs(&self) -> AppResult<Vec<VideoConfig>>;

    /// One streaming channel's video-encoding configuration (e.g. `101` main, `102` sub).
    async fn get_video_config(&self, channel: u32) -> AppResult<VideoConfig>;

    /// Write a channel's video-encoding configuration (read-modify-write of the device's XML).
    async fn put_video_config(&self, channel: u32, cfg: &VideoConfig) -> AppResult<()>;

    /// The device clock configuration (mode/local-time/timezone).
    async fn get_time_config(&self) -> AppResult<TimeConfig>;

    /// Write the device clock configuration.
    async fn put_time_config(&self, cfg: &TimeConfig) -> AppResult<()>;

    /// The configured NTP server.
    async fn get_ntp_config(&self) -> AppResult<NtpConfig>;

    /// Write the NTP server.
    async fn put_ntp_config(&self, cfg: &NtpConfig) -> AppResult<()>;

    /// Switch the clock to NTP if it is currently in manual mode; returns the resulting clock config.
    async fn sync_time_now(&self) -> AppResult<TimeConfig>;

    /// The ONVIF/ISAPI integration toggles.
    async fn get_onvif_settings(&self) -> AppResult<OnvifSettings>;

    /// Write the ONVIF/ISAPI integration toggles.
    async fn put_onvif_settings(&self, cfg: &OnvifSettings) -> AppResult<()>;

    /// Ensure a dedicated ONVIF user exists (create-if-absent; a duplicate create is treated as Ok).
    async fn ensure_onvif_user(
        &self,
        username: &str,
        password: &str,
        user_type: OnvifUserType,
    ) -> AppResult<()>;

    /// The on-screen-display (timestamp / channel-name) overlay configuration.
    async fn get_osd_config(&self) -> AppResult<OsdConfig>;

    /// Write the on-screen-display overlay configuration.
    async fn put_osd_config(&self, cfg: &OsdConfig) -> AppResult<()>;

    /// Reboot the device (DISRUPTIVE).
    async fn reboot(&self) -> AppResult<()>;

    // ---- Device-control surfaces (day/night, image/lighting, IO outputs, on-board ANPR). ----
    // Default implementations report "unsupported" so a vendor implementation only overrides what
    // its device actually exposes; the capability probe (services::camera_control) records which
    // surfaces answered so the dashboard renders only real controls.

    /// The day/night (IR-cut filter) configuration.
    async fn get_day_night(&self) -> AppResult<DayNightConfig> {
        unsupported("day/night configuration")
    }

    /// Write the day/night configuration (read-modify-write; only present patch fields change).
    async fn put_day_night(&self, _patch: &DayNightPatch) -> AppResult<()> {
        unsupported("day/night configuration")
    }

    /// The image/lighting configuration (brightness/contrast/saturation, WDR, BLC, supplement light).
    async fn get_image_config(&self) -> AppResult<ImageConfig> {
        unsupported("image configuration")
    }

    /// Write the image/lighting configuration (read-modify-write per sub-resource; only present
    /// patch fields change).
    async fn put_image_config(&self, _patch: &ImageConfigPatch) -> AppResult<()> {
        unsupported("image configuration")
    }

    /// The supplement-light modes this device supports (from its capability document), e.g.
    /// `["eventIntelligence", "colorVuWhiteLight", "irLight", "close"]` on a hybrid-light camera
    /// or `["irLight", "close"]` on an IR-only one. Empty when the device has no supplement light.
    async fn supplement_light_modes(&self) -> AppResult<Vec<String>> {
        unsupported("supplement light")
    }

    /// The device's built-in (on-camera) detection features — motion, line-crossing,
    /// intrusion/field detection, etc. — with their current arm state where cheaply readable.
    /// These are the camera's OWN smart events (configured on-device today), distinct from
    /// Heldar's server-side zone engine.
    async fn list_builtin_detections(&self) -> AppResult<Vec<BuiltinDetection>> {
        unsupported("built-in detections")
    }

    /// Arm/disarm one built-in detection (`motion` | `line_crossing` | `intrusion`) on the device.
    async fn set_builtin_detection(&self, _kind: &str, _enabled: bool) -> AppResult<()> {
        unsupported("built-in detections")
    }

    /// Open the device's live event-notification stream (an endless multipart response consumed
    /// by `services::camera_events`). `stream_http` must be a client with NO total timeout — the
    /// caller owns an idle watchdog. Default: unsupported.
    async fn open_event_stream(
        &self,
        _stream_http: &reqwest::Client,
    ) -> AppResult<reqwest::Response> {
        unsupported("on-camera event stream")
    }

    /// The device's alarm/relay output ports.
    async fn list_io_outputs(&self) -> AppResult<Vec<IoOutput>> {
        unsupported("IO outputs")
    }

    /// Set one alarm/relay output `high` or `low` (`active: true` = high). The caller owns pulse
    /// semantics (set high, hold, set low) — see `services::camera_control::pulse_output`.
    async fn set_io_output(&self, _port: i64, _active: bool) -> AppResult<()> {
        unsupported("IO outputs")
    }

    /// Whether the device exposes an on-board ANPR (plate recognition) engine.
    async fn supports_native_anpr(&self) -> bool {
        false
    }

    /// Plate reads from the device's on-board ANPR engine strictly AFTER the given device-format
    /// cursor time (empty cursor = everything the device still buffers).
    async fn fetch_anpr_plates(&self, _after: &str) -> AppResult<Vec<NativePlateRead>> {
        unsupported("on-board ANPR")
    }
}

/// Build a [`CameraConfigProvider`] for `cam`, dispatching on its vendor. Only HikVision (ISAPI) is
/// supported today; ONVIF-generic configuration is a future implementation.
pub fn for_camera(
    cam: &Camera,
    http: &reqwest::Client,
    timeout_ms: u64,
) -> AppResult<Box<dyn CameraConfigProvider>> {
    match cam.vendor.as_str() {
        "hikvision" => Ok(Box::new(hikvision::HikVisionIsapiClient::for_camera(
            cam, http, timeout_ms,
        )?)),
        _ => Err(AppError::BadRequest(
            "camera config only supported for hikvision; ONVIF-generic is a future impl".into(),
        )),
    }
}
