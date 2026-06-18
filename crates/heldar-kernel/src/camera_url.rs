//! RTSP URL construction from vendor templates, plus credential masking.

use chrono::{DateTime, Utc};

use crate::models::Camera;

/// Map a logical stream name to a HikVision channel id (101 = main, 102 = sub).
fn hik_channel(stream: &str) -> &'static str {
    if stream == "sub" {
        "102"
    } else {
        "101"
    }
}

/// Percent-encode the reserved characters that would break the `user:pass@host` userinfo section.
pub(crate) fn encode_userinfo(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            // RFC 3986 unreserved + a few safe chars
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Build the RTSP URL (with credentials) for the given stream ("main" | "sub").
/// Honors an explicit per-stream URL override; otherwise builds from the vendor template.
pub fn stream_url(cam: &Camera, stream: &str) -> Option<String> {
    let explicit = if stream == "sub" {
        cam.sub_stream_url.as_deref()
    } else {
        cam.main_stream_url.as_deref()
    };
    if let Some(u) = explicit {
        if !u.trim().is_empty() {
            return Some(u.trim().to_string());
        }
    }

    let host = cam.address.as_deref()?.trim();
    if host.is_empty() {
        return None;
    }
    let port = cam.rtsp_port;

    let creds = match (cam.username.as_deref(), cam.password.as_deref()) {
        (Some(u), Some(p)) if !u.is_empty() => {
            format!("{}:{}@", encode_userinfo(u), encode_userinfo(p))
        }
        (Some(u), _) if !u.is_empty() => format!("{}@", encode_userinfo(u)),
        _ => String::new(),
    };

    let path = match cam.vendor.as_str() {
        "hikvision" => format!("/Streaming/Channels/{}", hik_channel(stream)),
        "dahua" => format!(
            "/cam/realmonitor?channel=1&subtype={}",
            if stream == "sub" { "1" } else { "0" }
        ),
        // generic/onvif: without an explicit URL we cannot guess a path.
        _ => return None,
    };

    Some(format!("rtsp://{creds}{host}:{port}{path}"))
}

/// The RTSP URL for the stream this camera records.
pub fn record_url(cam: &Camera) -> Option<String> {
    stream_url(cam, &cam.record_stream)
}

/// A Hikvision-style replay timestamp: `20260613T120500Z` (UTC, used by ISAPI/RTSP playback).
fn hik_replay_time(t: DateTime<Utc>) -> String {
    t.format("%Y%m%dT%H%M%SZ").to_string()
}

/// Build the replay (playback) URL used by ANR to pull a `[start, end]` window from the camera's
/// ONBOARD storage. Honors a per-camera `anr_replay_url_template` (with `{start}` / `{end}`
/// placeholders, filled with Hikvision-format timestamps); otherwise defaults to the Hikvision RTSP
/// playback endpoint (`/Streaming/tracks/{channel}?starttime=..&endtime=..`) built from the camera's
/// address + credentials. Returns `None` when there is no template and no host/credentials to build
/// one from. Best-effort and camera-dependent — see `services/anr.rs`.
pub fn anr_replay_url(cam: &Camera, start: DateTime<Utc>, end: DateTime<Utc>) -> Option<String> {
    let s = hik_replay_time(start);
    let e = hik_replay_time(end);
    if let Some(tpl) = cam.anr_replay_url_template.as_deref() {
        let tpl = tpl.trim();
        if !tpl.is_empty() {
            return Some(tpl.replace("{start}", &s).replace("{end}", &e));
        }
    }
    let host = cam.address.as_deref()?.trim();
    if host.is_empty() {
        return None;
    }
    let port = cam.rtsp_port;
    let creds = match (cam.username.as_deref(), cam.password.as_deref()) {
        (Some(u), Some(p)) if !u.is_empty() => {
            format!("{}:{}@", encode_userinfo(u), encode_userinfo(p))
        }
        (Some(u), _) if !u.is_empty() => format!("{}@", encode_userinfo(u)),
        _ => String::new(),
    };
    let channel = hik_channel(&cam.record_stream);
    Some(format!(
        "rtsp://{creds}{host}:{port}/Streaming/tracks/{channel}?starttime={s}&endtime={e}"
    ))
}

/// Schemes permitted for explicit camera stream URLs. Excludes `file:`, `gopher:`, etc., which
/// would let ffmpeg/ffprobe/MediaMTX read local files or reach unintended protocols (SSRF/LFI).
const ALLOWED_SCHEMES: &[&str] = &["rtsp", "rtsps", "http", "https"];

/// Validate an operator-supplied stream URL: must parse and use an allowed scheme.
pub fn validate_stream_url(url: &str) -> Result<(), String> {
    let url = url.trim();
    let Some((scheme, _)) = url.split_once("://") else {
        return Err(format!(
            "invalid stream URL `{}` (no scheme://)",
            mask_url(url)
        ));
    };
    let scheme = scheme.to_ascii_lowercase();
    if !ALLOWED_SCHEMES.contains(&scheme.as_str()) {
        return Err(format!(
            "stream URL scheme `{scheme}` not allowed; use one of {ALLOWED_SCHEMES:?}"
        ));
    }
    Ok(())
}

/// Replace `user:pass@` (or `user@`) credentials in an RTSP/HTTP URL with `***` for safe logging/display.
pub fn mask_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let after = scheme_end + 3;
    // The userinfo/host boundary is the LAST '@' before the first '/' of the authority; using the
    // last '@' ensures a literal '@' inside the password (from an explicit URL) is fully masked.
    let authority_end = url[after..]
        .find('/')
        .map(|i| after + i)
        .unwrap_or(url.len());
    if let Some(at_rel) = url[after..authority_end].rfind('@') {
        let at = after + at_rel;
        format!("{}***@{}", &url[..after], &url[at + 1..])
    } else {
        url.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Camera;
    use chrono::Utc;
    use serde_json::json;
    use sqlx::types::Json;

    fn base() -> Camera {
        Camera {
            id: "cam1".into(),
            site_id: None,
            name: "Cam 1".into(),
            vendor: "hikvision".into(),
            model: None,
            address: Some("192.168.0.2".into()),
            rtsp_port: 554,
            username: Some("admin".into()),
            password: Some("p@ss/w:rd".into()),
            main_stream_url: None,
            sub_stream_url: None,
            record_stream: "main".into(),
            codec: None,
            resolution_main: None,
            resolution_sub: None,
            fps_main: None,
            fps_sub: None,
            capabilities: Json(json!({})),
            record_enabled: true,
            segment_seconds: 60,
            retention_hours: 24,
            storage_quota_bytes: None,
            record_audio: false,
            record_mode: "continuous".into(),
            pre_roll_seconds: 10,
            post_roll_seconds: 30,
            mirror_enabled: false,
            anr_enabled: false,
            anr_replay_url_template: None,
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn hikvision_main_url_percent_encodes_credentials() {
        let c = base();
        assert_eq!(
            stream_url(&c, "main").unwrap(),
            "rtsp://admin:p%40ss%2Fw%3Ard@192.168.0.2:554/Streaming/Channels/101"
        );
    }

    #[test]
    fn hikvision_sub_uses_channel_102() {
        assert!(stream_url(&base(), "sub")
            .unwrap()
            .ends_with("/Streaming/Channels/102"));
    }

    #[test]
    fn explicit_override_takes_precedence() {
        let mut c = base();
        c.main_stream_url = Some("rtsp://example/stream".into());
        assert_eq!(stream_url(&c, "main").unwrap(), "rtsp://example/stream");
    }

    #[test]
    fn generic_vendor_without_url_is_none() {
        let mut c = base();
        c.vendor = "generic".into();
        c.main_stream_url = None;
        assert!(stream_url(&c, "main").is_none());
    }

    #[test]
    fn mask_url_hides_credentials() {
        assert_eq!(
            mask_url("rtsp://admin:secret@10.0.0.1:554/Streaming/Channels/101"),
            "rtsp://***@10.0.0.1:554/Streaming/Channels/101"
        );
        assert_eq!(mask_url("rtsp://10.0.0.1:554/x"), "rtsp://10.0.0.1:554/x");
    }

    #[test]
    fn mask_url_handles_at_in_password() {
        // An explicit URL with a literal '@' in the password must be fully masked (use last '@').
        assert_eq!(
            mask_url("rtsp://user:p@ss@10.0.0.1:554/x"),
            "rtsp://***@10.0.0.1:554/x"
        );
    }

    #[test]
    fn anr_replay_url_default_hikvision_playback() {
        let c = base();
        let start = parse_t("2026-06-13T12:00:00Z");
        let end = parse_t("2026-06-13T12:01:30Z");
        assert_eq!(
            anr_replay_url(&c, start, end).unwrap(),
            "rtsp://admin:p%40ss%2Fw%3Ard@192.168.0.2:554/Streaming/tracks/101?\
             starttime=20260613T120000Z&endtime=20260613T120130Z"
        );
    }

    #[test]
    fn anr_replay_url_honors_template_placeholders() {
        let mut c = base();
        c.anr_replay_url_template = Some("rtsp://cam/replay?s={start}&e={end}".into());
        assert_eq!(
            anr_replay_url(
                &c,
                parse_t("2026-06-13T12:00:00Z"),
                parse_t("2026-06-13T12:00:05Z")
            )
            .unwrap(),
            "rtsp://cam/replay?s=20260613T120000Z&e=20260613T120005Z"
        );
    }

    #[test]
    fn anr_replay_url_none_without_host_or_template() {
        let mut c = base();
        c.vendor = "generic".into();
        c.address = None;
        c.anr_replay_url_template = None;
        assert!(anr_replay_url(&c, Utc::now(), Utc::now()).is_none());
    }

    fn parse_t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn validate_stream_url_rejects_dangerous_schemes() {
        assert!(validate_stream_url("rtsp://10.0.0.1:554/x").is_ok());
        assert!(validate_stream_url("https://cam/stream").is_ok());
        assert!(validate_stream_url("file:///etc/passwd").is_err());
        assert!(validate_stream_url("gopher://x").is_err());
        assert!(validate_stream_url("not-a-url").is_err());
    }
}
