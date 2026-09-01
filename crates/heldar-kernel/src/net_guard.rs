//! Shared SSRF egress guard for every server-initiated outbound HTTP request.
//!
//! Heldar makes outbound requests from many places — webhook delivery, the plugin registry fetcher,
//! ONVIF probes, sidecar reverse-proxy and health checks. Historically only the registry fetcher
//! validated its target and disabled redirects; the other sinks let an authenticated operator point
//! the box at internal services or the cloud metadata endpoint (169.254.169.254), with HTTP
//! redirect-following turning even a scheme check into a bypass. This module is the single guard they
//! all share.
//!
//! The policy is deliberately deployment-aware. A Heldar box is usually a LAN appliance whose
//! legitimate targets ARE private/loopback addresses (cameras on the LAN, the local MediaMTX API, a
//! sidecar plugin on `127.0.0.1`), so blanket-rejecting private ranges would break core features.
//! What is *never* a legitimate egress target on any deployment is the link-local range (which is
//! where the cloud metadata service lives) or the unspecified/broadcast addresses — those are
//! rejected regardless of policy. Loopback + RFC1918/ULA are gated behind [`EgressPolicy::allow_lan`],
//! which cloud-only egress (e.g. a hosted control-plane) sets to `false`.
//!
//! `allow_lan == false` is an ALLOWLIST, not a deny-list: the target must be positively classified as
//! globally routable unicast ([`is_globally_routable`]). Spelling it as "not loopback and not RFC1918"
//! silently admitted whole non-public classes — CGNAT/shared space (100.64.0.0/10), multicast, the
//! benchmarking and documentation ranges, 240/4 — each of which can reach a real host on some networks.
//!
//! DNS is validated *and pinned*, not out of scope. [`validate_egress_url`] stays the cheap
//! store-time UX check (it inspects only literal-IP hosts and never resolves), but every actual
//! outbound sink calls [`resolve_validate_pin`] right before it sends: that resolves ALL of the host's
//! A/AAAA records, FAIL-CLOSED rejects if the name resolves to nothing or if *any* resolved address is
//! forbidden under the policy (checking every record — not just the one that happens to be dialed —
//! defeats round-robin DNS rebinding), and returns a client PINNED to those validated addresses so the
//! IP that was checked is the exact IP that gets connected to (closing the TOCTOU re-resolution window).
//! Redirect-following stays disabled on every egress client as a second layer.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

/// How strict to be about the egress target for a given sink.
#[derive(Debug, Clone, Copy)]
pub struct EgressPolicy {
    /// Permit loopback and RFC1918/ULA targets. `true` for a LAN appliance sink that legitimately
    /// reaches cameras / the local MediaMTX / a localhost sidecar; `false` for cloud-only egress.
    /// Link-local (cloud metadata) and the unspecified/broadcast addresses are rejected either way.
    pub allow_lan: bool,
    /// Reject `http://` (require TLS). LAN sinks leave this `false` (plaintext cameras/automation are
    /// normal on a trusted LAN); public-internet sinks set it `true`.
    pub require_https: bool,
}

impl EgressPolicy {
    /// A LAN-appliance sink: permit private/loopback targets over http or https, but still reject the
    /// metadata/link-local and unspecified/broadcast ranges.
    pub const LAN: EgressPolicy = EgressPolicy {
        allow_lan: true,
        require_https: false,
    };

    /// A public-internet sink: https only, and the target must be a GLOBALLY ROUTABLE unicast address
    /// (see [`is_globally_routable`]) — not merely "not RFC1918".
    pub const PUBLIC: EgressPolicy = EgressPolicy {
        allow_lan: false,
        require_https: true,
    };
}

/// Validate a server-initiated egress URL against `policy`, returning the parsed URL on success.
///
/// Checks the scheme allowlist (`http`/`https`, `http` only when `!require_https`) and, when the host
/// is a literal IP, rejects the forbidden ranges. Hostnames are accepted here and resolved later —
/// always pair this with [`resolve_validate_pin`] at send time, which is what actually protects the
/// connection.
pub fn validate_egress_url(url: &str, policy: &EgressPolicy) -> Result<reqwest::Url, String> {
    let parsed = reqwest::Url::parse(url.trim()).map_err(|e| format!("bad url: {e}"))?;
    match parsed.scheme() {
        "https" => {}
        "http" if !policy.require_https => {}
        "http" => return Err("url must be https".into()),
        s => return Err(format!("unsupported url scheme `{s}`")),
    }
    let Some(host) = parsed.host_str() else {
        return Err("url has no host".into());
    };
    // `url` returns IPv6 literals bracketed (e.g. `[::1]`); strip the brackets so the literal-IP guard
    // fires for the v6 family too.
    let host_ip = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(ip) = host_ip.parse::<IpAddr>() {
        reject_forbidden_ip(ip, policy)?;
    }
    Ok(parsed)
}

/// Reject an IP that is a forbidden egress target under `policy`. v4-mapped/compat v6
/// (`::ffff:169.254.169.254`) is canonicalized to v4 first so it can't smuggle a forbidden v4 past
/// the v6 arm.
pub fn reject_forbidden_ip(ip: IpAddr, policy: &EgressPolicy) -> Result<(), String> {
    let ip = canonicalize(ip);
    // Never a legitimate target on any deployment: link-local (169.254/16 incl. the cloud metadata
    // endpoint 169.254.169.254; fe80::/10) and the unspecified/broadcast addresses.
    let always_forbidden = match ip {
        IpAddr::V4(v4) => v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast(),
        IpAddr::V6(v6) => v6.is_unicast_link_local() || v6.is_unspecified(),
    };
    if always_forbidden {
        return Err(format!(
            "{ip} is a link-local/metadata or unspecified address and is never a valid egress target"
        ));
    }
    // `allow_lan == false` means "public internet only". That is an ALLOWLIST of globally routable
    // unicast addresses, not a deny-list of the private ranges: the old `is_loopback() || is_private()`
    // test let CGNAT/shared space, multicast, the benchmarking and documentation blocks and the whole
    // 240/4 reserved range through as if they were public.
    if !policy.allow_lan && !is_globally_routable(ip) {
        return Err(format!(
            "{ip} is not a globally routable public address; only public targets are allowed"
        ));
    }
    Ok(())
}

/// Is `ip` a globally routable unicast address — i.e. can it legitimately be a public-internet peer?
///
/// This is the allowlist [`EgressPolicy::PUBLIC`] enforces: anything not positively classified as
/// globally routable is rejected. `Ipv4Addr::is_global`/`Ipv6Addr::is_global` express exactly this but
/// are UNSTABLE on stable Rust (and this crate builds on stable with `-D warnings`), so every class is
/// spelled out below against the IANA special-purpose address registries (RFC 6890 and successors).
///
/// Note this says nothing about [`EgressPolicy::LAN`] sinks — those deliberately reach RFC1918/loopback
/// cameras and localhost sidecars and never consult this function.
pub fn is_globally_routable(ip: IpAddr) -> bool {
    match canonicalize(ip) {
        IpAddr::V4(v4) => is_globally_routable_v4(v4),
        IpAddr::V6(v6) => is_globally_routable_v6(v6),
    }
}

/// IPv4 has no single contiguous "global unicast" block, so the allowlist is expressed as "belongs to
/// none of the IANA special-purpose registries". Every entry that can reach a non-public destination is
/// enumerated; anything left over is a routable public address.
fn is_globally_routable_v4(v4: Ipv4Addr) -> bool {
    let [a, b, c, _d] = v4.octets();
    let special_purpose = v4.is_loopback()                 // 127.0.0.0/8
        || v4.is_private()                                 // 10/8, 172.16/12, 192.168/16
        || v4.is_link_local()                              // 169.254.0.0/16 (cloud metadata)
        || v4.is_multicast()                               // 224.0.0.0/4
        || v4.is_broadcast()                               // 255.255.255.255
        || a == 0                                          // 0.0.0.0/8 "this network" (incl. unspecified)
        || (a == 100 && (64..=127).contains(&b))           // 100.64.0.0/10 shared address space (CGNAT)
        || (a == 192 && b == 0 && c == 0)                  // 192.0.0.0/24 IETF protocol assignments
        || (a == 192 && b == 0 && c == 2)                  // 192.0.2.0/24 documentation (TEST-NET-1)
        || (a == 198 && b == 51 && c == 100)               // 198.51.100.0/24 documentation (TEST-NET-2)
        || (a == 203 && b == 0 && c == 113)                // 203.0.113.0/24 documentation (TEST-NET-3)
        || (a == 198 && (b == 18 || b == 19))              // 198.18.0.0/15 benchmarking
        || (a == 192 && b == 88 && c == 99)                // 192.88.99.0/24 6to4 relay anycast (deprecated)
        || a >= 240; // 240.0.0.0/4 reserved (incl. 255.0.0.0/8)
    !special_purpose
}

/// IPv6 *does* have one global unicast block — 2000::/3 — so the allowlist is a positive prefix test,
/// which rejects `::/8` (loopback + unspecified + v4-compatible), `0100::/8` (discard-only),
/// `64:ff9b::/96` (NAT64), `fc00::/7` (unique-local), `fe80::/10` (link-local) and `ff00::/8`
/// (multicast) by construction. Only the special-purpose carve-outs *inside* 2000::/3 need naming.
fn is_globally_routable_v6(v6: Ipv6Addr) -> bool {
    let s = v6.segments();
    if s[0] & 0xe000 != 0x2000 {
        return false; // outside global unicast 2000::/3
    }
    let carved_out = match s[0] {
        // 2001::/23 IETF protocol assignments (Teredo 2001::/32, ORCHIDv2 2001:20::/28, AMT,
        // AS112-v6, the PCP/TURN anycasts) plus 2001:db8::/32 documentation.
        0x2001 => s[1] < 0x0200 || s[1] == 0x0db8,
        // 6to4 (RFC 7526-deprecated) embeds an arbitrary IPv4 address in the prefix, so a 2002::/16
        // target is a way to smuggle an RFC1918/metadata v4 destination past a v6 check on any host
        // with a 6to4 tunnel. Never a legitimate public target today.
        0x2002 => true,
        // 3fff::/20 documentation (RFC 9637).
        0x3fff => s[1] & 0xf000 == 0,
        _ => false,
    };
    !carved_out
}

/// v4-mapped/compat IPv6 (`::ffff:169.254.169.254`) collapses to its IPv4 form so it cannot smuggle a
/// forbidden v4 address past the v6 arm of a classification.
fn canonicalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    }
}

/// Validate a slice of already-resolved socket addresses against `policy`. Pure (does no DNS itself), so
/// the resolve-and-validate logic is unit-testable without touching the network. FAIL-CLOSED: an EMPTY
/// slice is rejected (a host that resolves to nothing must not fall through to a connection), and the
/// first forbidden address rejects the whole set. Rejecting when *any* resolved address is forbidden —
/// not just the one that happens to be dialed — is what defeats round-robin DNS rebinding, where only
/// some of a name's A records point at a forbidden target.
pub fn validate_resolved_addrs(addrs: &[SocketAddr], policy: &EgressPolicy) -> Result<(), String> {
    if addrs.is_empty() {
        return Err("host did not resolve to any address".into());
    }
    for addr in addrs {
        reject_forbidden_ip(addr.ip(), policy)?;
    }
    Ok(())
}

/// Resolve ALL A/AAAA records for `host:port` and validate every one against `policy`, returning the
/// validated addresses. FAIL-CLOSED: a resolution error, zero addresses, or ANY forbidden address is an
/// error. The returned addresses are meant to be PINNED into the egress client (see
/// [`pinned_egress_client`]) so the IP that was validated is the exact IP that gets dialed.
pub async fn resolve_and_validate(
    host: &str,
    port: u16,
    policy: &EgressPolicy,
) -> Result<Vec<SocketAddr>, String> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| format!("could not resolve host `{host}`: {e}"))?
        .collect();
    validate_resolved_addrs(&addrs, policy)?;
    Ok(addrs)
}

/// Build an egress client (redirects disabled, `timeout`) that PINS DNS for `host` to `addrs`. Pinning
/// makes reqwest dial only the (already-validated) addresses instead of re-resolving `host` at connect
/// time, closing the TOCTOU window a plain resolve-then-connect leaves open (a name could rebind to a
/// forbidden IP between the check and the connection). The URL keeps `host` as its name, so TLS SNI and
/// the `Host` header stay correct.
///
/// FAILS CLOSED. This used to end in `.unwrap_or_default()`: if the builder ever failed, the caller
/// silently received a `reqwest::Client::default()` that has NEITHER the redirect-none policy NOR the
/// DNS pin — i.e. the exact client the guard exists to prevent, handed out at the moment the guard
/// broke. A build failure is now an error the caller must handle.
pub fn pinned_egress_client(
    timeout: Duration,
    host: &str,
    addrs: &[SocketAddr],
) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(host, addrs)
        .build()
        .map_err(|e| format!("could not build a pinned egress client for `{host}`: {e}"))
}

/// Request-time egress guard: resolve `url`'s host, reject if it resolves to nothing or to any forbidden
/// address under `policy`, and return an egress client PINNED to the validated addresses. This is what
/// actually protects an outbound connection; [`validate_egress_url`] is only the cheap store-time UX
/// check (it never resolves DNS). Every server-initiated sink calls this immediately before it sends,
/// then issues the request through the returned client against the original `url`.
pub async fn resolve_validate_pin(
    url: &reqwest::Url,
    policy: &EgressPolicy,
    timeout: Duration,
) -> Result<reqwest::Client, String> {
    let host = url
        .host_str()
        .ok_or_else(|| "url has no host".to_string())?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "url has no known port".to_string())?;
    // `url` brackets IPv6 literals (`[::1]`); strip them for both the DNS lookup and the pin key.
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    let addrs = resolve_and_validate(host, port, policy).await?;
    pinned_egress_client(timeout, host, &addrs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejected(url: &str, policy: &EgressPolicy) -> bool {
        validate_egress_url(url, policy).is_err()
    }

    #[test]
    fn metadata_and_linklocal_rejected_under_every_policy() {
        for p in [EgressPolicy::LAN, EgressPolicy::PUBLIC] {
            // IPv4 link-local / cloud metadata.
            assert!(rejected("http://169.254.169.254/latest/meta-data/", &p));
            assert!(rejected("https://169.254.0.1/", &p));
            // v4-mapped metadata must not smuggle past the v6 arm.
            assert!(rejected("https://[::ffff:169.254.169.254]/", &p));
            // IPv6 link-local + unspecified.
            assert!(rejected("https://[fe80::1]/", &p));
            assert!(rejected("https://[::]/", &p));
            assert!(rejected("https://0.0.0.0/", &p));
        }
    }

    #[test]
    fn lan_policy_permits_private_and_loopback_but_not_metadata() {
        let p = EgressPolicy::LAN;
        // A LAN appliance must be able to reach cameras, the local MediaMTX, and localhost sidecars.
        assert!(validate_egress_url("http://192.168.1.50/onvif", &p).is_ok());
        assert!(validate_egress_url("http://10.0.0.5:8080/", &p).is_ok());
        assert!(validate_egress_url("http://127.0.0.1:9997/v3/config", &p).is_ok());
        assert!(validate_egress_url("https://[fd00::1]/", &p).is_ok()); // unique-local
                                                                        // ...but still never the metadata endpoint.
        assert!(rejected("http://169.254.169.254/", &p));
    }

    #[test]
    fn public_policy_rejects_all_private_and_requires_https() {
        let p = EgressPolicy::PUBLIC;
        assert!(rejected("http://example.com/", &p)); // http not allowed
        assert!(rejected("https://127.0.0.1/", &p));
        assert!(rejected("https://10.0.0.5/", &p));
        assert!(rejected("https://192.168.1.1/", &p));
        assert!(rejected("https://[::1]/", &p));
        assert!(rejected("https://2130706433/", &p)); // decimal 127.0.0.1
                                                      // Public hosts / IPs pass.
        assert!(validate_egress_url("https://hooks.example.com/x", &p).is_ok());
        assert!(validate_egress_url("https://8.8.8.8/", &p).is_ok());
    }

    #[test]
    fn validate_egress_url_accepts_hostnames_resolution_deferred() {
        // The cheap store-time check inspects literal IPs only; a hostname passes here and is resolved
        // + validated + pinned later by `resolve_validate_pin` at request time.
        assert!(validate_egress_url("https://internal.example.com/", &EgressPolicy::LAN).is_ok());
    }

    #[test]
    fn rejects_non_http_schemes() {
        assert!(rejected("file:///etc/passwd", &EgressPolicy::LAN));
        assert!(rejected("gopher://x/", &EgressPolicy::LAN));
    }

    #[test]
    fn resolved_addr_set_rejects_any_forbidden_member() {
        // A public host and the round-robin variants of forbidden targets.
        let public: SocketAddr = "93.184.216.34:443".parse().unwrap();
        let metadata: SocketAddr = "169.254.169.254:443".parse().unwrap();
        let loopback: SocketAddr = "127.0.0.1:443".parse().unwrap();

        // An all-public set passes under either policy.
        assert!(validate_resolved_addrs(&[public], &EgressPolicy::PUBLIC).is_ok());
        assert!(validate_resolved_addrs(&[public], &EgressPolicy::LAN).is_ok());

        // A single forbidden member fails the WHOLE set — this is what stops round-robin DNS rebinding
        // where only one of several A records points at a forbidden address.
        assert!(validate_resolved_addrs(&[public, metadata], &EgressPolicy::PUBLIC).is_err());
        assert!(validate_resolved_addrs(&[public, metadata], &EgressPolicy::LAN).is_err());

        // LAN permits loopback, but PUBLIC does not — even mixed with a public record.
        assert!(validate_resolved_addrs(&[loopback], &EgressPolicy::LAN).is_ok());
        assert!(validate_resolved_addrs(&[public, loopback], &EgressPolicy::PUBLIC).is_err());
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("test ip literal")
    }

    /// The PUBLIC allowlist: every non-globally-routable class is rejected, not just RFC1918/loopback.
    /// Each of these passed the old `is_loopback() || is_private()` deny-list.
    #[test]
    fn public_policy_rejects_every_non_global_class() {
        let p = EgressPolicy::PUBLIC;
        let non_global_v4 = [
            "100.64.0.1",      // shared address space / CGNAT, 100.64.0.0/10
            "100.100.50.7",    // ...mid-range
            "100.127.255.254", // ...last usable
            "198.18.0.1",      // benchmarking, 198.18.0.0/15
            "198.19.255.255",  // ...upper half
            "192.0.2.5",       // documentation TEST-NET-1
            "198.51.100.5",    // documentation TEST-NET-2
            "203.0.113.5",     // documentation TEST-NET-3
            "192.0.0.8",       // IETF protocol assignments
            "192.88.99.1",     // deprecated 6to4 relay anycast
            "224.0.0.1",       // multicast
            "239.255.255.250", // ...SSDP
            "240.0.0.1",       // reserved 240/4
            "255.255.255.254", // reserved 255/8 (below the broadcast address)
            "0.1.2.3",         // 0.0.0.0/8 "this network"
            "127.0.0.1",       // loopback (still rejected)
            "10.1.2.3",        // RFC1918 (still rejected)
            "172.20.0.1",      // RFC1918 (still rejected)
        ];
        for a in non_global_v4 {
            assert!(
                reject_forbidden_ip(ip(a), &p).is_err(),
                "{a} must be rejected under PUBLIC"
            );
        }

        let non_global_v6 = [
            "fc00::1",           // unique-local
            "fd12:3456::1",      // ...
            "ff02::1",           // multicast
            "2001:db8::1",       // documentation
            "2001::1",           // IETF protocol assignments (Teredo) 2001::/23
            "2001:20::1",        // ORCHIDv2, inside 2001::/23
            "3fff::1",           // documentation (RFC 9637)
            "2002:c0a8:0101::1", // 6to4 embedding 192.168.1.1
            "100::1",            // discard-only
            "64:ff9b::1",        // NAT64
        ];
        for a in non_global_v6 {
            assert!(
                reject_forbidden_ip(ip(a), &p).is_err(),
                "{a} must be rejected under PUBLIC"
            );
        }

        // ...and genuinely public unicast still passes, in both families.
        for a in ["8.8.8.8", "93.184.216.34", "1.1.1.1", "2606:4700::1111"] {
            assert!(
                reject_forbidden_ip(ip(a), &p).is_ok(),
                "{a} must be allowed under PUBLIC"
            );
        }
    }

    /// The tightened PUBLIC classification must not leak into LAN: a Heldar box's whole job is reaching
    /// RFC1918 cameras, loopback sidecars and the local MediaMTX. Only the metadata/link-local +
    /// unspecified/broadcast set is off-limits under LAN.
    #[test]
    fn lan_policy_unchanged_by_globally_routable_tightening() {
        let p = EgressPolicy::LAN;
        for a in [
            "127.0.0.1",
            "10.0.0.5",
            "172.16.0.9",
            "192.168.1.50",
            "100.64.0.1", // CGNAT: a Tailscale/overlay peer is a legitimate LAN-ish target
            "198.18.0.1", // benchmarking range is reachable on a lab LAN
            "8.8.8.8",    // a public target is fine under LAN too
            "::1",        // v6 loopback
            "fd00::1",    // unique-local
            "2001:db8::1", // documentation range is routable on a lab LAN
        ] {
            assert!(
                reject_forbidden_ip(ip(a), &p).is_ok(),
                "{a} must still be reachable under LAN"
            );
        }
        // Never, under any policy.
        for a in [
            "169.254.169.254",
            "0.0.0.0",
            "255.255.255.255",
            "fe80::1",
            "::",
        ] {
            assert!(
                reject_forbidden_ip(ip(a), &p).is_err(),
                "{a} must be rejected even under LAN"
            );
        }
    }

    /// The classifier itself, independent of policy — including the v4-mapped-v6 canonicalization that
    /// stops `::ffff:100.64.0.1` from being classified as an opaque (and thus "global") v6 address.
    #[test]
    fn globally_routable_classification() {
        assert!(is_globally_routable(ip("8.8.8.8")));
        assert!(is_globally_routable(ip("2606:4700::1111")));
        assert!(!is_globally_routable(ip("100.64.0.1")));
        assert!(!is_globally_routable(ip("::ffff:100.64.0.1")));
        assert!(!is_globally_routable(ip("::ffff:169.254.169.254")));
        // 2000::/3 boundaries: 1fff:: is below it, 4000:: is above it.
        assert!(!is_globally_routable(ip("1fff::1")));
        assert!(!is_globally_routable(ip("4000::1")));
        assert!(is_globally_routable(ip("2000::1")));
        assert!(is_globally_routable(ip("3ffe::1"))); // inside 2000::/3, outside 3fff::/20
    }

    #[test]
    fn empty_resolution_is_rejected_fail_closed() {
        // A host that resolves to nothing must be rejected, never allowed to fall through to a connect.
        assert!(validate_resolved_addrs(&[], &EgressPolicy::LAN).is_err());
        assert!(validate_resolved_addrs(&[], &EgressPolicy::PUBLIC).is_err());
    }
}
