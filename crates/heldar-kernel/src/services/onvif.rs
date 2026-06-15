//! ONVIF (Profile S MVP): WS-Discovery, a device probe, and PTZ control.
//!
//! All SOAP is hand-rolled with `format!` and parsed by substring extraction (no XML crate, per the
//! kernel's offline-build constraint). Authentication uses a WS-Security UsernameToken with a
//! `PasswordDigest` = `base64(sha1(nonce + created + password))` built from the existing `sha1` +
//! `base64` crates.
//!
//! Scope (intentionally narrow): device identification (GetDeviceInformation), service discovery
//! (GetCapabilities, falling back to GetServices), media profiles (GetProfiles), best-effort stream
//! URI (GetStreamUri), and PTZ (ContinuousMove / Stop / GetPresets / GotoPreset). Events, Profile G
//! (recording/replay), Profile T, imaging, and absolute/relative moves are out of scope.

use std::collections::HashSet;
use std::time::Duration;

use argon2::password_hash::rand_core::OsRng;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use chrono::{SecondsFormat, Utc};
use rand_core::RngCore;
use serde::Serialize;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use sqlx::types::Json as SqlxJson;
use sqlx::SqlitePool;
use tokio::net::UdpSocket;
use uuid::Uuid;

use crate::camera_url;
use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::models::{Camera, CameraOnvif, PtzPreset};
use crate::state::AppState;

const WSDD_MULTICAST: &str = "239.255.255.250:3702";

// ONVIF / WS-* namespace + action constants.
const NS_DEVICE: &str = "http://www.onvif.org/ver10/device/wsdl";
const NS_MEDIA: &str = "http://www.onvif.org/ver10/media/wsdl";
const NS_PTZ: &str = "http://www.onvif.org/ver20/ptz/wsdl";

// ========================= XML helpers (substring extraction) =========================
//
// These tolerate namespace prefixes and attributes on tags. They assume the small, well-formed SOAP
// bodies ONVIF devices return (no same-name nesting), which holds for every element we read here.

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

/// Extract the value of attribute `name` from an opening tag string (e.g. `<tptz:Preset token="P1">`).
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

/// Decode the five predefined XML entities (enough for ONVIF text/attribute values).
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

/// Extract a SOAP fault reason from a fault body, if present.
fn fault_reason(xml: &str) -> Option<String> {
    first_text(xml, "Text").or_else(|| first_text(xml, "faultstring"))
}

/// Extract the host (no scheme/userinfo/port/path) from a URL.
fn host_of(url: &str) -> Option<String> {
    let after = url.split("://").nth(1)?;
    let authority = after.split('/').next()?;
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = authority.split(':').next()?;
    (!host.is_empty()).then(|| host.to_string())
}

// ========================= SOAP envelope + WS-Security =========================

/// Build the WS-Security UsernameToken header (PasswordDigest). Empty when no username is set
/// (anonymous request — many devices allow GetDeviceInformation/GetCapabilities without auth).
fn security_header(user: Option<&str>, pass: Option<&str>) -> String {
    let Some(user) = user.filter(|u| !u.is_empty()) else {
        return String::new();
    };
    let pass = pass.unwrap_or("");
    let mut nonce = [0u8; 16];
    OsRng.fill_bytes(&mut nonce);
    let created = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let mut hasher = Sha1::new();
    hasher.update(nonce);
    hasher.update(created.as_bytes());
    hasher.update(pass.as_bytes());
    let digest = BASE64.encode(hasher.finalize());
    let nonce_b64 = BASE64.encode(nonce);
    format!(
        "<wsse:Security s:mustUnderstand=\"1\">\
<wsse:UsernameToken>\
<wsse:Username>{user}</wsse:Username>\
<wsse:Password Type=\"http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-username-token-profile-1.0#PasswordDigest\">{digest}</wsse:Password>\
<wsse:Nonce EncodingType=\"http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-soap-message-security-1.0#Base64Binary\">{nonce_b64}</wsse:Nonce>\
<wsu:Created>{created}</wsu:Created>\
</wsse:UsernameToken>\
</wsse:Security>",
        user = xml_escape(user),
    )
}

/// Wrap a SOAP body in an envelope carrying every namespace prefix the service calls use.
fn envelope(security: &str, body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<s:Envelope \
xmlns:s=\"http://www.w3.org/2003/05/soap-envelope\" \
xmlns:tds=\"{NS_DEVICE}\" \
xmlns:trt=\"{NS_MEDIA}\" \
xmlns:tptz=\"{NS_PTZ}\" \
xmlns:tt=\"http://www.onvif.org/ver10/schema\" \
xmlns:wsse=\"http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd\" \
xmlns:wsu=\"http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd\">\
<s:Header>{security}</s:Header><s:Body>{body}</s:Body></s:Envelope>"
    )
}

/// POST a SOAP envelope to an ONVIF service endpoint and return the response body text. ONVIF SOAP
/// faults (often returned with HTTP 4xx/5xx) are surfaced as a clear error with the fault reason.
async fn soap_call(
    state: &AppState,
    url: &str,
    action: &str,
    envelope: String,
) -> AppResult<String> {
    let timeout = Duration::from_millis(state.cfg.onvif_request_timeout_ms.max(500));
    let content_type = format!("application/soap+xml; charset=utf-8; action=\"{action}\"");
    let resp = state
        .http
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, content_type)
        .timeout(timeout)
        .body(envelope)
        .send()
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!("ONVIF request to {url} failed: {e}")))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let reason = fault_reason(&text).unwrap_or_else(|| format!("HTTP {status}"));
        return Err(AppError::Other(anyhow::anyhow!("ONVIF fault: {reason}")));
    }
    Ok(text)
}

// ========================= WS-Discovery =========================

/// A device found by WS-Discovery.
#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredOnvifDevice {
    /// The device's `wsa:EndpointReference/Address` (a `urn:uuid:` URN), if present.
    pub endpoint_reference: Option<String>,
    /// The first transport address (the ONVIF device service URL we would probe).
    pub device_url: String,
    /// All advertised transport addresses.
    pub xaddrs: Vec<String>,
    /// Host extracted from `device_url` (matches a camera's `address`).
    pub address: Option<String>,
    /// Advertised device types (e.g. `dn:NetworkVideoTransmitter`).
    pub types: Option<String>,
    /// Advertised scope URIs (name/hardware/location hints).
    pub scopes: Vec<String>,
}

fn discovery_probe(msg_id: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<e:Envelope \
xmlns:e=\"http://www.w3.org/2003/05/soap-envelope\" \
xmlns:w=\"http://schemas.xmlsoap.org/ws/2004/08/addressing\" \
xmlns:d=\"http://schemas.xmlsoap.org/ws/2005/04/discovery\" \
xmlns:dn=\"http://www.onvif.org/ver10/network/wsdl\">\
<e:Header>\
<w:MessageID>urn:uuid:{msg_id}</w:MessageID>\
<w:To e:mustUnderstand=\"true\">urn:schemas-xmlsoap-org:ws:2005:04:discovery</w:To>\
<w:Action e:mustUnderstand=\"true\">http://schemas.xmlsoap.org/ws/2005/04/discovery/Probe</w:Action>\
</e:Header>\
<e:Body><d:Probe><d:Types>dn:NetworkVideoTransmitter</d:Types></d:Probe></e:Body>\
</e:Envelope>"
    )
}

/// Multicast a WS-Discovery Probe and collect ProbeMatch replies for the configured window.
pub async fn discover(cfg: &Config) -> AppResult<Vec<DiscoveredOnvifDevice>> {
    let window = Duration::from_millis(cfg.onvif_discovery_timeout_ms.max(200));
    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!("ONVIF discovery: bind UDP socket: {e}")))?;
    let _ = socket.set_broadcast(true);

    let probe = discovery_probe(&Uuid::new_v4().to_string());
    socket
        .send_to(probe.as_bytes(), WSDD_MULTICAST)
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!("ONVIF discovery: send Probe: {e}")))?;

    let mut devices: Vec<DiscoveredOnvifDevice> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let deadline = tokio::time::Instant::now() + window;
    let mut buf = vec![0u8; 65_535];
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        let remaining = deadline - now;
        match tokio::time::timeout(remaining, socket.recv_from(&mut buf)).await {
            Ok(Ok((n, _src))) => {
                let xml = String::from_utf8_lossy(&buf[..n]);
                parse_probe_matches(&xml, &mut devices, &mut seen);
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "ONVIF discovery: recv error");
                break;
            }
            Err(_) => break, // window elapsed
        }
    }
    Ok(devices)
}

/// Parse every `ProbeMatch` in a discovery reply envelope into deduplicated devices.
fn parse_probe_matches(
    xml: &str,
    out: &mut Vec<DiscoveredOnvifDevice>,
    seen: &mut HashSet<String>,
) {
    for (_open, inner) in elements(xml, "ProbeMatch") {
        let xaddrs: Vec<String> = first_text(inner, "XAddrs")
            .map(|s| s.split_whitespace().map(|x| x.to_string()).collect())
            .unwrap_or_default();
        let Some(device_url) = xaddrs.first().cloned() else {
            continue;
        };
        if !seen.insert(device_url.clone()) {
            continue;
        }
        let scopes: Vec<String> = first_text(inner, "Scopes")
            .map(|s| s.split_whitespace().map(|x| x.to_string()).collect())
            .unwrap_or_default();
        out.push(DiscoveredOnvifDevice {
            endpoint_reference: first_text(inner, "Address"),
            address: host_of(&device_url),
            types: first_text(inner, "Types"),
            scopes,
            xaddrs,
            device_url,
        });
    }
}

// ========================= Probe (identify + capabilities + profiles) =========================

/// The chosen media profile and PTZ binding extracted from GetProfiles.
struct ProfileChoice {
    token: Option<String>,
    node_token: Option<String>,
    has_ptz_config: bool,
}

/// Pick a media profile: prefer the first one carrying a PTZConfiguration; otherwise the first
/// profile. Returns its token, bound PTZ node token, and whether it has a PTZConfiguration.
fn parse_profiles(xml: &str) -> ProfileChoice {
    let mut first: Option<(Option<String>, Option<String>, bool)> = None;
    for (open, inner) in elements(xml, "Profiles") {
        let token = attr_in_tag(open, "token");
        let ptz_cfg = first_inner(inner, "PTZConfiguration");
        let has_ptz = ptz_cfg.is_some();
        let node_token = ptz_cfg.and_then(|c| first_text(c, "NodeToken"));
        if has_ptz {
            return ProfileChoice {
                token,
                node_token,
                has_ptz_config: true,
            };
        }
        if first.is_none() {
            first = Some((token, node_token, has_ptz));
        }
    }
    match first {
        Some((token, node_token, has_ptz_config)) => ProfileChoice {
            token,
            node_token,
            has_ptz_config,
        },
        None => ProfileChoice {
            token: None,
            node_token: None,
            has_ptz_config: false,
        },
    }
}

/// Resolve the ONVIF device service URL for a camera: explicit override, then any previously probed
/// URL, then the standard path derived from the camera's address.
async fn resolve_device_url(
    pool: &SqlitePool,
    cam: &Camera,
    override_url: Option<String>,
) -> AppResult<String> {
    if let Some(u) = override_url
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        camera_url::validate_stream_url(&u).map_err(AppError::BadRequest)?;
        return Ok(u);
    }
    let existing: Option<String> =
        sqlx::query_scalar("SELECT device_url FROM camera_onvif WHERE camera_id = ?")
            .bind(&cam.id)
            .fetch_optional(pool)
            .await?;
    if let Some(u) = existing.filter(|s| !s.trim().is_empty()) {
        return Ok(u);
    }
    let host = cam
        .address
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            AppError::BadRequest(
                "camera has no address; set its address or pass an explicit `device_url`".into(),
            )
        })?;
    Ok(format!("http://{host}/onvif/device_service"))
}

/// Probe a camera's ONVIF interface and persist the result into `camera_onvif`.
pub async fn probe(
    state: &AppState,
    camera_id: &str,
    device_url_override: Option<String>,
) -> AppResult<CameraOnvif> {
    let cam: Camera = sqlx::query_as::<_, Camera>("SELECT * FROM cameras WHERE id = ?")
        .bind(camera_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("camera {camera_id} not found")))?;

    let device_url = resolve_device_url(&state.pool, &cam, device_url_override).await?;
    let user = cam.username.as_deref();
    let pass = cam.password.as_deref();

    // 1) Device identification.
    let info = soap_call(
        state,
        &device_url,
        &format!("{NS_DEVICE}/GetDeviceInformation"),
        envelope(&security_header(user, pass), "<tds:GetDeviceInformation/>"),
    )
    .await?;
    let manufacturer = first_text(&info, "Manufacturer");
    let model = first_text(&info, "Model");
    let firmware_version = first_text(&info, "FirmwareVersion");
    let serial_number = first_text(&info, "SerialNumber");
    let hardware_id = first_text(&info, "HardwareId");

    // 2) Service endpoints: GetCapabilities first, GetServices as a fallback.
    let (mut media_url, mut ptz_url) = match soap_call(
        state,
        &device_url,
        &format!("{NS_DEVICE}/GetCapabilities"),
        envelope(
            &security_header(user, pass),
            "<tds:GetCapabilities><tds:Category>All</tds:Category></tds:GetCapabilities>",
        ),
    )
    .await
    {
        Ok(caps) => parse_capabilities(&caps),
        Err(e) => {
            tracing::warn!(%camera_id, error = %e, "ONVIF: GetCapabilities failed; trying GetServices");
            (None, None)
        }
    };
    if media_url.is_none() {
        if let Ok(services) = soap_call(
            state,
            &device_url,
            &format!("{NS_DEVICE}/GetServices"),
            envelope(
                &security_header(user, pass),
                "<tds:GetServices><tds:IncludeCapability>false</tds:IncludeCapability></tds:GetServices>",
            ),
        )
        .await
        {
            let (m, p) = parse_services(&services);
            media_url = media_url.or(m);
            ptz_url = ptz_url.or(p);
        }
    }

    // 3) Media profiles (profile token + PTZ binding). Needs the media service URL.
    let mut profile = ProfileChoice {
        token: None,
        node_token: None,
        has_ptz_config: false,
    };
    if let Some(murl) = media_url.as_deref() {
        match soap_call(
            state,
            murl,
            &format!("{NS_MEDIA}/GetProfiles"),
            envelope(&security_header(user, pass), "<trt:GetProfiles/>"),
        )
        .await
        {
            Ok(profiles) => profile = parse_profiles(&profiles),
            Err(e) => tracing::warn!(%camera_id, error = %e, "ONVIF: GetProfiles failed"),
        }
    }

    let ptz_enabled = ptz_url.is_some() && profile.has_ptz_config && profile.token.is_some();

    // 4) Best-effort stream URI: only used to FILL a camera that has no recordable URL yet.
    if let (Some(murl), Some(token)) = (media_url.as_deref(), profile.token.as_deref()) {
        if camera_url::record_url(&cam).is_none() {
            if let Some(uri) = get_stream_uri(state, murl, token, user, pass).await {
                let with_creds = inject_creds(&uri, user, pass);
                let _ = sqlx::query(
                    "UPDATE cameras SET main_stream_url = ?, updated_at = ? WHERE id = ? AND (main_stream_url IS NULL OR main_stream_url = '')",
                )
                .bind(&with_creds)
                .bind(Utc::now())
                .bind(camera_id)
                .execute(&state.pool)
                .await;
                tracing::info!(%camera_id, "ONVIF: filled main_stream_url from GetStreamUri");
            }
        }
    }

    // Preserve any scopes captured by a prior discovery (probe itself does not fetch scopes).
    let scopes: Value =
        sqlx::query_scalar::<_, String>("SELECT scopes FROM camera_onvif WHERE camera_id = ?")
            .bind(camera_id)
            .fetch_optional(&state.pool)
            .await?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| json!([]));

    let now = Utc::now();
    sqlx::query(
        "INSERT INTO camera_onvif
           (camera_id, device_url, manufacturer, model, firmware_version, serial_number, hardware_id,
            scopes, media_url, ptz_url, profile_token, ptz_node_token, ptz_enabled, probed_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(camera_id) DO UPDATE SET
            device_url = excluded.device_url,
            manufacturer = excluded.manufacturer,
            model = excluded.model,
            firmware_version = excluded.firmware_version,
            serial_number = excluded.serial_number,
            hardware_id = excluded.hardware_id,
            scopes = excluded.scopes,
            media_url = excluded.media_url,
            ptz_url = excluded.ptz_url,
            profile_token = excluded.profile_token,
            ptz_node_token = excluded.ptz_node_token,
            ptz_enabled = excluded.ptz_enabled,
            probed_at = excluded.probed_at",
    )
    .bind(camera_id)
    .bind(&device_url)
    .bind(&manufacturer)
    .bind(&model)
    .bind(&firmware_version)
    .bind(&serial_number)
    .bind(&hardware_id)
    .bind(SqlxJson(scopes))
    .bind(&media_url)
    .bind(&ptz_url)
    .bind(&profile.token)
    .bind(&profile.node_token)
    .bind(ptz_enabled)
    .bind(now)
    .execute(&state.pool)
    .await?;

    load_onvif(&state.pool, camera_id).await
}

/// Parse media + PTZ service URLs from a GetCapabilities response.
fn parse_capabilities(xml: &str) -> (Option<String>, Option<String>) {
    let media_url = first_inner(xml, "Media").and_then(|b| first_text(b, "XAddr"));
    let ptz_url = first_inner(xml, "PTZ").and_then(|b| first_text(b, "XAddr"));
    (media_url, ptz_url)
}

/// Parse media + PTZ service URLs from a GetServices response (one `Service` per namespace).
fn parse_services(xml: &str) -> (Option<String>, Option<String>) {
    let mut media_url = None;
    let mut ptz_url = None;
    for (_open, inner) in elements(xml, "Service") {
        let ns = first_text(inner, "Namespace").unwrap_or_default();
        let xaddr = first_text(inner, "XAddr");
        if ns.contains("/media/") && media_url.is_none() {
            media_url = xaddr;
        } else if ns.contains("/ptz/") && ptz_url.is_none() {
            ptz_url = xaddr;
        }
    }
    (media_url, ptz_url)
}

/// Best-effort GetStreamUri (RTSP unicast) for a media profile.
async fn get_stream_uri(
    state: &AppState,
    media_url: &str,
    profile_token: &str,
    user: Option<&str>,
    pass: Option<&str>,
) -> Option<String> {
    let body = format!(
        "<trt:GetStreamUri>\
<trt:StreamSetup><tt:Stream>RTP-Unicast</tt:Stream>\
<tt:Transport><tt:Protocol>RTSP</tt:Protocol></tt:Transport></trt:StreamSetup>\
<trt:ProfileToken>{}</trt:ProfileToken>\
</trt:GetStreamUri>",
        xml_escape(profile_token)
    );
    match soap_call(
        state,
        media_url,
        &format!("{NS_MEDIA}/GetStreamUri"),
        envelope(&security_header(user, pass), &body),
    )
    .await
    {
        Ok(resp) => first_text(&resp, "Uri").filter(|u| u.starts_with("rtsp://")),
        Err(e) => {
            tracing::warn!(error = %e, "ONVIF: GetStreamUri failed");
            None
        }
    }
}

/// Inject `user:pass@` userinfo into an `rtsp://` URI that lacks it (so the recorder can authenticate).
fn inject_creds(uri: &str, user: Option<&str>, pass: Option<&str>) -> String {
    let Some(user) = user.filter(|u| !u.is_empty()) else {
        return uri.to_string();
    };
    let Some(rest) = uri.strip_prefix("rtsp://") else {
        return uri.to_string();
    };
    let authority = rest.split('/').next().unwrap_or("");
    if authority.contains('@') {
        return uri.to_string(); // already has userinfo
    }
    let creds = match pass.filter(|p| !p.is_empty()) {
        Some(p) => format!(
            "{}:{}@",
            camera_url::encode_userinfo(user),
            camera_url::encode_userinfo(p)
        ),
        None => format!("{}@", camera_url::encode_userinfo(user)),
    };
    format!("rtsp://{creds}{rest}")
}

// ========================= PTZ control =========================

/// Load a camera's persisted ONVIF profile (404 when the camera has not been probed yet).
pub async fn load_onvif(pool: &SqlitePool, camera_id: &str) -> AppResult<CameraOnvif> {
    sqlx::query_as::<_, CameraOnvif>("SELECT * FROM camera_onvif WHERE camera_id = ?")
        .bind(camera_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| {
            AppError::NotFound(format!(
                "camera {camera_id} has no ONVIF profile; run the ONVIF probe first"
            ))
        })
}

/// Load a camera + its ONVIF profile, asserting the PTZ service + profile token are present.
async fn load_ptz_target(
    pool: &SqlitePool,
    camera_id: &str,
) -> AppResult<(Camera, CameraOnvif, String, String)> {
    let cam: Camera = sqlx::query_as::<_, Camera>("SELECT * FROM cameras WHERE id = ?")
        .bind(camera_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("camera {camera_id} not found")))?;
    let onvif = load_onvif(pool, camera_id).await?;
    let ptz_url = onvif
        .ptz_url
        .clone()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| AppError::BadRequest("camera exposes no ONVIF PTZ service".into()))?;
    let profile_token = onvif
        .profile_token
        .clone()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| AppError::BadRequest("camera has no ONVIF media profile token".into()))?;
    Ok((cam, onvif, ptz_url, profile_token))
}

/// Continuously pan/tilt/zoom at the given normalized velocities (each clamped to -1.0..=1.0). The
/// motion runs until [`stop`] (or the device's own timeout).
pub async fn continuous_move(
    state: &AppState,
    camera_id: &str,
    pan: f64,
    tilt: f64,
    zoom: f64,
) -> AppResult<()> {
    let (cam, _onvif, ptz_url, token) = load_ptz_target(&state.pool, camera_id).await?;
    let pan = pan.clamp(-1.0, 1.0);
    let tilt = tilt.clamp(-1.0, 1.0);
    let zoom = zoom.clamp(-1.0, 1.0);
    let body = format!(
        "<tptz:ContinuousMove>\
<tptz:ProfileToken>{token}</tptz:ProfileToken>\
<tptz:Velocity>\
<tt:PanTilt x=\"{pan:.4}\" y=\"{tilt:.4}\"/>\
<tt:Zoom x=\"{zoom:.4}\"/>\
</tptz:Velocity>\
</tptz:ContinuousMove>",
        token = xml_escape(&token),
    );
    soap_call(
        state,
        &ptz_url,
        &format!("{NS_PTZ}/ContinuousMove"),
        envelope(
            &security_header(cam.username.as_deref(), cam.password.as_deref()),
            &body,
        ),
    )
    .await?;
    Ok(())
}

/// Stop all PTZ motion (pan/tilt + zoom).
pub async fn stop(state: &AppState, camera_id: &str) -> AppResult<()> {
    let (cam, _onvif, ptz_url, token) = load_ptz_target(&state.pool, camera_id).await?;
    let body = format!(
        "<tptz:Stop>\
<tptz:ProfileToken>{token}</tptz:ProfileToken>\
<tptz:PanTilt>true</tptz:PanTilt>\
<tptz:Zoom>true</tptz:Zoom>\
</tptz:Stop>",
        token = xml_escape(&token),
    );
    soap_call(
        state,
        &ptz_url,
        &format!("{NS_PTZ}/Stop"),
        envelope(
            &security_header(cam.username.as_deref(), cam.password.as_deref()),
            &body,
        ),
    )
    .await?;
    Ok(())
}

/// Fetch the camera's PTZ presets, persist them (upsert + prune stale), and return the current set.
pub async fn get_presets(state: &AppState, camera_id: &str) -> AppResult<Vec<PtzPreset>> {
    let (cam, _onvif, ptz_url, token) = load_ptz_target(&state.pool, camera_id).await?;
    let body = format!(
        "<tptz:GetPresets><tptz:ProfileToken>{token}</tptz:ProfileToken></tptz:GetPresets>",
        token = xml_escape(&token),
    );
    let resp = soap_call(
        state,
        &ptz_url,
        &format!("{NS_PTZ}/GetPresets"),
        envelope(
            &security_header(cam.username.as_deref(), cam.password.as_deref()),
            &body,
        ),
    )
    .await?;

    // Parse <tptz:Preset token="X"><tt:Name>Y</tt:Name>...</tptz:Preset>.
    let mut fetched: Vec<(String, Option<String>)> = Vec::new();
    for (open, inner) in elements(&resp, "Preset") {
        if let Some(tok) = attr_in_tag(open, "token").filter(|t| !t.is_empty()) {
            fetched.push((tok, first_text(inner, "Name")));
        }
    }

    let now = Utc::now();
    for (tok, name) in &fetched {
        let id = format!("ptz_{}", Uuid::new_v4().simple());
        sqlx::query(
            "INSERT INTO camera_ptz_presets (id, camera_id, token, name, fetched_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(camera_id, token) DO UPDATE SET name = excluded.name, fetched_at = excluded.fetched_at",
        )
        .bind(&id)
        .bind(camera_id)
        .bind(tok)
        .bind(name)
        .bind(now)
        .execute(&state.pool)
        .await?;
    }
    // Prune presets that the device no longer reports.
    if fetched.is_empty() {
        sqlx::query("DELETE FROM camera_ptz_presets WHERE camera_id = ?")
            .bind(camera_id)
            .execute(&state.pool)
            .await?;
    } else {
        let placeholders = vec!["?"; fetched.len()].join(",");
        let sql = format!(
            "DELETE FROM camera_ptz_presets WHERE camera_id = ? AND token NOT IN ({placeholders})"
        );
        let mut q = sqlx::query(&sql).bind(camera_id);
        for (tok, _) in &fetched {
            q = q.bind(tok);
        }
        q.execute(&state.pool).await?;
    }

    sqlx::query_as::<_, PtzPreset>(
        "SELECT * FROM camera_ptz_presets WHERE camera_id = ? ORDER BY token ASC",
    )
    .bind(camera_id)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)
}

/// Move the camera to a stored preset by its device token.
pub async fn goto_preset(state: &AppState, camera_id: &str, preset_token: &str) -> AppResult<()> {
    let (cam, _onvif, ptz_url, token) = load_ptz_target(&state.pool, camera_id).await?;
    if preset_token.trim().is_empty() {
        return Err(AppError::BadRequest("`token` is required".into()));
    }
    let body = format!(
        "<tptz:GotoPreset>\
<tptz:ProfileToken>{token}</tptz:ProfileToken>\
<tptz:PresetToken>{preset}</tptz:PresetToken>\
</tptz:GotoPreset>",
        token = xml_escape(&token),
        preset = xml_escape(preset_token),
    );
    soap_call(
        state,
        &ptz_url,
        &format!("{NS_PTZ}/GotoPreset"),
        envelope(
            &security_header(cam.username.as_deref(), cam.password.as_deref()),
            &body,
        ),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_simple_text_with_prefix() {
        let xml = "<tds:Manufacturer>HIKVISION</tds:Manufacturer><tds:Model>DS-2CD</tds:Model>";
        assert_eq!(
            first_text(xml, "Manufacturer").as_deref(),
            Some("HIKVISION")
        );
        assert_eq!(first_text(xml, "Model").as_deref(), Some("DS-2CD"));
        assert_eq!(first_text(xml, "SerialNumber"), None);
    }

    #[test]
    fn unescapes_entities() {
        let xml = "<x>A &amp; B &lt;c&gt;</x>";
        assert_eq!(first_text(xml, "x").as_deref(), Some("A & B <c>"));
    }

    #[test]
    fn parses_capabilities_blocks() {
        let xml = "<tt:Capabilities>\
<tt:Media><tt:XAddr>http://10.0.0.2/onvif/Media</tt:XAddr></tt:Media>\
<tt:PTZ><tt:XAddr>http://10.0.0.2/onvif/PTZ</tt:XAddr></tt:PTZ>\
</tt:Capabilities>";
        let (m, p) = parse_capabilities(xml);
        assert_eq!(m.as_deref(), Some("http://10.0.0.2/onvif/Media"));
        assert_eq!(p.as_deref(), Some("http://10.0.0.2/onvif/PTZ"));
    }

    #[test]
    fn parses_services_by_namespace() {
        let xml = "<tds:GetServicesResponse>\
<tds:Service><tds:Namespace>http://www.onvif.org/ver10/media/wsdl</tds:Namespace>\
<tds:XAddr>http://10.0.0.2/onvif/Media</tds:XAddr></tds:Service>\
<tds:Service><tds:Namespace>http://www.onvif.org/ver20/ptz/wsdl</tds:Namespace>\
<tds:XAddr>http://10.0.0.2/onvif/PTZ</tds:XAddr></tds:Service>\
</tds:GetServicesResponse>";
        let (m, p) = parse_services(xml);
        assert_eq!(m.as_deref(), Some("http://10.0.0.2/onvif/Media"));
        assert_eq!(p.as_deref(), Some("http://10.0.0.2/onvif/PTZ"));
    }

    #[test]
    fn prefers_profile_with_ptz_config() {
        let xml = "<trt:GetProfilesResponse>\
<trt:Profiles token=\"P0\" fixed=\"true\"><tt:VideoSourceConfiguration/></trt:Profiles>\
<trt:Profiles token=\"P1\"><tt:PTZConfiguration><tt:NodeToken>NODE0</tt:NodeToken></tt:PTZConfiguration></trt:Profiles>\
</trt:GetProfilesResponse>";
        let c = parse_profiles(xml);
        assert_eq!(c.token.as_deref(), Some("P1"));
        assert_eq!(c.node_token.as_deref(), Some("NODE0"));
        assert!(c.has_ptz_config);
    }

    #[test]
    fn falls_back_to_first_profile_without_ptz() {
        let xml = "<trt:Profiles token=\"OnlyOne\"><tt:VideoSourceConfiguration/></trt:Profiles>";
        let c = parse_profiles(xml);
        assert_eq!(c.token.as_deref(), Some("OnlyOne"));
        assert!(!c.has_ptz_config);
    }

    #[test]
    fn parses_preset_token_and_name() {
        let xml = "<tptz:GetPresetsResponse>\
<tptz:Preset token=\"1\"><tt:Name>Gate</tt:Name></tptz:Preset>\
<tptz:Preset token=\"2\"/>\
</tptz:GetPresetsResponse>";
        let presets: Vec<_> = elements(xml, "Preset")
            .into_iter()
            .filter_map(|(open, inner)| {
                attr_in_tag(open, "token").map(|t| (t, first_text(inner, "Name")))
            })
            .collect();
        assert_eq!(presets.len(), 2);
        assert_eq!(presets[0].0, "1");
        assert_eq!(presets[0].1.as_deref(), Some("Gate"));
        assert_eq!(presets[1].0, "2");
        assert_eq!(presets[1].1, None);
    }

    #[test]
    fn parses_xaddrs_from_probe_match() {
        let xml = "<d:ProbeMatches><d:ProbeMatch>\
<wsa:EndpointReference><wsa:Address>urn:uuid:abc</wsa:Address></wsa:EndpointReference>\
<d:Types>dn:NetworkVideoTransmitter</d:Types>\
<d:Scopes>onvif://www.onvif.org/name/Cam onvif://www.onvif.org/hardware/DS</d:Scopes>\
<d:XAddrs>http://192.168.0.2/onvif/device_service</d:XAddrs>\
</d:ProbeMatch></d:ProbeMatches>";
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        parse_probe_matches(xml, &mut out, &mut seen);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].device_url, "http://192.168.0.2/onvif/device_service");
        assert_eq!(out[0].address.as_deref(), Some("192.168.0.2"));
        assert_eq!(out[0].scopes.len(), 2);
        assert_eq!(out[0].endpoint_reference.as_deref(), Some("urn:uuid:abc"));
    }

    #[test]
    fn injects_creds_into_stream_uri() {
        assert_eq!(
            inject_creds(
                "rtsp://10.0.0.2:554/Streaming/101",
                Some("admin"),
                Some("p@ss")
            ),
            "rtsp://admin:p%40ss@10.0.0.2:554/Streaming/101"
        );
        // Already has userinfo: unchanged.
        assert_eq!(
            inject_creds("rtsp://u:p@10.0.0.2/s", Some("admin"), Some("x")),
            "rtsp://u:p@10.0.0.2/s"
        );
        // No username: unchanged.
        assert_eq!(
            inject_creds("rtsp://10.0.0.2/s", None, None),
            "rtsp://10.0.0.2/s"
        );
    }

    #[test]
    fn fault_reason_extracted() {
        let xml = "<s:Fault><s:Reason><s:Text>Sender not authorized</s:Text></s:Reason></s:Fault>";
        assert_eq!(fault_reason(xml).as_deref(), Some("Sender not authorized"));
    }

    #[test]
    fn host_of_strips_everything() {
        assert_eq!(
            host_of("http://192.168.0.2/onvif/x").as_deref(),
            Some("192.168.0.2")
        );
        assert_eq!(
            host_of("http://u:p@10.0.0.5:8000/x").as_deref(),
            Some("10.0.0.5")
        );
    }
}
