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

/// The overlay text for ONE service, so a check binds to the service it names.
///
/// Global `count()` assertions were the first version and they were bypassable three ways: moving
/// `read_only` off `core` and onto `ai` kept the count at 2, and weakening `cap_drop: ["ALL"]` to
/// `["NET_RAW"]` kept its count at 4 while removing the protection entirely.
fn service(y: &str, name: &str) -> String {
    let start = y
        .find(&format!("\n  {name}:\n"))
        .unwrap_or_else(|| panic!("service `{name}` is missing from the overlay"));
    let rest = &y[start + 1..];
    // Up to the next top-level `  <name>:` key, or the end.
    let end = rest
        .match_indices("\n  ")
        .filter(|(i, _)| {
            let line = rest[*i + 1..].lines().next().unwrap_or("");
            line.starts_with("  ") && line.trim_end().ends_with(':') && !line.starts_with("    ")
        })
        .map(|(i, _)| i + 1)
        .find(|i| *i > 1)
        .unwrap_or(rest.len());
    rest[..end].to_string()
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
    // The VALUE, per service. `cap_drop: ["NET_RAW"]` also matches a bare `cap_drop:` count.
    for svc in ["mediamtx", "core", "web", "ai"] {
        assert!(
            service(&y, svc).contains(r#"cap_drop: ["ALL"]"#),
            "{svc} must drop ALL capabilities, not a subset"
        );
    }
    // Unbounded container logs fill the same disk the recordings need.
    assert!(y.contains("max-size:"), "container logs must be bounded");
}

/// The read-only rootfs is the strongest setting here, so the services that DON'T have it must say
/// why — otherwise the next reader assumes it was forgotten and either adds it (breaking boot) or
/// removes it elsewhere (losing the property).
#[test]
fn the_two_exceptions_to_read_only_are_explained() {
    let y = overlay();
    // Bound to the SERVICE. A global count of 2 stays 2 if read_only moves from core to ai, which
    // would quietly hand core a writable rootfs.
    for svc in ["core", "web"] {
        assert!(
            service(&y, svc).contains("read_only: true"),
            "{svc} must have a read-only root filesystem"
        );
    }
    for svc in ["mediamtx", "ai"] {
        assert!(
            !service(&y, svc).contains("read_only: true"),
            "{svc} is a documented exception — if it can now be read-only, update the note too"
        );
    }
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

/// The ceilings the docs advertise must exist. Compose v2 does enforce `deploy.resources.limits`
/// outside Swarm (that "Swarm-only" belief is a stale doc bug, since corrected upstream), so these
/// are real — and previously nothing referenced them at all: deleting every limits block failed no
/// assertion while `docs/PRODUCTION.md` still promised CPU/memory/PID ceilings.
#[test]
fn every_service_has_resource_ceilings() {
    let y = overlay();
    for svc in ["mediamtx", "core", "web", "ai"] {
        let block = service(&y, svc);
        for key in ["cpus:", "memory:", "pids:"] {
            assert!(
                block.contains(key),
                "{svc} has no `{key}` ceiling — a runaway there takes the host with it"
            );
        }
    }
}

/// nginx needs three capabilities back and no more. NET_BIND_SERVICE in particular would be a smell:
/// it listens on :8080, so needing it would mean something moved to a privileged port.
#[test]
fn nginx_gets_the_minimum_capabilities_not_a_blanket_grant() {
    let y = overlay();
    for c in ["CHOWN", "SETUID", "SETGID"] {
        assert!(
            y.contains(c),
            "nginx entrypoint needs {c} before it drops to the worker user"
        );
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
