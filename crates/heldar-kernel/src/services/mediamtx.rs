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

/// Ensure a MediaMTX path exists for this camera and return its playback URLs.
pub async fn ensure_live(state: &AppState, camera_id: &str) -> AppResult<LiveUrls> {
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
        let run_on_demand = format!(
            "ffmpeg -nostdin -rtsp_transport tcp -timeout 10000000 -i {source} -an \
{codec_args} \
-f rtsp rtsp://localhost:$RTSP_PORT/$MTX_PATH"
        );
        let body = json!({
            "runOnDemand": run_on_demand,
            "runOnDemandRestart": true,
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

    let hls = state.cfg.mediamtx_hls_base.trim_end_matches('/');
    let webrtc = state.cfg.mediamtx_webrtc_base.trim_end_matches('/');
    let rtsp = state.cfg.mediamtx_rtsp_base.trim_end_matches('/');
    Ok(LiveUrls {
        hls_url: format!("{hls}/{name}/index.m3u8"),
        webrtc_url: format!("{webrtc}/{name}"),
        rtsp_url: format!("{rtsp}/{name}"),
        name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
