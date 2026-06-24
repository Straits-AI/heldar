//! Live-view gateway integration: registers a camera's stream as a MediaMTX path (server-side,
//! credentials never exposed to the browser) and returns HLS / WebRTC / RTSP playback URLs.

use serde::Serialize;
use serde_json::json;

use crate::camera_url;
use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::models::Camera;
use crate::state::AppState;

/// Software (libx264) encoder args for the live HEVC->H.264 preview transcode (the default path).
const SOFTWARE_CODEC_ARGS: &str =
    "-c:v libx264 -preset ultrafast -tune zerolatency -profile:v baseline -pix_fmt yuv420p -g 30";

/// FFmpeg encoder args for the live preview transcode, selected by `HELDAR_LIVE_TRANSCODE_ENGINE`.
/// `software` uses libx264 (CPU); `vaapi` offloads to an Intel/AMD render node; `nvenc` to an NVIDIA
/// GPU. An unknown engine warns and falls back to software so a typo never breaks live preview.
pub fn transcode_codec_args(cfg: &Config) -> String {
    select_codec_args(&cfg.live_transcode_engine, &cfg.vaapi_device)
}

fn select_codec_args(engine: &str, vaapi_device: &str) -> String {
    match engine {
        "software" => SOFTWARE_CODEC_ARGS.to_string(),
        // VAAPI: upload the decoded frames to the render node and encode with h264_vaapi.
        "vaapi" => {
            format!("-vaapi_device {vaapi_device} -vf format=nv12,hwupload -c:v h264_vaapi -g 30")
        }
        // NVENC: low-latency NVIDIA hardware encoder.
        "nvenc" => "-c:v h264_nvenc -preset p1 -tune ll -profile:v baseline -pix_fmt yuv420p -g 30"
            .to_string(),
        other => {
            tracing::warn!(
                engine = %other,
                "unknown HELDAR_LIVE_TRANSCODE_ENGINE; falling back to software (libx264)"
            );
            SOFTWARE_CODEC_ARGS.to_string()
        }
    }
}

#[derive(Debug, Serialize)]
pub struct LiveUrls {
    pub name: String,
    pub hls_url: String,
    pub webrtc_url: String,
    pub rtsp_url: String,
}

/// MediaMTX (and our default config) listen on loopback. A playback URL like `http://127.0.0.1:8888/…`
/// is useless to a REMOTE client — over the WireGuard tunnel (or on the LAN) `127.0.0.1` is the client
/// itself, not the box. When the configured base points at loopback/unspecified, rewrite its HOST to the
/// one the client used to reach us (the request's `Host` header), preserving scheme + port. An explicitly
/// external base (a real hostname/IP, e.g. a CDN) is left untouched so operator overrides still win.
fn client_facing_base(base: &str, request_host: Option<&str>) -> String {
    let Some(host) = request_host.and_then(host_only) else {
        return base.to_string();
    };
    let Some((scheme, rest)) = base.split_once("://") else {
        return base.to_string();
    };
    let (authority, tail) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let (cur_host, port) = split_host_port(authority);
    if !is_loopback_host(cur_host) {
        return base.to_string();
    }
    let h = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    match port {
        Some(p) => format!("{scheme}://{h}:{p}{tail}"),
        None => format!("{scheme}://{h}{tail}"),
    }
}

fn is_loopback_host(h: &str) -> bool {
    matches!(h, "127.0.0.1" | "localhost" | "0.0.0.0" | "::1" | "[::1]")
}

/// Hostname from a `Host` header value: `"10.0.0.1:8000"` → `"10.0.0.1"`, `"[::1]:8000"` → `"::1"`.
fn host_only(host_header: &str) -> Option<String> {
    let h = host_header.trim();
    if h.is_empty() {
        return None;
    }
    if let Some(rest) = h.strip_prefix('[') {
        return rest.split(']').next().map(str::to_string); // IPv6 literal
    }
    Some(h.rsplit_once(':').map_or(h, |(host, _)| host).to_string())
}

/// Split a URL authority into `(host, port?)`, handling `[ipv6]:port`.
fn split_host_port(authority: &str) -> (&str, Option<&str>) {
    if let Some(rest) = authority.strip_prefix('[') {
        if let Some(close) = rest.find(']') {
            return (&rest[..close], rest[close + 1..].strip_prefix(':'));
        }
    }
    authority
        .rsplit_once(':')
        .map_or((authority, None), |(h, p)| (h, Some(p)))
}

/// Ensure a MediaMTX path exists for this camera and return its playback URLs. `request_host` is the
/// `Host` header of the originating request, used to make loopback stream URLs reachable by the client.
pub async fn ensure_live(
    state: &AppState,
    camera_id: &str,
    request_host: Option<&str>,
) -> AppResult<LiveUrls> {
    let cam: Option<Camera> = sqlx::query_as::<_, Camera>("SELECT * FROM cameras WHERE id = ?")
        .bind(camera_id)
        .fetch_optional(&state.pool)
        .await?;
    let cam = cam.ok_or_else(|| AppError::NotFound(format!("camera {camera_id} not found")))?;

    let source = camera_url::stream_url(&cam, "sub")
        .or_else(|| camera_url::record_url(&cam))
        .ok_or_else(|| AppError::BadRequest("camera has no stream URL".into()))?;

    let name = format!("cam_{camera_id}");
    let api = state.cfg.mediamtx_api_url.trim_end_matches('/');

    let existing = state
        .http
        .get(format!("{api}/v3/config/paths/get/{name}"))
        .send()
        .await;
    let already = matches!(existing, Ok(ref r) if r.status().is_success());

    if !already {
        // Transcode to H.264 on demand: many cameras (e.g. these HikVision units) emit HEVC, which
        // browsers can't play over HLS/WebRTC. FFmpeg decodes the camera stream and republishes
        // H.264 to this path, but only while someone is actually watching (runOnDemand). The raw
        // stream is still recorded untouched by the recorder; this decode is preview-only.
        // $MTX_PATH / $RTSP_PORT are substituted by MediaMTX; credentials stay server-side. The
        // video encoder args are selected by HELDAR_LIVE_TRANSCODE_ENGINE (software | vaapi | nvenc).
        let codec_args = transcode_codec_args(&state.cfg);
        // Live audio is opt-in per camera, reusing the same `record_audio` intent as the recorder:
        // a camera you record audio for can also be listened to live (re-encoded to AAC for HLS; a
        // no-op when the source has no audio track). Cameras without it stay video-only (`-an`).
        let audio_args = if cam.record_audio {
            "-c:a aac -b:a 96k"
        } else {
            "-an"
        };
        let run_on_demand = format!(
            "ffmpeg -nostdin -rtsp_transport tcp -timeout 10000000 -i {source} {audio_args} \
{codec_args} \
-f rtsp rtsp://localhost:$RTSP_PORT/$MTX_PATH"
        );
        let body = json!({
            "runOnDemand": run_on_demand,
            "runOnDemandRestart": true,
            // The HEVC→H.264 transcode cold-start (ffmpeg connect + first keyframe) routinely exceeds
            // MediaMTX's 10s default, which would drop the WHEP/HLS reader before the source is ready.
            "runOnDemandStartTimeout": "30s",
            "runOnDemandCloseAfter": "10s",
        });
        let resp = state
            .http
            .post(format!("{api}/v3/config/paths/add/{name}"))
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::Other(anyhow::anyhow!("MediaMTX unreachable at {api}: {e}")))?;
        let code = resp.status();
        if !code.is_success() && code.as_u16() != 400 {
            let txt = resp.text().await.unwrap_or_default();
            return Err(AppError::Other(anyhow::anyhow!(
                "MediaMTX add-path failed ({code}): {txt}"
            )));
        }
    }

    // Rewrite loopback bases to the host the client actually reached us on, so streams are reachable
    // over the tunnel / LAN (not just from the box itself).
    let hls_base = client_facing_base(&state.cfg.mediamtx_hls_base, request_host);
    let webrtc_base = client_facing_base(&state.cfg.mediamtx_webrtc_base, request_host);
    let rtsp_base = client_facing_base(&state.cfg.mediamtx_rtsp_base, request_host);
    let hls = hls_base.trim_end_matches('/');
    let webrtc = webrtc_base.trim_end_matches('/');
    let rtsp = rtsp_base.trim_end_matches('/');
    Ok(LiveUrls {
        hls_url: format!("{hls}/{name}/index.m3u8"),
        webrtc_url: format!("{webrtc}/{name}"),
        rtsp_url: format!("{rtsp}/{name}"),
        name,
    })
}

/// Program MediaMTX's WebRTC ICE servers (STUN/TURN) so it gathers reachable candidates for remote
/// viewing — needed for symmetric-NAT traversal. `ice` is a MediaMTX `webrtcICEServers2` array
/// (`[{"url":..,"username"?:..,"password"?:..}]`). Patches the RUNNING MediaMTX over its API (no restart).
pub async fn set_webrtc_ice_servers(state: &AppState, ice: &serde_json::Value) -> AppResult<()> {
    let api = state.cfg.mediamtx_api_url.trim_end_matches('/');
    let resp = state
        .http
        .patch(format!("{api}/v3/config/global/patch"))
        .json(&json!({ "webrtcICEServers2": ice }))
        .send()
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!("MediaMTX unreachable at {api}: {e}")))?;
    if !resp.status().is_success() {
        let code = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        return Err(AppError::Other(anyhow::anyhow!(
            "MediaMTX set-ice failed ({code}): {txt}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_base_is_rewritten_to_the_request_host() {
        // tunnel client: dashboard reached at 10.200.0.1:8000 -> stream at 10.200.0.1:8888
        assert_eq!(
            client_facing_base("http://127.0.0.1:8888", Some("10.200.0.1:8000")),
            "http://10.200.0.1:8888"
        );
        // LAN client, localhost base, hostname Host, path preserved
        assert_eq!(
            client_facing_base("http://localhost:8889/", Some("192.168.1.50:8000")),
            "http://192.168.1.50:8889/"
        );
        // rtsp scheme + 0.0.0.0 also rewritten
        assert_eq!(
            client_facing_base("rtsp://0.0.0.0:8554", Some("box.local")),
            "rtsp://box.local:8554"
        );
    }

    #[test]
    fn non_loopback_base_and_missing_host_are_left_untouched() {
        // operator set a real external base -> respected
        assert_eq!(
            client_facing_base("https://cdn.example.com:8888", Some("10.200.0.1:8000")),
            "https://cdn.example.com:8888"
        );
        // no Host header -> unchanged
        assert_eq!(
            client_facing_base("http://127.0.0.1:8888", None),
            "http://127.0.0.1:8888"
        );
    }

    #[test]
    fn ipv6_request_host_is_bracketed() {
        assert_eq!(
            client_facing_base("http://127.0.0.1:8888", Some("[fd00::1]:8000")),
            "http://[fd00::1]:8888"
        );
    }

    #[test]
    fn codec_args_select_by_engine() {
        assert_eq!(
            select_codec_args("software", "/dev/dri/renderD128"),
            SOFTWARE_CODEC_ARGS
        );
        let vaapi = select_codec_args("vaapi", "/dev/dri/renderD129");
        assert!(vaapi.contains("h264_vaapi"));
        assert!(vaapi.contains("/dev/dri/renderD129"));
        assert!(select_codec_args("nvenc", "/dev/dri/renderD128").contains("h264_nvenc"));
        // Unknown engine falls back to software (libx264).
        assert_eq!(
            select_codec_args("bogus", "/dev/dri/renderD128"),
            SOFTWARE_CODEC_ARGS
        );
    }
}
