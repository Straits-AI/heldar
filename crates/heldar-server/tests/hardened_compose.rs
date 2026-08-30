//! The hardening overlay must keep hardening.
//!
//! `compose.hardened.yml` is the difference between "a compromised process is contained" and "it
//! owns the host", and nothing else in CI notices if a setting is dropped during an unrelated edit —
//! the stack still boots either way. That is the failure mode worth a test.
//!
//! It asserts the properties, not the file's shape: each service is present with the settings that
//! were actually BOOTED and verified, and the two documented exceptions stay documented rather than
//! quietly becoming three.

use std::path::PathBuf;

fn overlay() -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .join("deploy/compose.hardened.yml");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("reading {}: {e}", p.display()))
}

/// Every service gets the three settings that cost nothing and contain the most.
#[test]
fn every_service_drops_privileges() {
    let y = overlay();
    // `no-new-privileges` is the one that stops a setuid binary inside the image regaining what
    // cap_drop removed, so it belongs on all four regardless of what else they need.
    assert_eq!(
        y.matches("no-new-privileges:true").count(),
        4,
        "every service (mediamtx, core, web, ai) needs no-new-privileges"
    );
    assert_eq!(
        y.matches("cap_drop:").count(),
        4,
        "every service should start from ALL dropped"
    );
    // Unbounded container logs fill the same disk the recordings need.
    assert!(y.contains("max-size:"), "container logs must be bounded");
}

/// The read-only rootfs is the strongest setting here, so the services that DON'T have it must say
/// why — otherwise the next reader assumes it was forgotten and either adds it (breaking boot) or
/// removes it elsewhere (losing the property).
#[test]
fn the_two_exceptions_to_read_only_are_explained() {
    let y = overlay();
    assert_eq!(
        y.matches("read_only: true").count(),
        2,
        "core and web carry read_only; mediamtx and ai are the two documented exceptions, so a \
         change here means one was added or silently dropped"
    );
    // MediaMTX writes a self-signed keypair at startup; the AI worker downloads model weights.
    assert!(
        y.contains("auto.crt"),
        "the mediamtx exception must name the error that caused it"
    );
    assert!(
        y.contains("model weights") || y.contains("crash loop"),
        "the ai exception must say why read_only is absent"
    );
}

/// nginx needs three capabilities back and no more. NET_BIND_SERVICE in particular would be a smell:
/// it listens on :8080, so needing it would mean something moved to a privileged port.
#[test]
fn nginx_gets_the_minimum_capabilities_not_a_blanket_grant() {
    let y = overlay();
    for c in ["CHOWN", "SETUID", "SETGID"] {
        assert!(y.contains(c), "nginx entrypoint needs {c} before it drops to the worker user");
    }
    // Checked against the actual `cap_add:` lines, not the whole file — the comment above the grant
    // mentions NET_BIND_SERVICE by name to explain why it is absent, and a naive substring search
    // matches that prose and fails on the documentation rather than the configuration.
    let granted: String = y
        .lines()
        .filter(|l| l.trim_start().starts_with("cap_add:"))
        .collect();
    assert!(
        !granted.contains("NET_BIND_SERVICE"),
        "web listens on :8080 — granting NET_BIND_SERVICE means something moved to a privileged port"
    );
    assert!(
        !y.contains("privileged: true"),
        "nothing in this stack should run privileged"
    );
}
