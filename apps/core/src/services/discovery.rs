//! Network discovery: scan an IPv4 range / CIDR for cameras (open RTSP), then identify each by
//! probing a list of vendor RTSP paths with one or more credential sets (via ffprobe). Vendor-
//! agnostic: HikVision, Dahua, Axis, and generic/ONVIF paths are all tried. Optionally auto-
//! registers verified devices (recording disabled by default).

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::types::Json as SqlxJson;
use sqlx::SqlitePool;
use tokio::net::TcpStream;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::camera_url;
use crate::config::Config;
use crate::util::{self, ProbeInfo};

/// Cap on how many addresses a single scan may cover (a /22 worth), to bound work.
const MAX_TARGETS: usize = 1024;
const SCAN_CONCURRENCY: usize = 64;
/// How many hosts to credential-probe (ffprobe) at once.
const PROBE_CONCURRENCY: usize = 8;
/// Per-attempt ffprobe budget, and a cap on attempts per host (bounds time + lockout risk).
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PROBE_ATTEMPTS: usize = 18;

/// Candidate RTSP stream paths, tagged with the vendor they imply. Tried in order, but paths whose
/// vendor matches the HTTP banner guess are moved to the front.
const PROBE_PATHS: &[(&str, &str)] = &[
    ("hikvision", "/Streaming/Channels/101"),
    ("hikvision", "/Streaming/Channels/102"),
    ("dahua", "/cam/realmonitor?channel=1&subtype=0"),
    ("dahua", "/cam/realmonitor?channel=1&subtype=1"),
    ("axis", "/axis-media/media.amp"),
    ("generic", "/live"),
    ("generic", "/live.sdp"),
    ("generic", "/Streaming/Channels/1"),
    ("generic", "/h264"),
    ("generic", "/11"),
    ("generic", "/stream1"),
    ("generic", "/video1"),
    ("generic", "/media/video1"),
    ("generic", "/ch0_0.h264"),
    ("generic", "/onvif1"),
    ("generic", "/"),
];

/// Well-known default credentials, tried only when `try_default_creds` is set AND the device is not
/// identified as HikVision (HikVision locks out after a few failures — we never brute-force it).
const DEFAULT_CREDS: &[(&str, &str)] = &[
    ("admin", "admin"),
    ("admin", "12345"),
    ("admin", "123456"),
    ("admin", ""),
    ("root", "root"),
    ("root", "admin"),
    ("admin", "9999"),
];

#[derive(Debug, Clone, Deserialize)]
pub struct Credential {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct DiscoverOptions {
    /// CIDR ("192.168.0.0/24"), range ("192.168.0.2-192.168.0.12"), single IP, or comma list.
    pub targets: String,
    /// Single credential (convenience). Combined with `credentials` if both are given.
    pub username: Option<String>,
    pub password: Option<String>,
    /// Additional credential sets to try, in order.
    pub credentials: Option<Vec<Credential>>,
    /// Probe RTSP paths + credentials with ffprobe to confirm a working stream.
    #[serde(default)]
    pub verify: bool,
    /// Also try a built-in default-credentials list (non-HikVision hosts only).
    #[serde(default)]
    pub try_default_creds: bool,
    /// Register verified, not-yet-known devices as cameras (recording disabled by default).
    #[serde(default)]
    pub auto_add: bool,
    pub rtsp_port: Option<u16>,
    pub connect_timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredDevice {
    pub address: String,
    pub rtsp_port: u16,
    pub rtsp_open: bool,
    pub http_open: bool,
    pub vendor_guess: String,
    pub http_server: Option<String>,
    pub verified: bool,
    pub codec: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    /// The RTSP path that verified (e.g. `/Streaming/Channels/101`).
    pub stream_path: Option<String>,
    /// The username that verified (password is never serialized).
    pub matched_username: Option<String>,
    #[serde(skip)]
    pub matched_password: Option<String>,
    pub suggested_id: String,
    pub already_registered: bool,
}

/// Expand a targets spec into a bounded list of IPv4 addresses.
pub fn parse_targets(spec: &str) -> Result<Vec<Ipv4Addr>, String> {
    let mut out: Vec<Ipv4Addr> = Vec::new();
    let push = |a: u32, out: &mut Vec<Ipv4Addr>| -> Result<(), String> {
        if out.len() >= MAX_TARGETS {
            return Err(format!("too many targets (> {MAX_TARGETS})"));
        }
        out.push(Ipv4Addr::from(a));
        Ok(())
    };

    for token in spec.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if let Some((base, prefix)) = token.split_once('/') {
            let base: Ipv4Addr = base.parse().map_err(|_| format!("bad CIDR base: {base}"))?;
            let prefix: u32 = prefix
                .parse()
                .map_err(|_| format!("bad CIDR prefix: {prefix}"))?;
            if prefix > 32 {
                return Err(format!("bad CIDR prefix: {prefix}"));
            }
            let base_u = u32::from(base);
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix)
            };
            let network = base_u & mask;
            let broadcast = network | !mask;
            let (start, end) = if prefix <= 30 {
                (network + 1, broadcast - 1)
            } else {
                (network, broadcast)
            };
            for a in start..=end {
                push(a, &mut out)?;
            }
        } else if let Some((a, b)) = token.split_once('-') {
            let a: Ipv4Addr = a
                .trim()
                .parse()
                .map_err(|_| format!("bad range start: {a}"))?;
            let b: Ipv4Addr = b
                .trim()
                .parse()
                .map_err(|_| format!("bad range end: {b}"))?;
            let (a, b) = (u32::from(a), u32::from(b));
            if b < a {
                return Err("range end precedes start".into());
            }
            for x in a..=b {
                push(x, &mut out)?;
            }
        } else {
            let ip: Ipv4Addr = token.parse().map_err(|_| format!("bad IP: {token}"))?;
            out.push(ip);
        }
    }
    if out.is_empty() {
        return Err("no targets specified".into());
    }
    Ok(out)
}

async fn port_open(ip: Ipv4Addr, port: u16, timeout: Duration) -> bool {
    matches!(
        tokio::time::timeout(timeout, TcpStream::connect((ip, port))).await,
        Ok(Ok(_))
    )
}

fn guess_vendor(server: Option<&str>, body: &str) -> String {
    let s = server.unwrap_or("").to_ascii_lowercase();
    let b = body.to_ascii_lowercase();
    if s.contains("hikvision")
        || b.contains("hikvision")
        || s == "webserver"
        || b.contains("/doc/page/login")
    {
        "hikvision".into()
    } else if s.contains("app-webs") || b.contains("dahua") {
        "dahua".into()
    } else if s.contains("axis") || b.contains("axis") {
        "axis".into()
    } else if b.contains("boa")
        || s.contains("boa")
        || s.contains("hipcam")
        || s.contains("uc-httpd")
    {
        "generic".into()
    } else {
        "unknown".into()
    }
}

/// Build an RTSP URL with (optionally empty) credentials.
fn build_rtsp_url(host: &str, port: u16, user: &str, pass: &str, path: &str) -> String {
    if user.is_empty() {
        format!("rtsp://{host}:{port}{path}")
    } else {
        format!(
            "rtsp://{}:{}@{host}:{port}{path}",
            camera_url::encode_userinfo(user),
            camera_url::encode_userinfo(pass)
        )
    }
}

struct ProbeMatch {
    vendor: String,
    path: String,
    username: String,
    password: String,
    info: ProbeInfo,
}

/// Try credential/path combinations against a host until one yields a readable stream.
async fn probe_host(
    ffprobe_bin: &str,
    host: &str,
    port: u16,
    banner_vendor: &str,
    creds: &[(String, String)],
    try_default_creds: bool,
) -> Option<ProbeMatch> {
    // Vendor-ordered paths: matches for the banner vendor first.
    let mut paths: Vec<(&str, &str)> = PROBE_PATHS.to_vec();
    paths.sort_by_key(|(v, _)| if *v == banner_vendor { 0 } else { 1 });

    // Credential order: provided creds first; default creds only for non-HikVision hosts.
    let mut cred_list: Vec<(String, String)> = creds.to_vec();
    if try_default_creds && banner_vendor != "hikvision" {
        for (u, p) in DEFAULT_CREDS {
            cred_list.push((u.to_string(), p.to_string()));
        }
    }
    if cred_list.is_empty() {
        cred_list.push((String::new(), String::new()));
    }

    let mut attempts = 0usize;
    for (user, pass) in &cred_list {
        for (vendor, path) in &paths {
            if attempts >= MAX_PROBE_ATTEMPTS {
                return None;
            }
            attempts += 1;
            let url = build_rtsp_url(host, port, user, pass, path);
            match tokio::time::timeout(PROBE_TIMEOUT, util::ffprobe_stream(ffprobe_bin, &url)).await
            {
                Ok(Ok(info)) if info.codec.is_some() => {
                    return Some(ProbeMatch {
                        vendor: (*vendor).to_string(),
                        path: (*path).to_string(),
                        username: user.clone(),
                        password: pass.clone(),
                        info,
                    });
                }
                _ => {}
            }
        }
    }
    None
}

pub async fn discover(
    pool: &SqlitePool,
    cfg: &Config,
    http: &reqwest::Client,
    opts: &DiscoverOptions,
) -> Result<Vec<DiscoveredDevice>, String> {
    let ips = parse_targets(&opts.targets)?;
    let rtsp_port = opts.rtsp_port.unwrap_or(554);
    let timeout = Duration::from_millis(opts.connect_timeout_ms.unwrap_or(700));

    let existing: Vec<String> =
        sqlx::query_scalar("SELECT address FROM cameras WHERE address IS NOT NULL")
            .fetch_all(pool)
            .await
            .unwrap_or_default();

    // Assemble the credential list (single convenience cred + explicit list).
    let mut creds: Vec<(String, String)> = Vec::new();
    if let Some(u) = opts.username.as_deref().filter(|s| !s.is_empty()) {
        creds.push((u.to_string(), opts.password.clone().unwrap_or_default()));
    }
    if let Some(list) = &opts.credentials {
        for c in list {
            creds.push((c.username.clone(), c.password.clone()));
        }
    }

    // 1) Bounded-concurrency port scan for open RTSP (and HTTP, for vendor identification).
    let sem = Arc::new(Semaphore::new(SCAN_CONCURRENCY));
    let mut set: JoinSet<(Ipv4Addr, bool, bool)> = JoinSet::new();
    for ip in ips {
        let sem = sem.clone();
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.expect("semaphore");
            let rtsp = port_open(ip, rtsp_port, timeout).await;
            let http = if rtsp {
                port_open(ip, 80, timeout).await
            } else {
                false
            };
            (ip, rtsp, http)
        });
    }
    let mut candidates: Vec<(Ipv4Addr, bool)> = Vec::new();
    while let Some(res) = set.join_next().await {
        if let Ok((ip, rtsp, http)) = res {
            if rtsp {
                candidates.push((ip, http));
            }
        }
    }
    candidates.sort_by_key(|(ip, _)| u32::from(*ip));

    // 2) Identify (HTTP banner) + optionally verify (ffprobe paths x creds), parallel across hosts.
    let probe_sem = Arc::new(Semaphore::new(PROBE_CONCURRENCY));
    let mut probe_set: JoinSet<DiscoveredDevice> = JoinSet::new();
    for (ip, http_open) in candidates {
        let http = http.clone();
        let probe_sem = probe_sem.clone();
        let ffprobe_bin = cfg.ffprobe_bin.clone();
        let creds = creds.clone();
        let verify = opts.verify;
        let try_default = opts.try_default_creds;
        let existing = existing.clone();
        probe_set.spawn(async move {
            let _permit = probe_sem.acquire_owned().await.expect("semaphore");
            let addr = ip.to_string();

            let mut http_server = None;
            let mut vendor_guess = "unknown".to_string();
            if http_open {
                if let Ok(resp) = http
                    .get(format!("http://{addr}/"))
                    .timeout(Duration::from_secs(3))
                    .send()
                    .await
                {
                    let server = resp
                        .headers()
                        .get("server")
                        .and_then(|v| v.to_str().ok())
                        .map(|s| s.to_string());
                    http_server = server.clone();
                    let body = resp.text().await.unwrap_or_default();
                    vendor_guess = guess_vendor(server.as_deref(), &body);
                }
            }

            let mut device = DiscoveredDevice {
                address: addr.clone(),
                rtsp_port,
                rtsp_open: true,
                http_open,
                vendor_guess: vendor_guess.clone(),
                http_server,
                verified: false,
                codec: None,
                width: None,
                height: None,
                stream_path: None,
                matched_username: None,
                matched_password: None,
                suggested_id: format!("cam_{}", addr.replace('.', "_")),
                already_registered: existing.iter().any(|a| a == &addr),
            };

            if verify {
                if let Some(m) = probe_host(
                    &ffprobe_bin,
                    &addr,
                    rtsp_port,
                    &vendor_guess,
                    &creds,
                    try_default,
                )
                .await
                {
                    device.verified = true;
                    // The working path is stronger vendor evidence than the banner.
                    if m.vendor != "generic" {
                        device.vendor_guess = m.vendor;
                    } else if vendor_guess == "unknown" {
                        device.vendor_guess = "generic".into();
                    }
                    device.codec = m.info.codec;
                    device.width = m.info.width;
                    device.height = m.info.height;
                    device.stream_path = Some(m.path);
                    device.matched_username = Some(m.username);
                    device.matched_password = Some(m.password);
                }
            }
            device
        });
    }

    let mut devices = Vec::new();
    while let Some(res) = probe_set.join_next().await {
        if let Ok(d) = res {
            devices.push(d);
        }
    }
    devices.sort_by_key(|d| {
        d.address
            .parse::<Ipv4Addr>()
            .map(u32::from)
            .unwrap_or(u32::MAX)
    });
    Ok(devices)
}

/// Register a discovered device as a camera with recording DISABLED (operator enables it later).
/// HikVision/Dahua use the vendor template (so the sub-stream is derivable); other vendors store the
/// exact discovered RTSP URL. Returns the new camera id.
pub async fn add_device(pool: &SqlitePool, device: &DiscoveredDevice) -> sqlx::Result<String> {
    let vendor = device.vendor_guess.as_str();
    let username = device.matched_username.as_deref();
    let password = device.matched_password.as_deref();

    // For non-template vendors, store the exact URL that verified (path can't be guessed).
    let main_stream_url = if matches!(vendor, "hikvision" | "dahua") {
        None
    } else {
        device.stream_path.as_deref().map(|path| {
            build_rtsp_url(
                &device.address,
                device.rtsp_port,
                username.unwrap_or(""),
                password.unwrap_or(""),
                path,
            )
        })
    };
    let store_vendor = if vendor == "unknown" {
        "generic"
    } else {
        vendor
    };

    let now = Utc::now();
    sqlx::query(
        "INSERT INTO cameras
           (id, name, vendor, address, rtsp_port, username, password, main_stream_url, record_stream,
            capabilities, record_enabled, segment_seconds, retention_hours, enabled, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'main', ?, 0, 60, 24, 1, ?, ?)",
    )
    .bind(&device.suggested_id)
    .bind(format!("Camera {}", device.address))
    .bind(store_vendor)
    .bind(&device.address)
    .bind(device.rtsp_port as i64)
    .bind(username)
    .bind(password)
    .bind(&main_stream_url)
    .bind(SqlxJson(json!({
        "discovered": true,
        "stream_path": device.stream_path,
        "codec": device.codec,
    })))
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO camera_status (camera_id, state, updated_at) VALUES (?, 'unknown', ?)
         ON CONFLICT(camera_id) DO NOTHING",
    )
    .bind(&device.suggested_id)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(device.suggested_id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cidr_excludes_network_and_broadcast() {
        let ips = parse_targets("192.168.0.0/30").unwrap();
        assert_eq!(
            ips,
            vec![
                "192.168.0.1".parse::<Ipv4Addr>().unwrap(),
                "192.168.0.2".parse().unwrap()
            ]
        );
    }

    #[test]
    fn parse_range_and_list() {
        let ips = parse_targets("192.168.0.2-192.168.0.4, 10.0.0.5").unwrap();
        assert_eq!(ips.len(), 4);
        assert_eq!(ips[3], "10.0.0.5".parse::<Ipv4Addr>().unwrap());
    }

    #[test]
    fn parse_rejects_oversized_and_bad() {
        assert!(parse_targets("10.0.0.0/8").is_err());
        assert!(parse_targets("not-an-ip").is_err());
    }

    #[test]
    fn build_rtsp_url_with_and_without_creds() {
        assert_eq!(
            build_rtsp_url("10.0.0.5", 554, "admin", "p@ss", "/live"),
            "rtsp://admin:p%40ss@10.0.0.5:554/live"
        );
        assert_eq!(
            build_rtsp_url("10.0.0.5", 554, "", "", "/live"),
            "rtsp://10.0.0.5:554/live"
        );
    }
}
