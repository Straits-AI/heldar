//! HikVision ISAPI implementation of [`CameraConfigProvider`].
//!
//! ISAPI is a plain HTTP(S) request/response API whose bodies are XML in the
//! `http://www.hikvision.com/ver20/XMLSchema` namespace. Authentication is HTTP Digest (RFC 2617):
//! every request is sent once unauthenticated, and on the `401` challenge an `Authorization: Digest`
//! header is built with [`super::digest::digest_auth_header`] and the request is retried once
//! ([`HikVisionIsapiClient::isapi_request_raw`]).
//!
//! All XML is parsed by substring extraction (the kernel's offline-build constraint forbids an XML
//! crate); the helpers below mirror `services/onvif.rs`. Writes are read-modify-write: GET the
//! current element, splice in the changed sub-fields, and PUT the result back so device-managed
//! fields (ids, namespaces, untouched sub-elements) are preserved verbatim.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Method, StatusCode};

use super::types::{
    BuiltinDetection, DayNightConfig, DayNightPatch, DeviceInfo, ImageConfig, ImageConfigPatch,
    IoOutput, NativePlateRead, NtpConfig, OnvifSettings, OnvifUserType, OsdConfig, TimeConfig,
    VideoConfig,
};
use super::CameraConfigProvider;
use crate::camera_url;
use crate::error::{AppError, AppResult};
use crate::models::Camera;

/// XML namespace every HikVision ISAPI body carries.
const HIK_NS: &str = "http://www.hikvision.com/ver20/XMLSchema";
/// Overlay (OSD) endpoint for the primary video input channel.
const OSD_PATH: &str = "/ISAPI/System/Video/inputs/channels/1/overlays";
/// ONVIF user provisioning endpoint.
const ONVIF_USERS_PATH: &str = "/ISAPI/Security/ONVIF/users";
/// Day/night (IR-cut filter) endpoint for the primary image channel.
const IRCUT_PATH: &str = "/ISAPI/Image/channels/1/ircutFilter";
/// Alarm/relay output ports endpoint.
const IO_OUTPUTS_PATH: &str = "/ISAPI/System/IO/outputs";
/// On-board ANPR plate-results endpoint (traffic cameras / ANPR barrier cameras).
const ANPR_PLATES_PATH: &str = "/ISAPI/Traffic/channels/1/vehicleDetect/plates";
/// On-camera event notification stream (multipart XML long-poll).
const ALERT_STREAM_PATH: &str = "/ISAPI/Event/notification/alertStream";
/// Smart-event capability flags (`/ISAPI/Smart/capabilities` `isSupportX` element → stable kind
/// token → optional config resource whose `<enabled>` gives the current arm state).
const SMART_DETECTIONS: &[(&str, &str, Option<&str>)] = &[
    (
        "isSupportLineDetection",
        "line_crossing",
        Some("/ISAPI/Smart/LineDetection/1"),
    ),
    (
        "isSupportFieldDetection",
        "intrusion",
        Some("/ISAPI/Smart/FieldDetection/1"),
    ),
    ("isSupportRegionEntrance", "region_entrance", None),
    ("isSupportRegionExiting", "region_exiting", None),
    ("isSupportLoitering", "loitering", None),
    ("isSupportFaceDetect", "face_detection", None),
    ("isSupportAudioDetection", "audio_detection", None),
    ("isSupportSceneChangeDetection", "scene_change", None),
    ("isSupportDefocusDetection", "defocus", None),
    ("isSupportRapidMove", "rapid_move", None),
    ("isSupportParking", "parking", None),
    ("isSupportUnattendedBaggage", "unattended_baggage", None),
];

/// A HikVision camera reached over ISAPI with HTTP Digest authentication.
pub struct HikVisionIsapiClient {
    base_url: String,
    username: String,
    password: String,
    http: reqwest::Client,
    timeout: Duration,
}

impl HikVisionIsapiClient {
    /// Build a client for `cam`. ISAPI is plain HTTP on port 80 unless the camera's `address` itself
    /// carries an explicit `host:port`. Requires credentials (Digest auth has no anonymous mode).
    pub fn for_camera(cam: &Camera, http: &reqwest::Client, timeout_ms: u64) -> AppResult<Self> {
        let host = cam
            .address
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                AppError::BadRequest(
                    "camera has no address; set its address to configure it".into(),
                )
            })?;
        let username = cam.username.clone().unwrap_or_default();
        if username.is_empty() {
            return Err(AppError::BadRequest(
                "camera has no credentials; ISAPI configuration requires a username/password"
                    .into(),
            ));
        }
        // Decrypt the stored credential (encryption-at-rest seals it as `enc:v1:…`). Previously the raw
        // sealed blob was sent as the Digest password, so enabling HELDAR_SECRET_KEY broke all ISAPI
        // config. `decrypted_password` passes plaintext through unchanged when no key is configured.
        let password = camera_url::decrypted_password(cam).unwrap_or_default();
        Ok(Self {
            base_url: format!("http://{host}"),
            username,
            password,
            http: http.clone(),
            timeout: Duration::from_millis(timeout_ms.max(500)),
        })
    }

    /// Perform the two-leg Digest dance and return the final `(status, body)` WITHOUT mapping a
    /// non-2xx status to an error (callers that tolerate 4xx — e.g. duplicate-user creates — use this
    /// directly). Send once unauthenticated; on `401`, build an `Authorization: Digest` from the
    /// `WWW-Authenticate` challenge and retry exactly once.
    async fn isapi_request_raw(
        &self,
        method: Method,
        path: &str,
        body: Option<String>,
    ) -> AppResult<(StatusCode, String)> {
        let url = format!("{}{}", self.base_url, path);

        // Leg 1: unauthenticated probe (ISAPI answers 401 with a Digest challenge).
        let mut req = self
            .http
            .request(method.clone(), url.as_str())
            .timeout(self.timeout);
        if let Some(b) = body.clone() {
            req = req
                .header(reqwest::header::CONTENT_TYPE, "application/xml")
                .body(b);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| AppError::Other(anyhow::anyhow!("ISAPI {method} {path} failed: {e}")))?;

        if resp.status() != StatusCode::UNAUTHORIZED {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Ok((status, text));
        }

        // Leg 2: answer the Digest challenge and retry once.
        let www = resp
            .headers()
            .get(reqwest::header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                AppError::Other(anyhow::anyhow!(
                    "ISAPI {method} {path}: 401 without a WWW-Authenticate header"
                ))
            })?
            .to_string();
        let auth = super::digest::digest_auth_header(
            method.as_str(),
            path,
            &self.username,
            &self.password,
            &www,
        )
        .ok_or_else(|| {
            AppError::Other(anyhow::anyhow!(
                "ISAPI {method} {path}: unsupported Digest challenge"
            ))
        })?;

        let mut req = self
            .http
            .request(method.clone(), url.as_str())
            .timeout(self.timeout)
            .header(reqwest::header::AUTHORIZATION, auth);
        if let Some(b) = body {
            req = req
                .header(reqwest::header::CONTENT_TYPE, "application/xml")
                .body(b);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| AppError::Other(anyhow::anyhow!("ISAPI {method} {path} failed: {e}")))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Ok((status, text))
    }

    /// As [`Self::isapi_request_raw`] but a non-2xx status becomes an [`AppError`], surfacing the
    /// ISAPI `<statusString>` (or `<errorMsg>`) when present.
    async fn isapi_request(
        &self,
        method: Method,
        path: &str,
        body: Option<String>,
    ) -> AppResult<String> {
        let (status, text) = self.isapi_request_raw(method.clone(), path, body).await?;
        if !status.is_success() {
            let reason = first_text(&text, "statusString")
                .or_else(|| first_text(&text, "errorMsg"))
                .unwrap_or_else(|| format!("HTTP {status}"));
            return Err(AppError::Other(anyhow::anyhow!(
                "ISAPI {method} {path} failed: {reason}"
            )));
        }
        Ok(text)
    }
}

#[async_trait]
impl CameraConfigProvider for HikVisionIsapiClient {
    async fn get_device_info(&self) -> AppResult<DeviceInfo> {
        let xml = self
            .isapi_request(Method::GET, "/ISAPI/System/deviceInfo", None)
            .await?;
        Ok(DeviceInfo {
            device_name: first_text(&xml, "deviceName"),
            model: first_text(&xml, "model"),
            firmware_version: first_text(&xml, "firmwareVersion"),
            serial_number: first_text(&xml, "serialNumber"),
        })
    }

    async fn list_video_configs(&self) -> AppResult<Vec<VideoConfig>> {
        let xml = self
            .isapi_request(Method::GET, "/ISAPI/Streaming/channels", None)
            .await?;
        let configs = elements(&xml, "StreamingChannel")
            .into_iter()
            .filter_map(|(_open, inner)| parse_streaming_channel(inner))
            .collect();
        Ok(configs)
    }

    async fn get_video_config(&self, channel: u32) -> AppResult<VideoConfig> {
        let path = format!("/ISAPI/Streaming/channels/{channel}");
        let xml = self.isapi_request(Method::GET, &path, None).await?;
        parse_streaming_channel(&xml).ok_or_else(|| {
            AppError::Other(anyhow::anyhow!(
                "ISAPI: could not parse StreamingChannel {channel}"
            ))
        })
    }

    async fn put_video_config(&self, channel: u32, cfg: &VideoConfig) -> AppResult<()> {
        let path = format!("/ISAPI/Streaming/channels/{channel}");
        let original = self.isapi_request(Method::GET, &path, None).await?;
        let body = build_video_put_body(&original, cfg)?;
        self.isapi_request(Method::PUT, &path, Some(body)).await?;
        Ok(())
    }

    async fn get_time_config(&self) -> AppResult<TimeConfig> {
        let xml = self
            .isapi_request(Method::GET, "/ISAPI/System/time", None)
            .await?;
        Ok(parse_time(&xml))
    }

    async fn put_time_config(&self, cfg: &TimeConfig) -> AppResult<()> {
        let original = self
            .isapi_request(Method::GET, "/ISAPI/System/time", None)
            .await?;
        let mut body = replace_first_text(&original, "timeMode", &cfg.time_mode);
        body = replace_first_text(&body, "localTime", &cfg.local_time);
        body = replace_first_text(&body, "timeZone", &cfg.time_zone);
        self.isapi_request(Method::PUT, "/ISAPI/System/time", Some(body))
            .await?;
        Ok(())
    }

    async fn get_ntp_config(&self) -> AppResult<NtpConfig> {
        let xml = self
            .isapi_request(Method::GET, "/ISAPI/System/time/ntpServers/1", None)
            .await?;
        Ok(NtpConfig {
            addressing_format: first_text(&xml, "addressingFormatType")
                .unwrap_or_else(|| "hostname".to_string()),
            host_name: first_text(&xml, "hostName")
                .or_else(|| first_text(&xml, "ipAddress"))
                .unwrap_or_default(),
            port: first_text(&xml, "portNo")
                .and_then(|s| s.parse().ok())
                .unwrap_or(123),
        })
    }

    async fn put_ntp_config(&self, cfg: &NtpConfig) -> AppResult<()> {
        let original = self
            .isapi_request(Method::GET, "/ISAPI/System/time/ntpServers/1", None)
            .await?;
        let mut body =
            replace_first_text(&original, "addressingFormatType", &cfg.addressing_format);
        body = replace_first_text(&body, "hostName", &cfg.host_name);
        // Some firmwares carry a separate <ipAddress> element for the `ipaddress` format.
        if cfg.addressing_format.eq_ignore_ascii_case("ipaddress") {
            body = replace_first_text(&body, "ipAddress", &cfg.host_name);
        }
        body = replace_first_text(&body, "portNo", &cfg.port.to_string());
        self.isapi_request(Method::PUT, "/ISAPI/System/time/ntpServers/1", Some(body))
            .await?;
        Ok(())
    }

    async fn sync_time_now(&self) -> AppResult<TimeConfig> {
        let original = self
            .isapi_request(Method::GET, "/ISAPI/System/time", None)
            .await?;
        if first_text(&original, "timeMode")
            .unwrap_or_default()
            .eq_ignore_ascii_case("manual")
        {
            let body = replace_first_text(&original, "timeMode", "NTP");
            self.isapi_request(Method::PUT, "/ISAPI/System/time", Some(body))
                .await?;
            return self.get_time_config().await;
        }
        // Already on NTP (or an unknown mode): report the current clock unchanged.
        Ok(parse_time(&original))
    }

    async fn get_onvif_settings(&self) -> AppResult<OnvifSettings> {
        let xml = self
            .isapi_request(Method::GET, "/ISAPI/System/Network/Integrate", None)
            .await?;
        Ok(OnvifSettings {
            onvif_enabled: first_inner(&xml, "ONVIF")
                .and_then(|b| first_text(b, "enable"))
                .map(|s| parse_bool_text(&s))
                .unwrap_or(false),
            isapi_enabled: first_inner(&xml, "ISAPI")
                .and_then(|b| first_text(b, "enable"))
                .map(|s| parse_bool_text(&s))
                .unwrap_or(false),
        })
    }

    async fn put_onvif_settings(&self, cfg: &OnvifSettings) -> AppResult<()> {
        let original = self
            .isapi_request(Method::GET, "/ISAPI/System/Network/Integrate", None)
            .await?;
        let mut body = replace_in_block(&original, "ONVIF", "enable", bool_text(cfg.onvif_enabled));
        body = replace_in_block(&body, "ISAPI", "enable", bool_text(cfg.isapi_enabled));
        self.isapi_request(Method::PUT, "/ISAPI/System/Network/Integrate", Some(body))
            .await?;
        Ok(())
    }

    async fn ensure_onvif_user(
        &self,
        username: &str,
        password: &str,
        user_type: OnvifUserType,
    ) -> AppResult<()> {
        let xml = self
            .isapi_request(Method::GET, ONVIF_USERS_PATH, None)
            .await?;
        let users = elements(&xml, "User");
        let exists = users
            .iter()
            .any(|&(_open, inner)| first_text(inner, "userName").as_deref() == Some(username));
        if exists {
            return Ok(());
        }
        // Allocate the next id (max existing + 1) for the new user.
        let next_id = users
            .iter()
            .filter_map(|&(_open, inner)| {
                first_text(inner, "id").and_then(|s| s.parse::<i64>().ok())
            })
            .max()
            .unwrap_or(0)
            + 1;
        let body = format!(
            "<UserList version=\"2.0\" xmlns=\"{HIK_NS}\">\
<User><id>{id}</id><userName>{user}</userName><password>{pass}</password>\
<userType>{utype}</userType></User></UserList>",
            id = next_id,
            user = xml_escape(username),
            pass = xml_escape(password),
            utype = onvif_user_type_wire(user_type),
        );
        // POST creates the user; the device returns a 4xx if the user already exists — treat any 4xx
        // on create as success (only a 5xx / transport failure is a real error).
        let (status, text) = self
            .isapi_request_raw(Method::POST, ONVIF_USERS_PATH, Some(body))
            .await?;
        if status.is_success() || status.is_client_error() {
            Ok(())
        } else {
            let reason =
                first_text(&text, "statusString").unwrap_or_else(|| format!("HTTP {status}"));
            Err(AppError::Other(anyhow::anyhow!(
                "ISAPI POST {ONVIF_USERS_PATH} failed: {reason}"
            )))
        }
    }

    async fn get_osd_config(&self) -> AppResult<OsdConfig> {
        let xml = self.isapi_request(Method::GET, OSD_PATH, None).await?;
        let dt = first_inner(&xml, "DateTimeOverlay").unwrap_or("");
        let cn = first_inner(&xml, "channelNameOverlay").unwrap_or("");
        Ok(OsdConfig {
            datetime_enabled: first_text(dt, "enabled")
                .map(|s| parse_bool_text(&s))
                .unwrap_or(false),
            channel_name_enabled: first_text(cn, "enabled")
                .map(|s| parse_bool_text(&s))
                .unwrap_or(false),
            date_style: first_text(dt, "dateStyle"),
            time_style: first_text(dt, "timeStyle"),
            display_week: first_text(dt, "displayWeek").map(|s| parse_bool_text(&s)),
        })
    }

    async fn put_osd_config(&self, cfg: &OsdConfig) -> AppResult<()> {
        let original = self.isapi_request(Method::GET, OSD_PATH, None).await?;
        let mut body = replace_in_block(
            &original,
            "DateTimeOverlay",
            "enabled",
            bool_text(cfg.datetime_enabled),
        );
        body = replace_in_block(
            &body,
            "channelNameOverlay",
            "enabled",
            bool_text(cfg.channel_name_enabled),
        );
        if let Some(ds) = &cfg.date_style {
            body = replace_in_block(&body, "DateTimeOverlay", "dateStyle", ds);
        }
        if let Some(ts) = &cfg.time_style {
            body = replace_in_block(&body, "DateTimeOverlay", "timeStyle", ts);
        }
        if let Some(dw) = cfg.display_week {
            body = replace_in_block(&body, "DateTimeOverlay", "displayWeek", bool_text(dw));
        }
        self.isapi_request(Method::PUT, OSD_PATH, Some(body))
            .await?;
        Ok(())
    }

    async fn reboot(&self) -> AppResult<()> {
        self.isapi_request(Method::PUT, "/ISAPI/System/reboot", None)
            .await?;
        Ok(())
    }

    // ---- Device-control surfaces ----

    async fn get_day_night(&self) -> AppResult<DayNightConfig> {
        let xml = self.isapi_request(Method::GET, IRCUT_PATH, None).await?;
        Ok(parse_day_night(&xml))
    }

    async fn put_day_night(&self, patch: &DayNightPatch) -> AppResult<()> {
        let original = self.isapi_request(Method::GET, IRCUT_PATH, None).await?;
        let mut body = original;
        if let Some(mode) = &patch.mode {
            body = replace_first_text(&body, "IrcutFilterType", mode);
        }
        if let Some(s) = patch.sensitivity {
            body = replace_first_text(&body, "nightToDayFilterLevel", &s.to_string());
        }
        self.isapi_request(Method::PUT, IRCUT_PATH, Some(body))
            .await?;
        Ok(())
    }

    async fn get_image_config(&self) -> AppResult<ImageConfig> {
        // Aggregate the sub-resources a firmware may or may not expose. `color` is the baseline
        // (any camera with the Image service has it); WDR/BLC/supplementLight are best-effort.
        let color = self
            .isapi_request(Method::GET, "/ISAPI/Image/channels/1/color", None)
            .await?;
        let mut cfg = ImageConfig {
            brightness: first_text(&color, "brightnessLevel").and_then(|s| s.parse().ok()),
            contrast: first_text(&color, "contrastLevel").and_then(|s| s.parse().ok()),
            saturation: first_text(&color, "saturationLevel").and_then(|s| s.parse().ok()),
            ..ImageConfig::default()
        };
        if let Ok((status, wdr)) = self
            .isapi_request_raw(Method::GET, "/ISAPI/Image/channels/1/WDR", None)
            .await
        {
            if status.is_success() {
                cfg.wdr_mode = first_text(&wdr, "mode");
                cfg.wdr_level = first_text(&wdr, "WDRLevel").and_then(|s| s.parse().ok());
            }
        }
        if let Ok((status, blc)) = self
            .isapi_request_raw(Method::GET, "/ISAPI/Image/channels/1/BLC", None)
            .await
        {
            if status.is_success() {
                cfg.blc_enabled = first_text(&blc, "enabled").map(|s| parse_bool_text(&s));
            }
        }
        if let Ok((status, sl)) = self
            .isapi_request_raw(Method::GET, "/ISAPI/Image/channels/1/supplementLight", None)
            .await
        {
            if status.is_success() {
                cfg.supplement_light_mode = first_text(&sl, "supplementLightMode");
                cfg.supplement_brightness_mode = first_text(&sl, "mixedLightBrightnessRegulatMode");
                cfg.white_light_brightness =
                    first_text(&sl, "whiteLightBrightness").and_then(|s| s.parse().ok());
                cfg.ir_light_brightness =
                    first_text(&sl, "irLightBrightness").and_then(|s| s.parse().ok());
            }
        }
        Ok(cfg)
    }

    async fn put_image_config(&self, patch: &ImageConfigPatch) -> AppResult<()> {
        // Each sub-resource is read-modify-written only when the patch touches it, so a device
        // without (say) WDR is never sent a WDR write.
        if patch.brightness.is_some() || patch.contrast.is_some() || patch.saturation.is_some() {
            let path = "/ISAPI/Image/channels/1/color";
            let mut body = self.isapi_request(Method::GET, path, None).await?;
            if let Some(v) = patch.brightness {
                body = replace_first_text(&body, "brightnessLevel", &clamp_pct(v).to_string());
            }
            if let Some(v) = patch.contrast {
                body = replace_first_text(&body, "contrastLevel", &clamp_pct(v).to_string());
            }
            if let Some(v) = patch.saturation {
                body = replace_first_text(&body, "saturationLevel", &clamp_pct(v).to_string());
            }
            self.isapi_request(Method::PUT, path, Some(body)).await?;
        }
        if patch.wdr_mode.is_some() || patch.wdr_level.is_some() {
            let path = "/ISAPI/Image/channels/1/WDR";
            let mut body = self.isapi_request(Method::GET, path, None).await?;
            if let Some(m) = &patch.wdr_mode {
                body = replace_first_text(&body, "mode", m);
            }
            if let Some(v) = patch.wdr_level {
                body = replace_first_text(&body, "WDRLevel", &clamp_pct(v).to_string());
            }
            self.isapi_request(Method::PUT, path, Some(body)).await?;
        }
        if let Some(enabled) = patch.blc_enabled {
            let path = "/ISAPI/Image/channels/1/BLC";
            let body = self.isapi_request(Method::GET, path, None).await?;
            let body = replace_first_text(&body, "enabled", bool_text(enabled));
            self.isapi_request(Method::PUT, path, Some(body)).await?;
        }
        if patch.supplement_light_mode.is_some()
            || patch.supplement_brightness_mode.is_some()
            || patch.white_light_brightness.is_some()
            || patch.ir_light_brightness.is_some()
        {
            let path = "/ISAPI/Image/channels/1/supplementLight";
            let mut body = self.isapi_request(Method::GET, path, None).await?;
            if let Some(mode) = &patch.supplement_light_mode {
                body = replace_first_text(&body, "supplementLightMode", mode);
            }
            if let Some(m) = &patch.supplement_brightness_mode {
                body = replace_first_text(&body, "mixedLightBrightnessRegulatMode", m);
            }
            if let Some(v) = patch.white_light_brightness {
                body = replace_first_text(&body, "whiteLightBrightness", &clamp_pct(v).to_string());
            }
            if let Some(v) = patch.ir_light_brightness {
                body = replace_first_text(&body, "irLightBrightness", &clamp_pct(v).to_string());
            }
            self.isapi_request(Method::PUT, path, Some(body)).await?;
        }
        Ok(())
    }

    async fn supplement_light_modes(&self) -> AppResult<Vec<String>> {
        let (status, xml) = self
            .isapi_request_raw(
                Method::GET,
                "/ISAPI/Image/channels/1/supplementLight/capabilities",
                None,
            )
            .await?;
        if !status.is_success() {
            return Ok(Vec::new()); // no supplement light on this device
        }
        Ok(parse_supplement_light_modes(&xml))
    }

    async fn list_builtin_detections(&self) -> AppResult<Vec<BuiltinDetection>> {
        let mut out = Vec::new();
        // Basic motion detection lives outside the Smart service; presence of the endpoint = support.
        if let Ok((status, xml)) = self
            .isapi_request_raw(
                Method::GET,
                "/ISAPI/System/Video/inputs/channels/1/motionDetection",
                None,
            )
            .await
        {
            if status.is_success() {
                out.push(BuiltinDetection {
                    kind: "motion".into(),
                    enabled: first_text(&xml, "enabled").map(|s| parse_bool_text(&s)),
                });
            }
        }
        // Smart-event support flags. For the two common zone-style events we also read the arm
        // state from their config resource; the rest are reported support-only.
        if let Ok((status, cap)) = self
            .isapi_request_raw(Method::GET, "/ISAPI/Smart/capabilities", None)
            .await
        {
            if status.is_success() {
                for (flag, kind, state_path) in SMART_DETECTIONS {
                    let supported = first_text(&cap, flag)
                        .map(|s| parse_bool_text(&s))
                        .unwrap_or(false);
                    if !supported {
                        continue;
                    }
                    let mut enabled = None;
                    if let Some(path) = state_path {
                        if let Ok((st, xml)) = self.isapi_request_raw(Method::GET, path, None).await
                        {
                            if st.is_success() {
                                enabled = first_text(&xml, "enabled").map(|s| parse_bool_text(&s));
                            }
                        }
                    }
                    out.push(BuiltinDetection {
                        kind: (*kind).into(),
                        enabled,
                    });
                }
            }
        }
        Ok(out)
    }

    async fn set_builtin_detection(&self, kind: &str, enabled: bool) -> AppResult<()> {
        let path = builtin_detection_path(kind).ok_or_else(|| {
            AppError::BadRequest(format!(
                "built-in detection `{kind}` cannot be armed/disarmed on this device"
            ))
        })?;
        let body = self.isapi_request(Method::GET, path, None).await?;
        let body = replace_first_text(&body, "enabled", bool_text(enabled));
        self.isapi_request(Method::PUT, path, Some(body)).await?;
        Ok(())
    }

    async fn open_event_stream(
        &self,
        stream_http: &reqwest::Client,
    ) -> AppResult<reqwest::Response> {
        // The digest dance by hand: the challenge leg is bounded, but the authorized leg must NOT
        // carry a request timeout — the response is an endless multipart stream (the caller owns
        // an idle watchdog per chunk). `stream_http` must be a client with no total timeout.
        let url = format!("{}{}", self.base_url, ALERT_STREAM_PATH);
        let resp = stream_http
            .get(&url)
            .timeout(self.timeout)
            .send()
            .await
            .map_err(|e| AppError::Other(anyhow::anyhow!("alertStream probe failed: {e}")))?;
        if resp.status() != StatusCode::UNAUTHORIZED {
            if resp.status().is_success() {
                return Ok(resp); // anonymous stream (auth disabled on the device)
            }
            return Err(AppError::Other(anyhow::anyhow!(
                "alertStream refused: HTTP {}",
                resp.status()
            )));
        }
        let www = resp
            .headers()
            .get(reqwest::header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                AppError::Other(anyhow::anyhow!("alertStream: 401 without WWW-Authenticate"))
            })?
            .to_string();
        let auth = super::digest::digest_auth_header(
            "GET",
            ALERT_STREAM_PATH,
            &self.username,
            &self.password,
            &www,
        )
        .ok_or_else(|| {
            AppError::Other(anyhow::anyhow!("alertStream: unsupported Digest challenge"))
        })?;
        let resp = stream_http
            .get(&url)
            .header(reqwest::header::AUTHORIZATION, auth)
            .send()
            .await
            .map_err(|e| AppError::Other(anyhow::anyhow!("alertStream connect failed: {e}")))?;
        if !resp.status().is_success() {
            return Err(AppError::Other(anyhow::anyhow!(
                "alertStream auth failed: HTTP {}",
                resp.status()
            )));
        }
        Ok(resp)
    }

    async fn list_io_outputs(&self) -> AppResult<Vec<IoOutput>> {
        let xml = self
            .isapi_request(Method::GET, IO_OUTPUTS_PATH, None)
            .await?;
        Ok(parse_io_outputs(&xml))
    }

    async fn set_io_output(&self, port: i64, active: bool) -> AppResult<()> {
        if port < 1 {
            return Err(AppError::BadRequest("output port must be >= 1".into()));
        }
        let state = if active { "high" } else { "low" };
        let body = format!(
            "<IOPortData version=\"2.0\" xmlns=\"{HIK_NS}\"><outputState>{state}</outputState></IOPortData>"
        );
        let path = format!("{IO_OUTPUTS_PATH}/{port}/trigger");
        self.isapi_request(Method::PUT, &path, Some(body)).await?;
        Ok(())
    }

    async fn supports_native_anpr(&self) -> bool {
        // Probe the plate-results endpoint with an empty-window query: any 2xx means the on-board
        // ANPR engine exists (a non-ANPR camera answers 4xx/notSupported).
        let body = anpr_query_body("");
        matches!(
            self.isapi_request_raw(Method::POST, ANPR_PLATES_PATH, Some(body)).await,
            Ok((status, _)) if status.is_success()
        )
    }

    async fn fetch_anpr_plates(&self, after: &str) -> AppResult<Vec<NativePlateRead>> {
        let xml = self
            .isapi_request(Method::POST, ANPR_PLATES_PATH, Some(anpr_query_body(after)))
            .await?;
        Ok(parse_anpr_plates(&xml))
    }
}

// ========================= ISAPI body parsing / building =========================

/// Parse a `<StreamingChannel>` element (the slice may be the element's inner XML or any XML that
/// contains it) into a [`VideoConfig`]. Returns `None` when the channel id is missing/unparseable.
fn parse_streaming_channel(xml: &str) -> Option<VideoConfig> {
    let channel_id: i64 = first_text(xml, "id")?.parse().ok()?;
    let channel_name = first_text(xml, "channelName");
    let video = first_inner(xml, "Video")?;
    Some(VideoConfig {
        channel_id,
        channel_name,
        codec: first_text(video, "videoCodecType").unwrap_or_default(),
        width: parse_i64(video, "videoResolutionWidth"),
        height: parse_i64(video, "videoResolutionHeight"),
        fps: parse_i64(video, "maxFrameRate"),
        quality_control: first_text(video, "videoQualityControlType").unwrap_or_default(),
        bitrate: parse_i64(video, "constantBitRate"),
        vbr_upper_cap: parse_i64(video, "vbrUpperCap"),
        gop: parse_i64(video, "GovLength"),
    })
}

/// Read-modify-write the `<Video>` block of a `StreamingChannel` XML document, preserving the id,
/// channel name, namespace, and every untouched sub-element.
fn build_video_put_body(original: &str, cfg: &VideoConfig) -> AppResult<String> {
    let (_lt, gt, self_closing) = find_open(original, "Video", 0).ok_or_else(|| {
        AppError::Other(anyhow::anyhow!(
            "ISAPI: StreamingChannel has no <Video> block"
        ))
    })?;
    if self_closing {
        return Err(AppError::Other(anyhow::anyhow!(
            "ISAPI: StreamingChannel <Video> block is empty"
        )));
    }
    let cs = gt + 1;
    let close_rel = find_close(&original[cs..], "Video")
        .ok_or_else(|| AppError::Other(anyhow::anyhow!("ISAPI: unterminated <Video> block")))?;
    let ce = cs + close_rel;

    let mut v = replace_first_text(&original[cs..ce], "videoCodecType", &cfg.codec);
    v = replace_first_text(&v, "videoResolutionWidth", &cfg.width.to_string());
    v = replace_first_text(&v, "videoResolutionHeight", &cfg.height.to_string());
    v = replace_first_text(&v, "videoQualityControlType", &cfg.quality_control);
    v = replace_first_text(&v, "constantBitRate", &cfg.bitrate.to_string());
    v = replace_first_text(&v, "vbrUpperCap", &cfg.vbr_upper_cap.to_string());
    v = replace_first_text(&v, "maxFrameRate", &cfg.fps.to_string());
    v = replace_first_text(&v, "GovLength", &cfg.gop.to_string());

    let mut out = String::with_capacity(original.len() + v.len());
    out.push_str(&original[..cs]);
    out.push_str(&v);
    out.push_str(&original[ce..]);
    Ok(out)
}

/// The config resource whose `<enabled>` arms/disarms a built-in detection kind, for the kinds
/// that carry a per-feature resource (the same three the capability probe reads state from).
fn builtin_detection_path(kind: &str) -> Option<&'static str> {
    match kind {
        "motion" => Some("/ISAPI/System/Video/inputs/channels/1/motionDetection"),
        "line_crossing" => Some("/ISAPI/Smart/LineDetection/1"),
        "intrusion" => Some("/ISAPI/Smart/FieldDetection/1"),
        _ => None,
    }
}

/// Parse an `<IrcutFilter>` document into a [`DayNightConfig`].
fn parse_day_night(xml: &str) -> DayNightConfig {
    DayNightConfig {
        mode: first_text(xml, "IrcutFilterType").unwrap_or_else(|| "auto".into()),
        sensitivity: first_text(xml, "nightToDayFilterLevel").and_then(|s| s.parse().ok()),
    }
}

/// Parse an `<IOOutputPortList>` document into the output ports.
fn parse_io_outputs(xml: &str) -> Vec<IoOutput> {
    elements(xml, "IOOutputPort")
        .into_iter()
        .filter_map(|(_open, inner)| {
            let id: i64 = first_text(inner, "id")?.parse().ok()?;
            Some(IoOutput {
                id,
                name: first_text(inner, "name"),
                default_state: first_inner(inner, "PowerOnState")
                    .and_then(|b| first_text(b, "defaultState"))
                    .or_else(|| first_text(inner, "defaultState")),
            })
        })
        .collect()
}

/// Build the `<AfterTime>` query body for the on-board ANPR plate-results endpoint. An empty
/// cursor asks from the epoch of the device's buffer (a capability probe / first poll).
fn anpr_query_body(after: &str) -> String {
    let pic_time = if after.trim().is_empty() {
        "20000101000000000".to_string()
    } else {
        xml_escape(after.trim())
    };
    format!(
        "<AfterTime version=\"2.0\" xmlns=\"{HIK_NS}\"><picTime>{pic_time}</picTime></AfterTime>"
    )
}

/// Parse a `<Plates>` response into plate reads. Reads without a plate number are dropped.
fn parse_anpr_plates(xml: &str) -> Vec<NativePlateRead> {
    elements(xml, "Plate")
        .into_iter()
        .filter_map(|(_open, inner)| {
            let plate = first_text(inner, "plateNumber")?;
            Some(NativePlateRead {
                plate,
                capture_time: first_text(inner, "captureTime").unwrap_or_default(),
                direction: first_text(inner, "direction"),
                pic_name: first_text(inner, "picName"),
                country: first_text(inner, "country"),
            })
        })
        .collect()
}

/// Clamp an image level to the 0–100 range ISAPI expects.
fn clamp_pct(v: i64) -> i64 {
    v.clamp(0, 100)
}

/// Parse the supported supplement-light modes from a capability document: the `opt` attribute of
/// `<supplementLightMode opt="eventIntelligence,colorVuWhiteLight,irLight,close">` (verified live
/// on DS-2CD3T56WDV3-L; an IR-only DS-2CD3356WDV3-I reports `opt="irLight,close"`).
fn parse_supplement_light_modes(xml: &str) -> Vec<String> {
    let Some((lt, gt, _)) = find_open(xml, "supplementLightMode", 0) else {
        return Vec::new();
    };
    let open_tag = &xml[lt..=gt];
    attr_in_tag(open_tag, "opt")
        .map(|opts| {
            opts.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Extract the value of attribute `name` from an opening tag string
/// (e.g. `<supplementLightMode opt="irLight,close">`). Mirrors `services/onvif.rs`.
fn attr_in_tag(tag: &str, name: &str) -> Option<String> {
    let bytes = tag.as_bytes();
    let mut i = 0;
    while let Some(rel) = tag[i..].find(name) {
        let pos = i + rel;
        let before_ok = pos == 0
            || matches!(bytes.get(pos - 1), Some(b) if b.is_ascii_whitespace() || *b == b'<');
        let after = &tag[pos + name.len()..];
        let after_trim = after.trim_start();
        if before_ok && after_trim.starts_with('=') {
            let rest = after_trim[1..].trim_start();
            let quote = rest.chars().next()?;
            if quote == '"' || quote == '\'' {
                let val = &rest[1..];
                let end = val.find(quote)?;
                return Some(xml_unescape(&val[..end]));
            }
        }
        i = pos + name.len();
    }
    None
}

/// Parse a `<Time>` document into a [`TimeConfig`].
fn parse_time(xml: &str) -> TimeConfig {
    TimeConfig {
        time_mode: first_text(xml, "timeMode").unwrap_or_default(),
        local_time: first_text(xml, "localTime").unwrap_or_default(),
        time_zone: first_text(xml, "timeZone").unwrap_or_default(),
    }
}

/// Parse the integer text of the first `<local>` element, or `0`.
fn parse_i64(xml: &str, local: &str) -> i64 {
    first_text(xml, local)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Map an [`OnvifUserType`] to its verbatim ISAPI `userType` value.
fn onvif_user_type_wire(t: OnvifUserType) -> &'static str {
    match t {
        OnvifUserType::Administrator => "administrator",
        OnvifUserType::Operator => "operator",
        OnvifUserType::MediaUser => "mediaUser",
    }
}

/// ISAPI boolean text.
fn bool_text(b: bool) -> &'static str {
    if b {
        "true"
    } else {
        "false"
    }
}

/// Interpret ISAPI boolean text (`true`/`1`/`yes`, case-insensitive).
pub(crate) fn parse_bool_text(s: &str) -> bool {
    matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes")
}

/// Replace the inner text of the FIRST `<local>...</local>` element with `new_value` (XML-escaped).
/// A self-closing or absent element leaves `xml` unchanged (read-modify-write never adds elements).
fn replace_first_text(xml: &str, local: &str, new_value: &str) -> String {
    let Some((_lt, gt, self_closing)) = find_open(xml, local, 0) else {
        return xml.to_string();
    };
    if self_closing {
        return xml.to_string();
    }
    let cs = gt + 1;
    let Some(close_rel) = find_close(&xml[cs..], local) else {
        return xml.to_string();
    };
    let ce = cs + close_rel;
    let escaped = xml_escape(new_value);
    let mut out = String::with_capacity(xml.len() + escaped.len());
    out.push_str(&xml[..cs]);
    out.push_str(&escaped);
    out.push_str(&xml[ce..]);
    out
}

/// Replace the inner text of the first `<local>` element found INSIDE the first `<block>` element,
/// so a name that repeats across sibling blocks (e.g. `<enable>` under both `<ONVIF>` and `<ISAPI>`)
/// is disambiguated. Leaves `xml` unchanged when the block is absent/self-closing.
fn replace_in_block(xml: &str, block: &str, local: &str, new_value: &str) -> String {
    let Some((_lt, gt, self_closing)) = find_open(xml, block, 0) else {
        return xml.to_string();
    };
    if self_closing {
        return xml.to_string();
    }
    let cs = gt + 1;
    let Some(close_rel) = find_close(&xml[cs..], block) else {
        return xml.to_string();
    };
    let ce = cs + close_rel;
    let modified = replace_first_text(&xml[cs..ce], local, new_value);
    let mut out = String::with_capacity(xml.len() + modified.len());
    out.push_str(&xml[..cs]);
    out.push_str(&modified);
    out.push_str(&xml[ce..]);
    out
}

// ========================= XML helpers (substring extraction) =========================
//
// Copied from `services/onvif.rs`: these tolerate namespace prefixes and attributes on tags and
// assume the small, well-formed XML bodies ISAPI returns (no same-name nesting in what we read).

/// Locate the first element with local name `local` at/after byte `from`. Returns
/// `(open_lt, open_gt, self_closing)`: index of the opening `<`, index of that tag's `>`, and whether
/// the element is self-closing (`/>`). Comments, declarations, and closing tags are skipped.
pub(crate) fn find_open(xml: &str, local: &str, from: usize) -> Option<(usize, usize, bool)> {
    let bytes = xml.as_bytes();
    let mut i = from.min(xml.len());
    while let Some(rel) = xml[i..].find('<') {
        let lt = i + rel;
        match bytes.get(lt + 1).copied() {
            Some(b'/') | Some(b'!') | Some(b'?') => {
                i = lt + 1;
                continue;
            }
            _ => {}
        }
        let name_start = lt + 1;
        let gt_rel = xml[name_start..].find('>')?;
        let gt = name_start + gt_rel;
        let self_closing = gt > name_start && bytes.get(gt - 1).copied() == Some(b'/');
        let tag = &xml[name_start..gt];
        let head = tag.split([' ', '\t', '\n', '\r', '/']).next().unwrap_or("");
        let local_name = head.rsplit(':').next().unwrap_or(head);
        if local_name == local {
            return Some((lt, gt, self_closing));
        }
        i = gt + 1;
    }
    None
}

/// Find the byte offset of the first closing tag `</...local>` in `xml`.
pub(crate) fn find_close(xml: &str, local: &str) -> Option<usize> {
    let mut i = 0;
    while let Some(rel) = xml[i..].find("</") {
        let pos = i + rel;
        let after = &xml[pos + 2..];
        let gt_rel = after.find('>')?;
        let name = after[..gt_rel].trim();
        let local_name = name.rsplit(':').next().unwrap_or(name);
        if local_name == local {
            return Some(pos);
        }
        i = pos + 2;
    }
    None
}

/// Inner XML (raw) of the first element with local name `local`.
pub(crate) fn first_inner<'a>(xml: &'a str, local: &str) -> Option<&'a str> {
    let (_lt, gt, self_closing) = find_open(xml, local, 0)?;
    if self_closing {
        return Some("");
    }
    let cs = gt + 1;
    let close_rel = find_close(&xml[cs..], local)?;
    Some(&xml[cs..cs + close_rel])
}

/// Trimmed, entity-decoded text content of the first element with local name `local`. Returns `None`
/// when the element is absent or its text is empty.
pub(crate) fn first_text(xml: &str, local: &str) -> Option<String> {
    let inner = first_inner(xml, local)?;
    let t = inner.trim();
    if t.is_empty() {
        None
    } else {
        Some(xml_unescape(t))
    }
}

/// All elements with local name `local`, returned as `(opening_tag, inner_xml)` pairs.
fn elements<'a>(xml: &'a str, local: &str) -> Vec<(&'a str, &'a str)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some((lt, gt, self_closing)) = find_open(xml, local, from) {
        let open = &xml[lt..=gt];
        if self_closing {
            out.push((open, ""));
            from = gt + 1;
            continue;
        }
        let cs = gt + 1;
        match find_close(&xml[cs..], local) {
            Some(close_rel) => {
                out.push((open, &xml[cs..cs + close_rel]));
                from = cs + close_rel;
            }
            None => break,
        }
    }
    out
}

/// Decode the five predefined XML entities.
pub(crate) fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Escape the characters that are not safe in XML text / attribute values.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHANNEL: &str = "<StreamingChannel version=\"2.0\" xmlns=\"http://www.hikvision.com/ver20/XMLSchema\">\
<id>101</id><channelName>Front Door</channelName><enabled>true</enabled>\
<Video><enabled>true</enabled><videoInputChannelID>1</videoInputChannelID>\
<videoCodecType>H.265</videoCodecType><videoResolutionWidth>2560</videoResolutionWidth>\
<videoResolutionHeight>1440</videoResolutionHeight><videoQualityControlType>VBR</videoQualityControlType>\
<constantBitRate>4096</constantBitRate><vbrUpperCap>4096</vbrUpperCap>\
<maxFrameRate>2000</maxFrameRate><GovLength>50</GovLength></Video></StreamingChannel>";

    #[test]
    fn parses_streaming_channel() {
        let c = parse_streaming_channel(CHANNEL).expect("parsed");
        assert_eq!(c.channel_id, 101);
        assert_eq!(c.channel_name.as_deref(), Some("Front Door"));
        assert_eq!(c.codec, "H.265");
        assert_eq!(c.width, 2560);
        assert_eq!(c.height, 1440);
        assert_eq!(c.fps, 2000);
        assert_eq!(c.quality_control, "VBR");
        assert_eq!(c.bitrate, 4096);
        assert_eq!(c.vbr_upper_cap, 4096);
        assert_eq!(c.gop, 50);
    }

    #[test]
    fn read_modify_write_preserves_untouched_fields() {
        let cfg = VideoConfig {
            channel_id: 101,
            channel_name: Some("ignored".into()),
            codec: "H.264".into(),
            width: 1920,
            height: 1080,
            fps: 2500,
            quality_control: "CBR".into(),
            bitrate: 2048,
            vbr_upper_cap: 2048,
            gop: 25,
        };
        let body = build_video_put_body(CHANNEL, &cfg).expect("built");
        // Changed fields.
        assert!(body.contains("<videoCodecType>H.264</videoCodecType>"));
        assert!(body.contains("<videoResolutionWidth>1920</videoResolutionWidth>"));
        assert!(body.contains("<maxFrameRate>2500</maxFrameRate>"));
        assert!(body.contains("<videoQualityControlType>CBR</videoQualityControlType>"));
        assert!(body.contains("<GovLength>25</GovLength>"));
        // Preserved id / channel name / namespace / untouched sub-elements.
        assert!(body.contains("<id>101</id>"));
        assert!(body.contains("<channelName>Front Door</channelName>"));
        assert!(body.contains("xmlns=\"http://www.hikvision.com/ver20/XMLSchema\""));
        assert!(body.contains("<videoInputChannelID>1</videoInputChannelID>"));
    }

    #[test]
    fn replace_in_block_disambiguates_repeated_names() {
        let xml = "<Integrate><ONVIF><enable>false</enable></ONVIF>\
<ISAPI><enable>true</enable></ISAPI></Integrate>";
        let out = replace_in_block(xml, "ONVIF", "enable", "true");
        assert_eq!(
            out,
            "<Integrate><ONVIF><enable>true</enable></ONVIF>\
<ISAPI><enable>true</enable></ISAPI></Integrate>"
        );
        // The ISAPI <enable> is untouched.
        let out2 = replace_in_block(&out, "ISAPI", "enable", "false");
        assert!(out2.contains("<ONVIF><enable>true</enable></ONVIF>"));
        assert!(out2.contains("<ISAPI><enable>false</enable></ISAPI>"));
    }

    #[test]
    fn parses_onvif_user_list() {
        let xml = "<UserList version=\"2.0\" xmlns=\"http://www.hikvision.com/ver20/XMLSchema\">\
<User><id>1</id><userName>admin</userName><userType>administrator</userType></User>\
<User><id>2</id><userName>heldar_onvif</userName><userType>operator</userType></User>\
</UserList>";
        let users = elements(xml, "User");
        assert_eq!(users.len(), 2);
        let names: Vec<_> = users
            .iter()
            .filter_map(|&(_o, inner)| first_text(inner, "userName"))
            .collect();
        assert_eq!(names, vec!["admin", "heldar_onvif"]);
        let max_id = users
            .iter()
            .filter_map(|&(_o, inner)| first_text(inner, "id").and_then(|s| s.parse::<i64>().ok()))
            .max();
        assert_eq!(max_id, Some(2));
    }

    #[test]
    fn replace_first_text_escapes_and_no_ops_when_absent() {
        let xml = "<NTPServer><hostName>old</hostName></NTPServer>";
        assert_eq!(
            replace_first_text(xml, "hostName", "a&b"),
            "<NTPServer><hostName>a&amp;b</hostName></NTPServer>"
        );
        // Absent element -> unchanged.
        assert_eq!(replace_first_text(xml, "portNo", "123"), xml);
    }

    #[test]
    fn parses_day_night() {
        let xml =
            "<IrcutFilter version=\"2.0\" xmlns=\"http://www.hikvision.com/ver20/XMLSchema\">\
<IrcutFilterType>auto</IrcutFilterType><nightToDayFilterLevel>4</nightToDayFilterLevel>\
<nightToDayFilterTime>5</nightToDayFilterTime></IrcutFilter>";
        let c = parse_day_night(xml);
        assert_eq!(c.mode, "auto");
        assert_eq!(c.sensitivity, Some(4));
    }

    #[test]
    fn parses_io_outputs() {
        let xml = "<IOOutputPortList version=\"2.0\" xmlns=\"http://www.hikvision.com/ver20/XMLSchema\">\
<IOOutputPort><id>1</id><name>Gate relay</name><PowerOnState><defaultState>low</defaultState></PowerOnState></IOOutputPort>\
<IOOutputPort><id>2</id></IOOutputPort></IOOutputPortList>";
        let ports = parse_io_outputs(xml);
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].id, 1);
        assert_eq!(ports[0].name.as_deref(), Some("Gate relay"));
        assert_eq!(ports[0].default_state.as_deref(), Some("low"));
        assert_eq!(ports[1].id, 2);
        assert_eq!(ports[1].name, None);
    }

    #[test]
    fn parses_anpr_plates_and_skips_plateless() {
        let xml = "<Plates version=\"2.0\" xmlns=\"http://www.hikvision.com/ver20/XMLSchema\">\
<Plate><captureTime>20260716093012456</captureTime><plateNumber>WXY8888</plateNumber>\
<direction>forward</direction><picName>202607160930124560001</picName><country>MYS</country></Plate>\
<Plate><captureTime>20260716093013000</captureTime></Plate></Plates>";
        let plates = parse_anpr_plates(xml);
        assert_eq!(plates.len(), 1, "the plateless read is dropped");
        assert_eq!(plates[0].plate, "WXY8888");
        assert_eq!(plates[0].capture_time, "20260716093012456");
        assert_eq!(plates[0].direction.as_deref(), Some("forward"));
        assert_eq!(plates[0].pic_name.as_deref(), Some("202607160930124560001"));
    }

    /// Verbatim capability document from a live DS-2CD3T56WDV3-L (hybrid white-light model).
    #[test]
    fn parses_supplement_light_modes_hybrid() {
        let xml = "<SupplementLight>\
<supplementLightMode opt=\"eventIntelligence,colorVuWhiteLight,irLight,close\">irLight</supplementLightMode>\
<mixedLightBrightnessRegulatMode opt=\"auto,manual\">auto</mixedLightBrightnessRegulatMode>\
<whiteLightBrightness min=\"0\" max=\"100\">100</whiteLightBrightness>\
</SupplementLight>";
        assert_eq!(
            parse_supplement_light_modes(xml),
            vec!["eventIntelligence", "colorVuWhiteLight", "irLight", "close"]
        );
    }

    /// Verbatim from a live DS-2CD3356WDV3-I (IR-only model): no white-light modes.
    #[test]
    fn parses_supplement_light_modes_ir_only() {
        let xml =
            "<SupplementLight version=\"2.0\" xmlns=\"http://www.hikvision.com/ver20/XMLSchema\">\
<supplementLightMode opt=\"irLight,close\">irLight</supplementLightMode>\
<irLightBrightness min=\"0\" max=\"100\">100</irLightBrightness>\
</SupplementLight>";
        assert_eq!(parse_supplement_light_modes(xml), vec!["irLight", "close"]);
        // No supplementLightMode element at all -> empty (device without a supplement light).
        assert!(parse_supplement_light_modes("<SupplementLight/>").is_empty());
    }

    /// Verbatim `/ISAPI/Smart/capabilities` from a live DS-2CD3T56WDV3-L: the support-flag table
    /// must select exactly line_crossing + intrusion (face/loitering/etc. are false).
    #[test]
    fn smart_cap_flags_select_supported_detections() {
        let xml = "<SmartCap version=\"2.0\" xmlns=\"http://www.hikvision.com/ver20/XMLSchema\">\
<isSupportROI>true</isSupportROI><isSupportFaceDetect>false</isSupportFaceDetect>\
<isSupportDefocusDetection>false</isSupportDefocusDetection><isSupportAudioDetection>false</isSupportAudioDetection>\
<isSupportSceneChangeDetection>false</isSupportSceneChangeDetection><isSupportIntelliTrace>false</isSupportIntelliTrace>\
<isSupportFieldDetection>true</isSupportFieldDetection><isSupportLineDetection>true</isSupportLineDetection>\
<isSupportRegionEntrance>false</isSupportRegionEntrance><isSupportRegionExiting>false</isSupportRegionExiting>\
<isSupportLoitering>false</isSupportLoitering><isSupportGroup>false</isSupportGroup>\
<isSupportRapidMove>false</isSupportRapidMove><isSupportParking>false</isSupportParking>\
<isSupportUnattendedBaggage>false</isSupportUnattendedBaggage><isSupportAttendedBaggage>false</isSupportAttendedBaggage>\
<isSupportSmartCalibration>true</isSupportSmartCalibration><isSupportStorageDetection>false</isSupportStorageDetection>\
</SmartCap>";
        let supported: Vec<&str> = SMART_DETECTIONS
            .iter()
            .filter(|(flag, _, _)| {
                first_text(xml, flag)
                    .map(|s| parse_bool_text(&s))
                    .unwrap_or(false)
            })
            .map(|(_, kind, _)| *kind)
            .collect();
        assert_eq!(supported, vec!["line_crossing", "intrusion"]);
    }

    #[test]
    fn anpr_query_body_defaults_and_escapes() {
        assert!(anpr_query_body("").contains("<picTime>20000101000000000</picTime>"));
        assert!(
            anpr_query_body("20260716093012456").contains("<picTime>20260716093012456</picTime>")
        );
    }

    #[test]
    fn user_type_wire_values() {
        assert_eq!(
            onvif_user_type_wire(OnvifUserType::Administrator),
            "administrator"
        );
        assert_eq!(onvif_user_type_wire(OnvifUserType::Operator), "operator");
        assert_eq!(onvif_user_type_wire(OnvifUserType::MediaUser), "mediaUser");
    }
}
