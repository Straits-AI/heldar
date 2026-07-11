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
//! Scope (v1, matching the original registry guard): only *literal-IP* hosts are inspected. A
//! hostname that resolves to a private address (DNS rebinding) is out of scope here; the mitigation
//! that actually closes it is [`egress_client`], which disables redirects — combine the two.

use std::net::IpAddr;
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

    /// A public-internet sink: https only, reject every non-public literal address.
    pub const PUBLIC: EgressPolicy = EgressPolicy {
        allow_lan: false,
        require_https: true,
    };
}

/// Validate a server-initiated egress URL against `policy`, returning the parsed URL on success.
///
/// Checks the scheme allowlist (`http`/`https`, `http` only when `!require_https`) and, when the host
/// is a literal IP, rejects the forbidden ranges. Hostnames are accepted (DNS-rebinding is out of
/// scope — pair with [`egress_client`] so redirects can't reach a forbidden address).
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
    let ip = match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    };
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
    if !policy.allow_lan {
        let is_lan = match ip {
            IpAddr::V4(v4) => v4.is_loopback() || v4.is_private(),
            IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local(),
        };
        if is_lan {
            return Err(format!(
                "{ip} is a private/loopback address; only public targets are allowed"
            ));
        }
    }
    Ok(())
}

/// Build a reqwest client for server-initiated egress with redirect-following DISABLED. Following
/// redirects is the SSRF bypass that defeats a target check (a public host 302s to an internal or
/// metadata URL), so no egress client should follow them. Falls back to a default client only if the
/// builder somehow fails.
pub fn egress_client(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default()
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
    fn hostnames_pass_dns_rebind_out_of_scope() {
        // Literal-IP only; a hostname is accepted (redirects disabled by egress_client close the gap).
        assert!(validate_egress_url("https://internal.example.com/", &EgressPolicy::LAN).is_ok());
    }

    #[test]
    fn rejects_non_http_schemes() {
        assert!(rejected("file:///etc/passwd", &EgressPolicy::LAN));
        assert!(rejected("gopher://x/", &EgressPolicy::LAN));
    }
}
