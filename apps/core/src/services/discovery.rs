//! Network discovery: scan an IPv4 range / CIDR for cameras (open RTSP), guess the vendor from the
//! HTTP server banner, and optionally verify credentials via ffprobe. Optionally auto-registers
//! verified devices (recording disabled by default — the operator enables it deliberately).

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
use crate::util;

/// Cap on how many addresses a single scan may cover (a /22 worth), to bound work.
const MAX_TARGETS: usize = 1024;
const SCAN_CONCURRENCY: usize = 64;

#[derive(Debug, Deserialize)]
pub struct DiscoverOptions {
    /// CIDR ("192.168.0.0/24"), range ("192.168.0.2-192.168.0.12"), single IP, or comma list.
    pub targets: String,
    pub username: Option<String>,
    pub password: Option<String>,
    /// Probe each candidate with ffprobe + credentials to confirm a working stream.
    #[serde(default)]
    pub verify: bool,
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
            // Exclude network + broadcast for normal subnets; include the lot for /31 and /32.
            let (start, end) = if prefix <= 30 {
                (network + 1, broadcast - 1)
            } else {
                (network, broadcast)
            };
            for a in start..=end {
                push(a, &mut out)?;
            }
        } else if let Some((a, b)) = token.split_once('-') {
            let a: Ipv4Addr = a.trim().parse().map_err(|_| format!("bad range start: {a}"))?;
            let b: Ipv4Addr = b.trim().parse().map_err(|_| format!("bad range end: {b}"))?;
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
    if s.contains("hikvision") || b.contains("hikvision") || s == "webserver" || b.contains("/doc/page/login")
    {
        "hikvision".into()
    } else if s.contains("app-webs") || b.contains("dahua") {
        "dahua".into()
    } else {
        "unknown".into()
    }
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

    // 2) Identify (HTTP banner) and optionally verify (ffprobe with credentials).
    let mut devices = Vec::new();
    for (ip, http_open) in candidates {
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

        let mut verified = false;
        let (mut codec, mut width, mut height) = (None, None, None);
        if opts.verify {
            if let (Some(u), Some(p)) = (opts.username.as_deref(), opts.password.as_deref()) {
                let path = if vendor_guess == "dahua" {
                    "/cam/realmonitor?channel=1&subtype=0"
                } else {
                    "/Streaming/Channels/101"
                };
                let url = format!(
                    "rtsp://{}:{}@{}:{}{}",
                    camera_url::encode_userinfo(u),
                    camera_url::encode_userinfo(p),
                    addr,
                    rtsp_port,
                    path
                );
                if let Ok(info) = util::ffprobe_stream(&cfg.ffprobe_bin, &url).await {
                    verified = true;
                    codec = info.codec;
                    width = info.width;
                    height = info.height;
                }
            }
        }

        let suggested_id = format!("cam_{}", addr.replace('.', "_"));
        let already_registered = existing.iter().any(|a| a == &addr);
        devices.push(DiscoveredDevice {
            address: addr,
            rtsp_port,
            rtsp_open: true,
            http_open,
            vendor_guess,
            http_server,
            verified,
            codec,
            width,
            height,
            suggested_id,
            already_registered,
        });
    }
    Ok(devices)
}

/// Register a discovered device as a camera with recording DISABLED (operator enables it later).
/// Returns the new camera id.
pub async fn add_device(
    pool: &SqlitePool,
    device: &DiscoveredDevice,
    username: Option<&str>,
    password: Option<&str>,
) -> sqlx::Result<String> {
    let vendor = if device.vendor_guess == "unknown" {
        "hikvision"
    } else {
        &device.vendor_guess
    };
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO cameras
           (id, name, vendor, address, rtsp_port, username, password, record_stream,
            capabilities, record_enabled, segment_seconds, retention_hours, enabled, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, 'main', ?, 0, 60, 24, 1, ?, ?)",
    )
    .bind(&device.suggested_id)
    .bind(format!("Camera {}", device.address))
    .bind(vendor)
    .bind(&device.address)
    .bind(device.rtsp_port as i64)
    .bind(username)
    .bind(password)
    .bind(SqlxJson(json!({ "discovered": true })))
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
}
