//! Live-view gateway integration: registers a camera's stream as a MediaMTX path (server-side,
//! credentials never exposed to the browser) and returns HLS / WebRTC / RTSP playback URLs.

use std::time::Duration;

use serde::Serialize;
use serde_json::json;

use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::models::Camera;
use crate::state::AppState;

/// Software (libx264) encoder args for the live HEVC->H.264 preview transcode (the default path).
const SOFTWARE_CODEC_ARGS: &str =
    "-c:v libx264 -preset ultrafast -tune zerolatency -profile:v baseline -pix_fmt yuv420p -g 30";

/// The engines an operator may select for the live preview transcode.
pub const VALID_ENGINES: [&str; 3] = ["software", "vaapi", "nvenc"];

/// The EFFECTIVE transcode engine: the operator's settings-table override when valid, else the
/// `HELDAR_LIVE_TRANSCODE_ENGINE` env default — the same precedence as the disk/DB size caps.
pub async fn effective_engine(pool: &sqlx::SqlitePool, cfg: &Config) -> String {
    crate::services::settings::get_str(pool, crate::services::settings::LIVE_TRANSCODE_ENGINE)
        .await
        .filter(|e| VALID_ENGINES.contains(&e.as_str()))
        .unwrap_or_else(|| {
            // Canonicalize an invalid env default to what actually runs (select_codec_args falls
            // back to software for unknown engines), so the API/UI never report a phantom engine.
            if VALID_ENGINES.contains(&cfg.live_transcode_engine.as_str()) {
                cfg.live_transcode_engine.clone()
            } else {
                "software".to_string()
            }
        })
}

/// FFmpeg encoder args for the live preview transcode under the effective engine. `software` uses
/// libx264 (CPU); `vaapi` offloads to an Intel/AMD render node; `nvenc` to an NVIDIA GPU. An unknown
/// engine warns and falls back to software so a typo never breaks live preview.
pub async fn effective_codec_args(pool: &sqlx::SqlitePool, cfg: &Config) -> String {
    select_codec_args(&effective_engine(pool, cfg).await, &cfg.vaapi_device)
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
    /// Short-lived, path-scoped read token (see [`crate::services::live_token`]). Already appended as
    /// `?token=` to `hls_url`/`rtsp_url`; returned separately so the frontend can append it AFTER the
    /// `/whep` suffix it adds to `webrtc_url`, and so the HLS loader can carry it onto segment requests.
    pub token: String,
}

/// MediaMTX (and our default config) listen on loopback. A playback URL like `http://127.0.0.1:8888/…`
/// is useless to a REMOTE client — over an overlay tunnel (Tailscale/NetBird/WireGuard) or on the LAN,
/// `127.0.0.1` is the client
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

/// Origin-relative prefixes used when [`crate::config::Config::media_same_origin`] is on. These are a
/// CONTRACT with the reverse proxy: it must route `{prefix}/<rest>` to the matching MediaMTX port with
/// the prefix stripped (HLS 8888, WebRTC/WHEP 8889). Changing either string requires the same change
/// in `deploy/Caddyfile`.
const MEDIA_HLS_PREFIX: &str = "/live/hls";
const MEDIA_WHEP_PREFIX: &str = "/live/whep";

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

/// Ensure the camera's MediaMTX path exists (plain — receive-only) and that the KERNEL-OWNED live
/// publisher is running for it, then return playback URLs. `request_host` is the `Host` header of
/// the originating request, used to make loopback stream URLs reachable by the client.
///
/// The transcode ffmpeg is spawned by [`crate::services::live_publisher`], never by MediaMTX
/// (`runOnDemand` is deliberately unused: MediaMTX's exec environment — e.g. the official docker
/// image, which ships no ffmpeg — is not ours to assume; see the live_publisher module docs).
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
    if !cam.enabled {
        // Same rule the remote WHEP bridge enforces: a disabled camera has no live surface. Without
        // this, the browser opens a stream that can never become ready and hammers 404s.
        return Err(AppError::BadRequest(format!(
            "camera {camera_id} is disabled — enable it to view live"
        )));
    }

    // Explicit 400 when the camera has no usable stream URL (address+creds or an explicit URL) —
    // otherwise the viewer would stall through the ready-wait and get URLs that can never work.
    if crate::camera_url::stream_url(&cam, "sub")
        .or_else(|| crate::camera_url::record_url(&cam))
        .is_none()
    {
        return Err(AppError::BadRequest("camera has no stream URL".into()));
    }

    let name = format!("cam_{camera_id}");
    let api = state.cfg.mediamtx_api_url.trim_end_matches('/');

    ensure_plain_path(&state.http, api, &name)
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!("MediaMTX path setup failed: {e}")))?;
    // Record viewer demand (starts the publisher; resets the idle-reap clock). Re-validates the
    // camera under its serialization lock, so a concurrent disable can't be raced past.
    state.live.demand(camera_id).await;

    // Give a cold-started publisher a bounded window to come up so the FIRST player request already
    // finds a ready stream (measured ~2s on HEVC sub-streams). On timeout still return the URLs —
    // players retry, and the supervise loop keeps working on the stream.
    wait_ready(&state.http, api, &name, Duration::from_secs(8)).await;

    // Rewrite loopback bases to the host the client actually reached us on, so streams are reachable
    // over the tunnel / LAN (not just from the box itself).
    let hls_base = client_facing_base(&state.cfg.mediamtx_hls_base, request_host);
    let webrtc_base = client_facing_base(&state.cfg.mediamtx_webrtc_base, request_host);
    let rtsp_base = client_facing_base(&state.cfg.mediamtx_rtsp_base, request_host);
    let hls = hls_base.trim_end_matches('/');
    let webrtc = webrtc_base.trim_end_matches('/');
    let rtsp = rtsp_base.trim_end_matches('/');
    // Mint a short-lived, path-scoped read token now that `can_view` has passed upstream. MediaMTX
    // (configured with HTTP external auth) calls the kernel back per read; when kernel auth is enabled
    // the read is refused unless the URL carries this token — so the browser streaming directly from
    // MediaMTX is still gated by kernel auth. Token chars are URL-safe (base64url + digits + dots).
    let token = crate::services::live_token::mint(
        &name,
        chrono::Utc::now().timestamp(),
        state.cfg.live_token_ttl_secs,
    );
    let (hls_url, webrtc_url) =
        browser_media_urls(state.cfg.media_same_origin, hls, webrtc, &name, &token);
    Ok(LiveUrls {
        hls_url,
        webrtc_url,
        rtsp_url: format!("{rtsp}/{name}?token={token}"),
        name,
        token,
    })
}

/// The two BROWSER-facing media URLs (HLS + WebRTC/WHEP base) for `name`.
///
/// With `same_origin`, they are origin-RELATIVE so they inherit the page's scheme and port. This is
/// what makes live view work behind a TLS terminator: an absolute `http://host:8888/…` served to an
/// `https://` page is blocked as mixed content, and HSTS only makes it worse by upgrading it to an
/// `https://host:8888` that MediaMTX serves no TLS on. The proxy in front is responsible for mapping
/// [`MEDIA_HLS_PREFIX`]/[`MEDIA_WHEP_PREFIX`] back onto MediaMTX (see `deploy/Caddyfile`).
///
/// Otherwise they stay absolute against the (host-rewritten) MediaMTX bases — the plain-HTTP LAN
/// dashboard, where there is no proxy to route the prefixes.
fn browser_media_urls(
    same_origin: bool,
    hls_base: &str,
    webrtc_base: &str,
    name: &str,
    token: &str,
) -> (String, String) {
    if same_origin {
        return (
            format!("{MEDIA_HLS_PREFIX}/{name}/index.m3u8?token={token}"),
            format!("{MEDIA_WHEP_PREFIX}/{name}"),
        );
    }
    (
        format!("{hls_base}/{name}/index.m3u8?token={token}"),
        format!("{webrtc_base}/{name}"),
    )
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

/// Ensure `name` exists as a PLAIN MediaMTX path — no `runOnDemand`/`runOnInit` exec config (the
/// kernel owns the publisher process). Also strips those commands from a pre-existing path, healing
/// deployments configured by older kernels.
pub async fn ensure_plain_path(
    http: &reqwest::Client,
    api: &str,
    name: &str,
) -> anyhow::Result<()> {
    let plain = json!({ "runOnDemand": "", "runOnInit": "" });
    let existing = http
        .get(format!("{api}/v3/config/paths/get/{name}"))
        .send()
        .await;
    match existing {
        Ok(r) if r.status().is_success() => {
            let cfg: serde_json::Value = r.json().await.unwrap_or_default();
            let stale = ["runOnDemand", "runOnInit"].iter().any(|k| {
                cfg.get(*k)
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| !s.is_empty())
            });
            if stale {
                let resp = http
                    .patch(format!("{api}/v3/config/paths/patch/{name}"))
                    .json(&plain)
                    .send()
                    .await
                    .map_err(|e| anyhow::anyhow!("MediaMTX unreachable at {api}: {e}"))?;
                if !resp.status().is_success() {
                    anyhow::bail!("MediaMTX patch-path failed ({})", resp.status());
                }
            }
            Ok(())
        }
        _ => {
            let resp = http
                .post(format!("{api}/v3/config/paths/add/{name}"))
                .json(&plain)
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("MediaMTX unreachable at {api}: {e}"))?;
            let code = resp.status();
            // 400 = "path already exists" (raced another ensure) — fine.
            if !code.is_success() && code.as_u16() != 400 {
                anyhow::bail!("MediaMTX add-path failed ({code})");
            }
            Ok(())
        }
    }
}

/// Delete a MediaMTX path (best-effort; missing path or unreachable MediaMTX is fine).
pub async fn delete_path(http: &reqwest::Client, api: &str, name: &str) {
    let _ = http
        .delete(format!("{api}/v3/config/paths/delete/{name}"))
        .send()
        .await;
}

/// How many readers MediaMTX reports on a path; `None` when the path is missing or MediaMTX is
/// unreachable (callers must treat unknown as "may have viewers").
pub async fn path_readers(http: &reqwest::Client, api: &str, name: &str) -> Option<usize> {
    let r = http
        .get(format!("{api}/v3/paths/get/{name}"))
        .send()
        .await
        .ok()?;
    if !r.status().is_success() {
        return None;
    }
    let v: serde_json::Value = r.json().await.ok()?;
    Some(v.get("readers")?.as_array()?.len())
}

/// Poll until the path reports `ready: true` (a publisher is delivering) or `max` elapses.
async fn wait_ready(http: &reqwest::Client, api: &str, name: &str, max: Duration) {
    let start = std::time::Instant::now();
    while start.elapsed() < max {
        let ready = async {
            let r = http
                .get(format!("{api}/v3/paths/get/{name}"))
                .send()
                .await
                .ok()?;
            let v: serde_json::Value = r.json().await.ok()?;
            v.get("ready")?.as_bool()
        }
        .await;
        if ready == Some(true) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
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

    /// The operator's settings-table engine override beats the env default; an invalid stored value
    /// is ignored (falls back to env) so a corrupt setting can never break live preview.
    #[tokio::test]
    async fn effective_engine_settings_override_beats_env_and_invalid_is_ignored() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let mut cfg = Config::from_env();
        cfg.live_transcode_engine = "software".into();

        // unset → env default
        assert_eq!(effective_engine(&pool, &cfg).await, "software");
        // valid override wins
        crate::services::settings::set_str(
            &pool,
            crate::services::settings::LIVE_TRANSCODE_ENGINE,
            "nvenc",
        )
        .await
        .unwrap();
        assert_eq!(effective_engine(&pool, &cfg).await, "nvenc");
        assert!(effective_codec_args(&pool, &cfg)
            .await
            .contains("h264_nvenc"));
        // invalid override is ignored → env default again
        crate::services::settings::set_str(
            &pool,
            crate::services::settings::LIVE_TRANSCODE_ENGINE,
            "bogus",
        )
        .await
        .unwrap();
        assert_eq!(effective_engine(&pool, &cfg).await, "software");
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

    #[test]
    fn same_origin_media_urls_are_relative_so_https_pages_can_load_them() {
        // The regression: absolute http://host:8888 URLs handed to an https:// dashboard are blocked
        // as mixed content, so live view dies entirely behind the TLS overlay.
        let (hls, whep) = browser_media_urls(
            true,
            "http://cam.example.com:8888",
            "http://cam.example.com:8889",
            "cam_front",
            "tok123",
        );
        assert_eq!(hls, "/live/hls/cam_front/index.m3u8?token=tok123");
        assert_eq!(whep, "/live/whep/cam_front");
        // Relative means no scheme and no authority to disagree with the page's.
        for u in [&hls, &whep] {
            assert!(u.starts_with('/'), "must be origin-relative: {u}");
            assert!(!u.contains("://"), "must carry no scheme: {u}");
        }
    }

    #[test]
    fn default_mode_keeps_absolute_mediamtx_urls() {
        // The plain-HTTP LAN dashboard has no proxy to route /live/* — it must keep talking to
        // MediaMTX directly, so this path must not change.
        let (hls, whep) = browser_media_urls(
            false,
            "http://192.168.1.50:8888",
            "http://192.168.1.50:8889",
            "cam_front",
            "tok123",
        );
        assert_eq!(
            hls,
            "http://192.168.1.50:8888/cam_front/index.m3u8?token=tok123"
        );
        assert_eq!(whep, "http://192.168.1.50:8889/cam_front");
    }

    #[test]
    fn same_origin_prefixes_match_the_reverse_proxy_contract() {
        // These strings are duplicated in deploy/Caddyfile (handle_path blocks). If either side
        // changes without the other, live view 404s behind TLS.
        assert_eq!(MEDIA_HLS_PREFIX, "/live/hls");
        assert_eq!(MEDIA_WHEP_PREFIX, "/live/whep");
    }
}
