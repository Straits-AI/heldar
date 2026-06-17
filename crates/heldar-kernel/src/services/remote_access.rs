//! Remote-access overlay awareness (open kernel platform feature; see `docs/REMOTE-ACCESS.md`).
//!
//! A Heldar deployment is typically behind CGNAT (no inbound port-forward, DDNS useless). The
//! supported way to reach it remotely is a **WireGuard overlay** — Tailscale for personal/dev use,
//! NetBird self-hosted for shipped products — running as an EXTERNAL daemon on the host. The overlay
//! is deliberately *orthogonal* to the media stack: MediaMTX and this API keep serving on their
//! normal ports, now reachable over the overlay's interface by authorized peers. Connections are
//! P2P-first (direct hole-punched / IPv6 when possible) and end-to-end encrypted, so no relay can
//! ever read camera video.
//!
//! The kernel does **not** embed or manage WireGuard — that would duplicate mature daemons. It only
//! *observes* the configured overlay interface and reports whether remote access is currently
//! functional, so the dashboard/health surface can show it without log-diving. This keeps the
//! capability fully open (Apache-2.0) and transport-agnostic.

use serde::Serialize;

use crate::config::Config;

/// Health of the remote-access overlay, surfaced via `/api/v1/system`.
#[derive(Debug, Clone, Serialize)]
pub struct OverlayStatus {
    /// Whether remote access via an overlay is configured at all (else the deployment is LAN-only).
    pub enabled: bool,
    /// `tailscale` | `netbird` | `wireguard` | `none`.
    pub kind: String,
    /// The probed interface (e.g. `tailscale0`), if configured.
    pub iface: Option<String>,
    /// Whether that interface currently exists on the host (i.e. the overlay daemon created it).
    pub present: bool,
    /// Raw `operstate` of the interface, if readable.
    pub operstate: Option<String>,
    /// Whether remote access is considered functional right now.
    pub up: bool,
    /// Human-readable explanation for the dashboard.
    pub note: String,
}

/// A TUN/overlay interface is "up" when it exists and its operstate is `up` or `unknown`. WireGuard
/// and Tailscale TUN devices commonly report `unknown` even when fully functional (the kernel does
/// not track carrier on a point-to-point TUN), so `unknown` must NOT be treated as down.
fn is_up(present: bool, operstate: Option<&str>) -> bool {
    present && matches!(operstate, None | Some("up") | Some("unknown"))
}

/// Probe the configured overlay interface and report remote-access health. Dependency-free: reads
/// `/sys/class/net/<iface>` (Linux), so it is cheap enough to call per `/system` request.
pub fn status(cfg: &Config) -> OverlayStatus {
    if !cfg.overlay_enabled {
        return OverlayStatus {
            enabled: false,
            kind: cfg.overlay_kind.clone(),
            iface: cfg.overlay_iface.clone(),
            present: false,
            operstate: None,
            up: false,
            note: "Remote-access overlay disabled (HELDAR_OVERLAY_ENABLED=false); reachable on the LAN only."
                .into(),
        };
    }
    let Some(iface) = cfg.overlay_iface.clone() else {
        return OverlayStatus {
            enabled: true,
            kind: cfg.overlay_kind.clone(),
            iface: None,
            present: false,
            operstate: None,
            up: false,
            note: "Overlay enabled but HELDAR_OVERLAY_IFACE is unset; cannot determine status."
                .into(),
        };
    };

    let base = std::path::Path::new("/sys/class/net").join(&iface);
    let present = base.exists();
    let operstate = std::fs::read_to_string(base.join("operstate"))
        .ok()
        .map(|s| s.trim().to_string());
    let up = is_up(present, operstate.as_deref());

    let note = if !present {
        format!(
            "Overlay interface '{iface}' not found — is the {} daemon running and connected?",
            cfg.overlay_kind
        )
    } else if up {
        format!(
            "Overlay '{}' up on '{iface}'; deployment reachable to authorized peers (P2P-first, end-to-end encrypted).",
            cfg.overlay_kind
        )
    } else {
        format!(
            "Overlay interface '{iface}' present but not up (operstate={}).",
            operstate.as_deref().unwrap_or("?")
        )
    };

    OverlayStatus {
        enabled: true,
        kind: cfg.overlay_kind.clone(),
        iface: Some(iface),
        present,
        operstate,
        up,
        note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tun_unknown_counts_as_up() {
        // TUN devices report "unknown" when functional — must be treated as up.
        assert!(is_up(true, Some("unknown")));
        assert!(is_up(true, Some("up")));
        assert!(is_up(true, None));
    }

    #[test]
    fn down_or_absent_is_not_up() {
        assert!(!is_up(true, Some("down")));
        assert!(!is_up(false, Some("up")));
        assert!(!is_up(false, None));
    }
}
