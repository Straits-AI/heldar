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
    DeviceInfo, NtpConfig, OnvifSettings, OnvifUserType, OsdConfig, TimeConfig, VideoConfig,
};

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
