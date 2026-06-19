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
    DeviceInfo, NtpConfig, OnvifSettings, OnvifUserType, OsdConfig, TimeConfig, VideoConfig,
};
use super::CameraConfigProvider;
use crate::error::{AppError, AppResult};
use crate::models::Camera;

/// XML namespace every HikVision ISAPI body carries.
const HIK_NS: &str = "http://www.hikvision.com/ver20/XMLSchema";
/// Overlay (OSD) endpoint for the primary video input channel.
const OSD_PATH: &str = "/ISAPI/System/Video/inputs/channels/1/overlays";
/// ONVIF user provisioning endpoint.
const ONVIF_USERS_PATH: &str = "/ISAPI/Security/ONVIF/users";

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
        let password = cam.password.clone().unwrap_or_default();
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
fn parse_bool_text(s: &str) -> bool {
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
fn find_open(xml: &str, local: &str, from: usize) -> Option<(usize, usize, bool)> {
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
fn find_close(xml: &str, local: &str) -> Option<usize> {
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
fn first_inner<'a>(xml: &'a str, local: &str) -> Option<&'a str> {
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
fn first_text(xml: &str, local: &str) -> Option<String> {
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
fn xml_unescape(s: &str) -> String {
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
    fn user_type_wire_values() {
        assert_eq!(
            onvif_user_type_wire(OnvifUserType::Administrator),
            "administrator"
        );
        assert_eq!(onvif_user_type_wire(OnvifUserType::Operator), "operator");
        assert_eq!(onvif_user_type_wire(OnvifUserType::MediaUser), "mediaUser");
    }
}
