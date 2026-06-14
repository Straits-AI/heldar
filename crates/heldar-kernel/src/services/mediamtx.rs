//! Live-view gateway integration: registers a camera's stream as a MediaMTX path (server-side,
//! credentials never exposed to the browser) and returns HLS / WebRTC / RTSP playback URLs.

use serde::Serialize;
use serde_json::json;

use crate::camera_url;
use crate::error::{AppError, AppResult};
use crate::models::Camera;
use crate::state::AppState;

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
        // $MTX_PATH / $RTSP_PORT are substituted by MediaMTX; credentials stay server-side.
        let run_on_demand = format!(
            "ffmpeg -nostdin -rtsp_transport tcp -timeout 10000000 -i {source} -an \
-c:v libx264 -preset ultrafast -tune zerolatency -profile:v baseline -pix_fmt yuv420p -g 30 \
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
