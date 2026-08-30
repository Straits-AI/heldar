//! The OpenAPI spec must describe the routes actually served.
//!
//! A contract that drifts is worse than none: an integrator generates a client against it and finds
//! out at runtime. So this asserts spec-vs-router coverage the same way the route census asserts
//! scope coverage — every served route is documented, or is on a shrinking allowlist that names it.
//! Adding a route without documenting it fails CI.
//!
//! Route discovery is `heldar_testkit::Census`, not a second scanner: two implementations of "what
//! routes exist" would disagree eventually, and then this test would pass while missing routes.

use std::collections::BTreeSet;
use std::path::PathBuf;

use heldar_kernel::openapi::ApiDoc;
use utoipa::OpenApi;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/heldar-server -> repo root")
        .to_path_buf()
}

/// Served routes not yet in the spec. **This list only shrinks.**
///
/// Each entry is an endpoint an integrator cannot generate a client for. Documenting one means
/// adding `#[utoipa::path]` to its handler, listing it in `ApiDoc`, and deleting the line here.
const NOT_YET_DOCUMENTED: &[&str] = &[
    "/api/v1/ai-tasks/{task_id}",
    "/api/v1/ai/embed-queries",
    "/api/v1/ai/embed-queries/{id}/result",
    "/api/v1/ai/embeddings",
    "/api/v1/ai/events",
    "/api/v1/ai/leases",
    "/api/v1/ai/leases/{lease_id}",
    "/api/v1/ai/samplers",
    "/api/v1/ai/tasks",
    "/api/v1/api-keys",
    "/api/v1/api-keys/{id}",
    "/api/v1/archive/export",
    "/api/v1/archive/exports",
    "/api/v1/audit",
    "/api/v1/auth/login",
    "/api/v1/auth/logout",
    "/api/v1/auth/me",
    "/api/v1/backup/destinations",
    "/api/v1/backup/destinations/{id}",
    "/api/v1/backup/destinations/{id}/test",
    "/api/v1/backup/jobs",
    "/api/v1/backup/jobs/{id}",
    "/api/v1/backup/policies",
    "/api/v1/backup/policies/{id}",
    "/api/v1/backup/policies/{id}/trigger",
    "/api/v1/cameras/config/bulk",
    "/api/v1/cameras/{id}/ai-tasks",
    "/api/v1/cameras/{id}/clip",
    "/api/v1/cameras/{id}/config/device_info",
    "/api/v1/cameras/{id}/config/onvif",
    "/api/v1/cameras/{id}/config/onvif/ensure_user",
    "/api/v1/cameras/{id}/config/osd",
    "/api/v1/cameras/{id}/config/reboot",
    "/api/v1/cameras/{id}/config/time",
    "/api/v1/cameras/{id}/config/time/ntp",
    "/api/v1/cameras/{id}/config/time/sync_now",
    "/api/v1/cameras/{id}/config/video",
    "/api/v1/cameras/{id}/config/video/{channel}",
    "/api/v1/cameras/{id}/control/capabilities",
    "/api/v1/cameras/{id}/control/day_night",
    "/api/v1/cameras/{id}/control/detections/{kind}",
    "/api/v1/cameras/{id}/control/image",
    "/api/v1/cameras/{id}/control/intrusion",
    "/api/v1/cameras/{id}/control/io/outputs",
    "/api/v1/cameras/{id}/control/io/outputs/{port}/pulse",
    "/api/v1/cameras/{id}/control/line_crossing",
    "/api/v1/cameras/{id}/control/motion",
    "/api/v1/cameras/{id}/control/probe",
    "/api/v1/cameras/{id}/detections",
    "/api/v1/cameras/{id}/frame",
    "/api/v1/cameras/{id}/gaps",
    "/api/v1/cameras/{id}/health",
    "/api/v1/cameras/{id}/liveview",
    "/api/v1/cameras/{id}/onvif",
    "/api/v1/cameras/{id}/onvif/probe",
    "/api/v1/cameras/{id}/playback/sessions",
    "/api/v1/cameras/{id}/ptz/continuous",
    "/api/v1/cameras/{id}/ptz/goto_preset",
    "/api/v1/cameras/{id}/ptz/presets",
    "/api/v1/cameras/{id}/ptz/presets/refresh",
    "/api/v1/cameras/{id}/ptz/stop",
    "/api/v1/cameras/{id}/record-trigger",
    "/api/v1/cameras/{id}/recording-gaps",
    "/api/v1/cameras/{id}/recording-gaps/{gap_id}/retry",
    "/api/v1/cameras/{id}/schedules",
    "/api/v1/cameras/{id}/segments",
    "/api/v1/cameras/{id}/snapshot",
    "/api/v1/cameras/{id}/snapshot-schedules",
    "/api/v1/cameras/{id}/snapshots",
    "/api/v1/cameras/{id}/test",
    "/api/v1/cameras/{id}/timeline",
    "/api/v1/cameras/{id}/zone-events",
    "/api/v1/cameras/{id}/zone-events/aggregates",
    "/api/v1/cameras/{id}/zones",
    "/api/v1/cameras/{id}/zones/occupancy",
    "/api/v1/discover",
    "/api/v1/entry-events",
    "/api/v1/entry-events/{id}",
    "/api/v1/entry-events/{id}/confirm",
    "/api/v1/entry-events/{id}/reject",
    "/api/v1/entry/gate",
    "/api/v1/entry/gate/open/{camera_id}",
    "/api/v1/entry/gate/policies/{camera_id}",
    "/api/v1/entry/gate/settings",
    "/api/v1/events",
    "/api/v1/events/types",
    "/api/v1/health/cameras",
    "/api/v1/incidents",
    "/api/v1/incidents/{incident_id}/segments",
    "/api/v1/modules",
    "/api/v1/modules/entry/ui/index.js",
    "/api/v1/modules/movement/ui/index.js",
    "/api/v1/modules/search/ui/index.js",
    "/api/v1/modules/{id}",
    "/api/v1/movement/breaches",
    "/api/v1/movement/breaches/{id}/ack",
    "/api/v1/movement/breaches/{id}/resolve",
    "/api/v1/movement/candidates",
    "/api/v1/movement/candidates/{id}/confirm",
    "/api/v1/movement/candidates/{id}/reject",
    "/api/v1/movement/links",
    "/api/v1/movement/links/{id}",
    "/api/v1/movement/run",
    "/api/v1/movement/search/person",
    "/api/v1/movement/search/plate/{plate}",
    "/api/v1/onvif/discover",
    "/api/v1/openapi.json",
    "/api/v1/outbox",
    "/api/v1/passes",
    "/api/v1/passes/{id}",
    "/api/v1/passes/{id}/checkin",
    "/api/v1/passes/{id}/checkout",
    "/api/v1/playback/sessions/{session_id}",
    "/api/v1/registry",
    "/api/v1/registry/refresh",
    "/api/v1/reports/entry-log",
    "/api/v1/reports/exceptions",
    "/api/v1/schedules/{schedule_id}",
    "/api/v1/search/events",
    "/api/v1/search/nl",
    "/api/v1/search/plan",
    "/api/v1/search/semantic",
    "/api/v1/segments/{id}/evidence-lock",
    "/api/v1/segments/{id}/incident",
    "/api/v1/site",
    "/api/v1/snapshot-schedules/{schedule_id}",
    "/api/v1/system",
    "/api/v1/system/db",
    "/api/v1/system/db/convert",
    "/api/v1/system/retention",
    "/api/v1/system/transcode",
    "/api/v1/users",
    "/api/v1/users/{id}",
    "/api/v1/users/{id}/unlock",
    "/api/v1/vehicles",
    "/api/v1/vehicles/{id}",
    "/api/v1/watchlist",
    "/api/v1/watchlist/{id}",
    "/api/v1/webhooks",
    "/api/v1/webhooks/{id}",
    "/api/v1/webhooks/{id}/deliveries",
    "/api/v1/webhooks/{id}/test",
    "/api/v1/zones/{zone_id}",
];

/// Every `/api/v1` route the box serves is either documented or explicitly named as not yet.
#[test]
fn openapi_covers_every_route() {
    let census = heldar_testkit::Census::new(vec![
        repo_root().join("crates/heldar-kernel/src"),
        repo_root().join("crates/heldar-entry/src"),
        repo_root().join("crates/heldar-movement/src"),
        repo_root().join("crates/heldar-search/src"),
    ]);
    let served: BTreeSet<String> = census
        .discover()
        .into_iter()
        .map(|r| r.path)
        // Only the versioned API is a contract. `/healthz`, `/metrics`, `/media/*` and the MediaMTX
        // callback are operational surfaces with their own stability rules.
        .filter(|p| p.starts_with("/api/v1"))
        .collect();
    assert!(
        served.len() > 80,
        "route discovery found only {} — the scan is broken, not the product",
        served.len()
    );

    let spec = ApiDoc::openapi();
    let documented: BTreeSet<String> = spec.paths.paths.keys().cloned().collect();
    let allowed: BTreeSet<String> = NOT_YET_DOCUMENTED.iter().map(|s| s.to_string()).collect();

    let undocumented: Vec<&String> = served
        .iter()
        .filter(|p| !documented.contains(*p) && !allowed.contains(*p))
        .collect();
    assert!(
        undocumented.is_empty(),
        "{} route(s) are served but neither documented in `ApiDoc` nor listed in \
         NOT_YET_DOCUMENTED. A client generated from this spec cannot call them:\n  {}",
        undocumented.len(),
        undocumented
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );

    // A stale allowlist is its own hazard: it would keep excusing a route that no longer exists, and
    // silently excuse a NEW one that later claims that path.
    let stale: Vec<&String> = allowed
        .iter()
        .filter(|p| !served.contains(*p) && !documented.contains(*p))
        .collect();
    assert!(
        stale.is_empty(),
        "NOT_YET_DOCUMENTED names routes that are not served:\n  {}",
        stale
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );

    eprintln!(
        "openapi: {} of {} /api/v1 routes documented, {} to go",
        documented.len(),
        served.len(),
        served.len() - documented.len().min(served.len())
    );
}

/// The spec must be valid OpenAPI 3.1 and actually describe something.
#[test]
fn the_spec_is_well_formed() {
    let json = serde_json::to_value(ApiDoc::openapi()).expect("spec serializes");
    assert!(
        json["openapi"]
            .as_str()
            .unwrap_or_default()
            .starts_with("3.1"),
        "expected OpenAPI 3.1, got {:?}",
        json["openapi"]
    );
    assert!(!json["paths"].as_object().unwrap().is_empty());
    // The error body is referenced by every documented response; a spec missing it generates clients
    // with no error type at all.
    assert!(
        json["components"]["schemas"]["ErrorBody"].is_object(),
        "ErrorBody must be a component so clients share one error type"
    );
}

/// The contract's error-code enumeration must be exactly what the server can emit.
///
/// It was not. The published description listed `busy`, which `code_for_status` has never returned,
/// and omitted `payload_too_large` and `rate_limited`, which it returns routinely. A client
/// branching on `busy` would wait forever for a code that does not exist; one hitting a 429 would
/// meet an identifier the contract never mentioned.
///
/// That is the precise drift the contract module exists to prevent, occurring inside the contract
/// module — which is why the fix is a test rather than a correction. Both directions are checked:
/// a code the server can emit must be documented, AND a documented code must be reachable, because
/// a contract that promises an identifier nothing produces is its own kind of lie.
#[test]
fn codes_documented_match_codes_returned() {
    use axum::http::StatusCode;
    use heldar_kernel::error::AppError;
    use std::collections::BTreeSet;

    let documented: BTreeSet<&str> = AppError::ALL_CODES.iter().copied().collect();
    assert_eq!(
        documented.len(),
        AppError::ALL_CODES.len(),
        "ALL_CODES contains a duplicate"
    );

    // Every status the classifier can produce, swept exhaustively rather than by a hand-picked
    // sample — a sample is how `payload_too_large` went unnoticed in the first place.
    let mut emitted: BTreeSet<&str> = BTreeSet::new();
    for raw in 100u16..600 {
        if let Ok(status) = StatusCode::from_u16(raw) {
            emitted.insert(AppError::code_for_status(status));
        }
    }

    let undocumented: Vec<_> = emitted.difference(&documented).collect();
    assert!(
        undocumented.is_empty(),
        "the server can return {undocumented:?}, which the published contract does not mention — a \
         client cannot branch on a code it was never told about"
    );

    let unreachable: Vec<_> = documented.difference(&emitted).collect();
    assert!(
        unreachable.is_empty(),
        "the contract documents {unreachable:?}, which no status maps to — a client waiting for one \
         of these waits forever"
    );

    // And the served spec's own description must name them, so the JSON an integrator reads is the
    // same list. This is the artifact that actually reaches a client.
    let spec = serde_json::to_value(ApiDoc::openapi()).expect("spec");
    let desc = spec["components"]["schemas"]["ErrorBody"]["properties"]["code"]["description"]
        .as_str()
        .unwrap_or_default();
    for code in AppError::ALL_CODES {
        assert!(
            desc.contains(code),
            "the served contract's `code` description never mentions {code:?}:\n{desc}"
        );
    }
    assert!(
        !desc.contains("busy"),
        "the description still names `busy`, which nothing returns:\n{desc}"
    );
}

/// A write-only field must be structurally unable to reach a response type (#120).
///
/// `Camera` carries a plaintext `password`; `CameraView` carries `has_password: bool` instead, and
/// its doc comment says the password "is never serialized to clients". This turns that comment into
/// a check — against the SERVED SPEC, not the source — because the failure mode is a response schema
/// that quietly grows the field back and a generated client that then types it as returnable.
///
/// The sweep is over every schema in the document rather than a named list: a hand-picked list is
/// how the next response type carrying a secret gets missed.
#[test]
fn no_response_schema_exposes_a_write_only_secret() {
    let spec = serde_json::to_value(ApiDoc::openapi()).expect("spec");
    let schemas = spec["components"]["schemas"]
        .as_object()
        .expect("components.schemas");

    // Names that must never appear as a property of a schema a client READS. `has_password` is the
    // deliberate, safe alternative and is allowed by name.
    const FORBIDDEN: &[&str] = &["password", "secret", "token", "private_key", "api_key"];

    // Request bodies legitimately carry credentials — that is how a camera is enrolled. Only
    // schemas a response can reference are swept.
    let request_only: std::collections::BTreeSet<&str> = [
        "CameraCreate",
        "CameraUpdate",
        "SiteCreate",
        "SiteUpdate",
        "TimezoneUpdate",
    ]
    .into_iter()
    .collect();

    let mut offences: Vec<String> = Vec::new();
    for (name, schema) in schemas {
        if request_only.contains(name.as_str()) {
            continue;
        }
        let Some(props) = schema["properties"].as_object() else {
            continue;
        };
        for prop in props.keys() {
            let p = prop.to_ascii_lowercase();
            if p == "has_password" {
                continue;
            }
            if FORBIDDEN
                .iter()
                .any(|f| p == *f || p.ends_with(&format!("_{f}")))
            {
                offences.push(format!("{name}.{prop}"));
            }
        }
    }
    assert!(
        offences.is_empty(),
        "response schema(s) expose a write-only secret: {offences:?}. A generated client would type \
         these as readable, and an integrator would reasonably expect the server to return them."
    );
}

/// Write the served document to `target/openapi.json` so CI can diff it against the last release
/// without booting a server (#120).
///
/// A test rather than a binary because the document is already reachable from the test harness, and
/// a second entry point is a second thing that can disagree with the first — the failure this whole
/// module exists to prevent.
#[test]
fn write_the_served_document_for_diffing() {
    let spec = heldar_kernel::openapi::document();
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .join("target/openapi.json");
    std::fs::write(&out, serde_json::to_vec_pretty(&spec).expect("serialize"))
        .unwrap_or_else(|e| panic!("writing {}: {e}", out.display()));
    assert!(
        spec["paths"].as_object().is_some_and(|p| !p.is_empty()),
        "the document has no paths — writing an empty spec would make every diff against it read \
         as a wholesale removal"
    );
}
