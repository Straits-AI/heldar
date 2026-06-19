//! Kernel-managed WireGuard remote access — the off-by-default `wireguard` feature.
//!
//! Unlike [`super::remote_access`] (which only *observes* an external Tailscale/NetBird/wg daemon),
//! this module brings up Heldar's OWN, isolated WireGuard interface for private remote viewing. It is
//! designed to be a guest on the operator's host: it auto-selects an interface name, a /24 subnet, and
//! a UDP port that do NOT collide with anything already present (the LAN, Docker bridges, other
//! WireGuard tunnels), it touches ONLY the interface it creates plus one route scoped to its own
//! subnet, and it never reads or writes any pre-existing interface, the default route, or DNS.
//!
//! Privilege: managing a WireGuard device needs `CAP_NET_ADMIN`. The binary is expected to carry that
//! capability (`setcap cap_net_admin,cap_net_raw+eip`); `run` raises it into the ambient set so the
//! `ip`/`wg` children inherit it. With no capability the manager logs and parks — it never
//! falls back to anything that could disturb host networking.
//!
//! Dependency-free by design: it shells out to `ip` and `wg` (the codebase already drives ffmpeg /
//! rclone / mediamtx the same way), so the `wireguard` feature pulls in no extra crates.

use std::net::Ipv4Addr;

// ============================ pure allocation logic (unit-tested) ============================

/// A CIDR as (network address, prefix length), normalized so host bits are zero.
type Cidr = (u32, u8);

/// Mask for a prefix length (`/0` → 0, `/32` → all ones).
fn mask(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix as u32)
    }
}

/// Two CIDRs overlap iff they agree on the shorter of the two prefixes.
fn overlaps(a: Cidr, b: Cidr) -> bool {
    let p = a.1.min(b.1);
    let m = mask(p);
    (a.0 & m) == (b.0 & m)
}

/// Parse an IPv4 `a.b.c.d/p` (or bare address → /32) into a normalized CIDR. `None` on garbage / IPv6.
fn parse_cidr(token: &str) -> Option<Cidr> {
    let (addr, prefix) = match token.split_once('/') {
        Some((a, p)) => (a, p.parse::<u8>().ok()?),
        None => (token, 32),
    };
    if prefix > 32 {
        return None;
    }
    let ip: Ipv4Addr = addr.parse().ok()?;
    let raw = u32::from(ip);
    Some((raw & mask(prefix), prefix)) // normalize host bits to zero
}

/// Extract every IPv4 CIDR in use from the output of `ip -o -4 addr show` and `ip -o -4 route show`.
/// Tolerant line scanning: in `addr` output the CIDR follows the `inet` token; in `route` output the
/// destination is the first token (skipping `default`). Anything unparseable is ignored.
fn parse_inuse(addr_out: &str, route_out: &str) -> Vec<Cidr> {
    let mut out = Vec::new();
    for line in addr_out.lines() {
        let mut toks = line.split_whitespace();
        while let Some(t) = toks.next() {
            if t == "inet" {
                if let Some(c) = toks.next().and_then(parse_cidr) {
                    out.push(c);
                }
            }
        }
    }
    for line in route_out.lines() {
        if let Some(first) = line.split_whitespace().next() {
            if first != "default" {
                if let Some(c) = parse_cidr(first) {
                    out.push(c);
                }
            }
        }
    }
    out
}

/// Pick a free `/24` for the managed interface, skipping every in-use CIDR. Scans a large pool in the
/// less-common upper `10.x.0.0/24` range (WireGuard convention is 10.x; the upper octets are unlikely
/// to clash with a LAN's `10.0.x`/`10.1.x` or a Docker `172.x`). Returns the network address of a free
/// `/24`, or `None` if the whole pool is somehow occupied.
fn pick_subnet(inuse: &[Cidr]) -> Option<u32> {
    for second in 200u32..=254 {
        let cand = (10u32 << 24) | (second << 16); // 10.<second>.0.0
        if !inuse.iter().any(|&c| overlaps((cand, 24), c)) {
            return Some(cand);
        }
    }
    None
}

/// Pick a free interface name `heldar<N>` not already present. `existing` is the set of link names.
fn pick_iface(existing: &[String]) -> String {
    (0..)
        .map(|n| format!("heldar{n}"))
        .find(|name| !existing.iter().any(|e| e == name))
        .expect("0.. is unbounded")
}

/// The host address (`.1`) and a sample peer address (`.2`) for a chosen `/24` network.
fn host_and_first_peer(net24: u32) -> (Ipv4Addr, Ipv4Addr) {
    (Ipv4Addr::from(net24 | 1), Ipv4Addr::from(net24 | 2))
}

/// Lowest free peer host address in a `/24` (`.2`..`.254`, reserving `.1` for the host and `.255`
/// broadcast), given the host octets already assigned to peers. `None` when the /24 is full.
fn next_peer_ip(net24: u32, used_octets: &[u8]) -> Option<Ipv4Addr> {
    (2u8..=254)
        .find(|o| !used_octets.contains(o))
        .map(|o| Ipv4Addr::from(net24 | o as u32))
}

// ============================ host I/O + privileged manager ============================

use std::process::Command;

use serde::Serialize;

use crate::config::Config;

/// The concrete, collision-checked parameters of the managed interface.
#[derive(Debug, Clone, Serialize)]
pub struct Resolved {
    pub iface: String,
    pub subnet: String,
    pub host_ip: String,
    pub port: u16,
    /// `host:port` advertised to peers (bracketed if IPv6).
    pub endpoint: String,
    net24: u32,
}

/// Status surfaced to the dashboard / `/api/v1/system`.
#[derive(Debug, Clone, Serialize)]
pub struct WgStatus {
    pub managed: bool,
    pub iface: Option<String>,
    pub subnet: Option<String>,
    pub port: Option<u16>,
    pub endpoint: Option<String>,
    pub present: bool,
    pub up: bool,
    pub peers: usize,
    pub note: String,
}

/// A peer as enrolled — the `.conf` is what the remote device imports (also rendered as a QR client-side).
#[derive(Debug, Clone, Serialize)]
pub struct EnrolledPeer {
    pub name: String,
    pub public_key: String,
    pub address: String,
    pub config: String,
}

/// A peer as currently known to the kernel device (no secrets).
#[derive(Debug, Clone, Serialize)]
pub struct PeerInfo {
    pub name: String,
    pub public_key: String,
    pub address: String,
    /// Unix seconds of the last handshake, 0 if never (i.e. not yet connected).
    pub last_handshake: i64,
}

/// Run a privileged `ip`/`wg` command. Before exec, the child raises `CAP_NET_ADMIN` into its ambient
/// set (best-effort) so a `setcap cap_net_admin+eip` on this binary is inherited across the exec. If
/// the capability isn't held the command simply fails with EPERM, which we surface as a clear error.
fn run(program: &str, args: &[&str]) -> anyhow::Result<String> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    #[cfg(target_os = "linux")]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            // PR_CAP_AMBIENT_RAISE for CAP_NET_ADMIN (12). Ignore failure: when running as root no
            // ambient cap is needed, and when the cap isn't permitted the spawned tool errors clearly.
            libc::prctl(
                libc::PR_CAP_AMBIENT,
                libc::PR_CAP_AMBIENT_RAISE as libc::c_ulong,
                12 as libc::c_ulong, // CAP_NET_ADMIN
                0,
                0,
            );
            Ok(())
        });
    }
    let out = cmd
        .output()
        .map_err(|e| anyhow::anyhow!("spawning `{program}`: {e} (is it installed?)"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("`{program} {}` failed: {}", args.join(" "), stderr.trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `ip` that tolerates an idempotent "exists" (re-running bring-up must not error).
fn run_idempotent(program: &str, args: &[&str]) -> anyhow::Result<()> {
    match run(program, args) {
        Ok(_) => Ok(()),
        Err(e) if e.to_string().contains("exists") || e.to_string().contains("File exists") => {
            Ok(())
        }
        Err(e) => Err(e),
    }
}

fn inuse_cidrs() -> Vec<Cidr> {
    let addr = run("ip", &["-o", "-4", "addr", "show"]).unwrap_or_default();
    let route = run("ip", &["-o", "-4", "route", "show"]).unwrap_or_default();
    parse_inuse(&addr, &route)
}

fn existing_ifaces() -> Vec<String> {
    // `ip -o link show` lines: "N: name: <flags> ..." — take the second whitespace field, drop the colon.
    run("ip", &["-o", "link", "show"])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            l.split_whitespace()
                .nth(1)
                .map(|s| s.trim_end_matches(':').to_string())
        })
        .map(|s| s.split('@').next().unwrap_or(&s).to_string()) // strip "@parent" on virtual links
        .collect()
}

/// Auto-detect the host's global IPv6 (skipping link-local `fe80::` and ULA `fc00::/7`).
fn detect_ipv6() -> Option<String> {
    let out = run("ip", &["-o", "-6", "addr", "show", "scope", "global"]).ok()?;
    for line in out.lines() {
        let mut toks = line.split_whitespace();
        while let Some(t) = toks.next() {
            if t == "inet6" {
                if let Some(cidr) = toks.next() {
                    let addr = cidr.split('/').next().unwrap_or(cidr);
                    let lower = addr.to_ascii_lowercase();
                    if !lower.starts_with("fe80")
                        && !lower.starts_with("fd")
                        && !lower.starts_with("fc")
                    {
                        return Some(addr.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Find a bindable UDP port at/above `start` (best-effort free-port probe; WireGuard's in-kernel
/// socket isn't visible to a userspace bind, so a configured override always wins).
fn free_udp_port(start: u16) -> u16 {
    for p in start..start.saturating_add(200) {
        if std::net::UdpSocket::bind(("0.0.0.0", p)).is_ok() {
            return p;
        }
    }
    start
}

/// Resolve the managed interface parameters: honor any `HELDAR_WG_*` override, else auto-select a
/// name / subnet / port / endpoint that does not collide with anything already on the host.
pub fn resolve(cfg: &Config) -> anyhow::Result<Resolved> {
    let iface = cfg
        .wg_iface
        .clone()
        .unwrap_or_else(|| pick_iface(&existing_ifaces()));
    let net24 = match &cfg.wg_subnet {
        Some(s) => parse_cidr(s)
            .map(|(n, _)| n)
            .ok_or_else(|| anyhow::anyhow!("bad HELDAR_WG_SUBNET: {s}"))?,
        None => {
            pick_subnet(&inuse_cidrs()).ok_or_else(|| anyhow::anyhow!("no free /24 to allocate"))?
        }
    };
    let (host_ip, _) = host_and_first_peer(net24);
    let port = cfg.wg_port.unwrap_or_else(|| free_udp_port(51820));
    let endpoint = match &cfg.wg_endpoint {
        Some(e) => e.clone(),
        None => {
            let host = detect_ipv6().ok_or_else(|| {
                anyhow::anyhow!(
                    "no global IPv6 found; set HELDAR_WG_ENDPOINT to a reachable host:port"
                )
            })?;
            format!("[{host}]:{port}")
        }
    };
    Ok(Resolved {
        iface,
        subnet: format!("{}/24", Ipv4Addr::from(net24)),
        host_ip: host_ip.to_string(),
        port,
        endpoint,
        net24,
    })
}

fn key_dir(cfg: &Config) -> std::path::PathBuf {
    cfg.data_dir.join("wireguard")
}

/// Ensure the host keypair exists (private key persisted 0600); returns (private_key, public_key).
fn ensure_host_keys(cfg: &Config, iface: &str) -> anyhow::Result<(String, String)> {
    let dir = key_dir(cfg);
    std::fs::create_dir_all(&dir)?;
    let key_path = dir.join(format!("{iface}.key"));
    let priv_key = if key_path.exists() {
        std::fs::read_to_string(&key_path)?.trim().to_string()
    } else {
        let k = run("wg", &["genkey"])?.trim().to_string();
        std::fs::write(&key_path, format!("{k}\n"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
        }
        k
    };
    let public = pubkey_of(&priv_key)?;
    Ok((priv_key, public))
}

/// Derive a public key from a private key via `wg pubkey` (stdin).
fn pubkey_of(private: &str) -> anyhow::Result<String> {
    use std::io::Write;
    let mut child = Command::new("wg")
        .arg("pubkey")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(format!("{private}\n").as_bytes())?;
    let out = child.wait_with_output()?;
    anyhow::ensure!(out.status.success(), "wg pubkey failed");
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn names_path(cfg: &Config, iface: &str) -> std::path::PathBuf {
    key_dir(cfg).join(format!("{iface}.peers.json"))
}

/// Load the (public_key → friendly name) map; missing/garbage → empty.
fn load_names(cfg: &Config, iface: &str) -> std::collections::HashMap<String, String> {
    std::fs::read_to_string(names_path(cfg, iface))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_names(
    cfg: &Config,
    iface: &str,
    map: &std::collections::HashMap<String, String>,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(key_dir(cfg))?;
    std::fs::write(names_path(cfg, iface), serde_json::to_string_pretty(map)?)?;
    Ok(())
}

/// Bring the managed interface up (idempotent). Creates ONLY this interface; the `/24` scope route is
/// added by the kernel with the address and is bound to this interface — the default route, `wg0`, and
/// every other interface are untouched.
pub fn ensure_up(cfg: &Config) -> anyhow::Result<Resolved> {
    let r = resolve(cfg)?;
    let (_priv, _pub) = ensure_host_keys(cfg, &r.iface)?;
    let key_path = key_dir(cfg).join(format!("{}.key", r.iface));
    let key_path = key_path.to_string_lossy().into_owned();

    run_idempotent("ip", &["link", "add", "dev", &r.iface, "type", "wireguard"])?;
    run(
        "wg",
        &[
            "set",
            &r.iface,
            "listen-port",
            &r.port.to_string(),
            "private-key",
            &key_path,
        ],
    )?;
    run_idempotent(
        "ip",
        &[
            "address",
            "add",
            &format!("{}/24", r.host_ip),
            "dev",
            &r.iface,
        ],
    )?;
    run("ip", &["link", "set", "up", "dev", &r.iface])?;
    tracing::info!(iface = %r.iface, subnet = %r.subnet, port = r.port, endpoint = %r.endpoint, "managed WireGuard up");
    Ok(r)
}

/// Tear down ONLY the managed interface (removes its addresses + scope route with it). No-op if absent.
pub fn teardown(cfg: &Config) -> anyhow::Result<()> {
    let iface = cfg
        .wg_iface
        .clone()
        .unwrap_or_else(|| pick_iface(&existing_ifaces()));
    if iface_present(&iface) {
        run("ip", &["link", "del", "dev", &iface])?;
    }
    Ok(())
}

fn iface_present(iface: &str) -> bool {
    std::path::Path::new("/sys/class/net").join(iface).exists()
}

/// Parse `wg show <iface> dump` → (public_key, allowed_ips, last_handshake) per peer (skips the header).
fn dump_peers(iface: &str) -> anyhow::Result<Vec<(String, String, i64)>> {
    let out = run("wg", &["show", iface, "dump"])?;
    Ok(out
        .lines()
        .skip(1) // first line is the interface itself
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            // dump peer columns: pubkey, psk, endpoint, allowed-ips, latest-handshake, rx, tx, keepalive
            if f.len() >= 5 {
                Some((
                    f[0].to_string(),
                    f[3].to_string(),
                    f[4].parse().unwrap_or(0),
                ))
            } else {
                None
            }
        })
        .collect())
}

/// Enroll a new device: generate its keypair, allocate the lowest free address, add it to the kernel
/// device, persist its friendly name, and return the `.conf` for the remote WireGuard client.
pub fn add_peer(cfg: &Config, name: &str) -> anyhow::Result<EnrolledPeer> {
    let r = resolve(cfg)?;
    anyhow::ensure!(iface_present(&r.iface), "interface {} is not up", r.iface);
    let (_hpriv, host_pub) = ensure_host_keys(cfg, &r.iface)?;

    // Allocate the lowest free address from the peers already on the device.
    let used: Vec<u8> = dump_peers(&r.iface)?
        .iter()
        .filter_map(|(_, aips, _)| aips.split('/').next()?.parse::<Ipv4Addr>().ok())
        .map(|ip| ip.octets()[3])
        .collect();
    let peer_ip =
        next_peer_ip(r.net24, &used).ok_or_else(|| anyhow::anyhow!("address pool full"))?;

    let peer_priv = run("wg", &["genkey"])?.trim().to_string();
    let peer_pub = pubkey_of(&peer_priv)?;
    run(
        "wg",
        &[
            "set",
            &r.iface,
            "peer",
            &peer_pub,
            "allowed-ips",
            &format!("{peer_ip}/32"),
        ],
    )?;

    let mut names = load_names(cfg, &r.iface);
    names.insert(peer_pub.clone(), name.to_string());
    save_names(cfg, &r.iface, &names)?;

    // Split-tunnel client config: route ONLY the Heldar host over the tunnel, not all the device's traffic.
    let config = format!(
        "[Interface]\nPrivateKey = {peer_priv}\nAddress = {peer_ip}/32\n\n\
         [Peer]\nPublicKey = {host_pub}\nEndpoint = {endpoint}\nAllowedIPs = {host_ip}/32\nPersistentKeepalive = 25\n",
        endpoint = r.endpoint,
        host_ip = r.host_ip,
    );
    tracing::info!(iface = %r.iface, %name, address = %peer_ip, "enrolled WireGuard peer");
    Ok(EnrolledPeer {
        name: name.to_string(),
        public_key: peer_pub,
        address: peer_ip.to_string(),
        config,
    })
}

/// Remove a peer by its public key.
pub fn remove_peer(cfg: &Config, public_key: &str) -> anyhow::Result<()> {
    let r = resolve(cfg)?;
    run("wg", &["set", &r.iface, "peer", public_key, "remove"])?;
    let mut names = load_names(cfg, &r.iface);
    names.remove(public_key);
    save_names(cfg, &r.iface, &names)?;
    Ok(())
}

/// List enrolled peers (no secrets), joined with their friendly names.
pub fn list_peers(cfg: &Config) -> anyhow::Result<Vec<PeerInfo>> {
    let r = resolve(cfg)?;
    let names = load_names(cfg, &r.iface);
    Ok(dump_peers(&r.iface)?
        .into_iter()
        .map(|(pubkey, aips, hs)| PeerInfo {
            name: names
                .get(&pubkey)
                .cloned()
                .unwrap_or_else(|| "(unnamed)".into()),
            address: aips.split('/').next().unwrap_or(&aips).to_string(),
            public_key: pubkey,
            last_handshake: hs,
        })
        .collect())
}

/// Best-effort status for the dashboard. Never errors — a probe failure becomes a `note`.
pub fn status(cfg: &Config) -> WgStatus {
    if !cfg.wg_managed {
        return WgStatus {
            managed: false,
            iface: None,
            subnet: None,
            port: None,
            endpoint: None,
            present: false,
            up: false,
            peers: 0,
            note: "Kernel-managed WireGuard disabled (HELDAR_WG_MANAGED=false).".into(),
        };
    }
    match resolve(cfg) {
        Ok(r) => {
            let present = iface_present(&r.iface);
            let peers = if present {
                list_peers(cfg).map(|p| p.len()).unwrap_or(0)
            } else {
                0
            };
            let note = if present {
                format!(
                    "Managed WireGuard up on '{}' ({}); endpoint {}; {peers} peer(s).",
                    r.iface, r.subnet, r.endpoint
                )
            } else {
                format!("Managed WireGuard enabled but '{}' is not up — needs CAP_NET_ADMIN (setcap) and boot bring-up.", r.iface)
            };
            WgStatus {
                managed: true,
                iface: Some(r.iface),
                subnet: Some(r.subnet),
                port: Some(r.port),
                endpoint: Some(r.endpoint),
                present,
                up: present,
                peers,
                note,
            }
        }
        Err(e) => WgStatus {
            managed: true,
            iface: None,
            subnet: None,
            port: None,
            endpoint: None,
            present: false,
            up: false,
            peers: 0,
            note: format!("Managed WireGuard cannot resolve parameters: {e}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Cidr {
        parse_cidr(s).unwrap()
    }

    #[test]
    fn cidr_overlap_basic() {
        assert!(overlaps(c("10.88.0.0/24"), c("10.88.0.2/32")));
        assert!(overlaps(c("192.168.0.0/24"), c("192.168.0.128/32")));
        assert!(overlaps(c("172.16.0.0/12"), c("172.19.0.1/32"))); // docker bridge inside the /12
        assert!(!overlaps(c("10.200.0.0/24"), c("10.88.0.0/24")));
        assert!(!overlaps(c("10.200.0.0/24"), c("192.168.0.0/24")));
    }

    #[test]
    fn parse_inuse_from_ip_output() {
        // Real-shaped `ip -o -4 addr` + `ip -o -4 route` fragments from this kind of host.
        let addr = "1: lo    inet 127.0.0.1/8 scope host lo\n\
                    2: eno1    inet 192.168.0.128/24 brd 192.168.0.255 scope global eno1\n\
                    9: wg0    inet 10.88.0.2/24 scope global wg0\n\
                    3: docker0    inet 172.17.0.1/16 brd 172.17.255.255 scope global docker0";
        let route = "default via 192.168.0.1 dev eno1\n\
                     10.88.0.0/24 dev wg0 proto kernel scope link\n\
                     192.168.0.0/24 dev eno1 proto kernel scope link";
        let inuse = parse_inuse(addr, route);
        assert!(inuse.contains(&c("192.168.0.0/24")));
        assert!(inuse.contains(&c("10.88.0.0/24")));
        assert!(inuse.contains(&c("172.17.0.0/16")));
    }

    #[test]
    fn pick_subnet_avoids_everything_inuse() {
        // The host's real subnets, including the user's wg0 at 10.88.0.0/24.
        let inuse = vec![
            c("127.0.0.0/8"),
            c("192.168.0.0/24"),
            c("172.17.0.0/16"),
            c("172.18.0.0/16"),
            c("172.19.0.0/16"),
            c("10.88.0.0/24"),
        ];
        let net = pick_subnet(&inuse).expect("a free /24 exists");
        let chosen: Cidr = (net, 24);
        for u in &inuse {
            assert!(!overlaps(chosen, *u), "chosen {net:#x} overlaps {u:?}");
        }
        // First free candidate is 10.200.0.0/24 (nothing in the 10.200+ range is in use here).
        assert_eq!(net, (10u32 << 24) | (200 << 16));
    }

    #[test]
    fn pick_subnet_steps_past_a_used_candidate() {
        // If 10.200.0.0/24 is taken, the allocator moves to 10.201.0.0/24.
        let inuse = vec![c("10.200.0.0/24")];
        let net = pick_subnet(&inuse).unwrap();
        assert_eq!(net, (10u32 << 24) | (201 << 16));
    }

    #[test]
    fn pick_iface_skips_existing() {
        assert_eq!(pick_iface(&["wg0".into(), "eno1".into()]), "heldar0");
        assert_eq!(
            pick_iface(&["heldar0".into(), "wg0".into()]),
            "heldar1",
            "must not reuse an existing heldar0"
        );
    }

    #[test]
    fn host_and_peer_addrs() {
        let net = (10u32 << 24) | (200 << 16); // 10.200.0.0
        let (host, peer) = host_and_first_peer(net);
        assert_eq!(host, Ipv4Addr::new(10, 200, 0, 1));
        assert_eq!(peer, Ipv4Addr::new(10, 200, 0, 2));
    }

    #[test]
    fn peer_ip_allocation_skips_used() {
        let net = (10u32 << 24) | (200 << 16); // 10.200.0.0
        assert_eq!(next_peer_ip(net, &[]), Some(Ipv4Addr::new(10, 200, 0, 2)));
        assert_eq!(
            next_peer_ip(net, &[2, 3]),
            Some(Ipv4Addr::new(10, 200, 0, 4))
        );
        assert_eq!(next_peer_ip(net, &(2..=254).collect::<Vec<u8>>()), None); // pool full
    }
}
