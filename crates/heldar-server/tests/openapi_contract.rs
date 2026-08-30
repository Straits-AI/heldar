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
    "/api/v1/cameras/{id}/health",
    "/api/v1/cameras/{id}/liveview",
    "/api/v1/cameras/{id}/recording-gaps",
    "/api/v1/cameras/{id}/recording-gaps/{gap_id}/retry",
    "/api/v1/cameras/{id}/schedules",
    "/api/v1/cameras/{id}/snapshot-schedules",
    "/api/v1/cameras/{id}/snapshots",
    "/api/v1/cameras/{id}/test",
    "/api/v1/discover",
    "/api/v1/events",
    "/api/v1/health/cameras",
    "/api/v1/incidents",
    "/api/v1/incidents/{incident_id}/segments",
    "/api/v1/modules",
    "/api/v1/modules/{id}",
    "/api/v1/openapi.json",
    "/api/v1/outbox",
    "/api/v1/registry",
    "/api/v1/registry/refresh",
    "/api/v1/schedules/{schedule_id}",
    "/api/v1/segments/{id}/evidence-lock",
    "/api/v1/segments/{id}/incident",
    "/api/v1/site",
    "/api/v1/snapshot-schedules/{schedule_id}",
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

    // The COMPOSED document — kernel plus every app crate. Reading only the kernel's `ApiDoc`
    // here would report every app-crate route as undocumented while the served surface documents
    // them perfectly well.
    let spec = heldar_server::api_document();
    let documented: BTreeSet<String> = spec["paths"]
        .as_object()
        .expect("paths")
        .keys()
        .cloned()
        .collect();
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
    let json = heldar_server::api_document();
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
    let spec = heldar_server::api_document();
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
    let spec = heldar_server::api_document();
    let schemas = spec["components"]["schemas"]
        .as_object()
        .expect("components.schemas");

    // Names that must never appear as a property of a schema a client READS. `has_password` and
    // `has_secret` are the deliberate, safe alternatives — a boolean saying whether one is set.
    const FORBIDDEN: &[&str] = &["password", "secret", "token", "private_key", "api_key"];

    // WHICH SCHEMAS A RESPONSE CAN REACH, derived from the document rather than listed by hand.
    //
    // Request bodies legitimately carry credentials — that is how a camera is enrolled and how
    // anyone logs in. The first version of this exempted them by NAME, which is the same
    // hardcoded-list shape that has gone stale three times in this repository already: it listed
    // five types, and the moment the contract grew it flagged `LoginRequest.password` as a leak.
    //
    // A schema is a response schema if a 2xx response references it, directly or through another
    // response schema. Nothing else needs saying, and nothing goes stale.
    let mut response_roots: std::collections::BTreeSet<String> = Default::default();
    for (_, item) in spec["paths"].as_object().expect("paths") {
        for (_, op) in item.as_object().expect("path item") {
            let Some(responses) = op["responses"].as_object() else {
                continue;
            };
            for (code, resp) in responses {
                if !code.starts_with('2') {
                    continue;
                }
                collect_refs(&resp["content"], &mut response_roots);
            }
        }
    }
    // Transitively: a response schema's own properties are also read by a client.
    let mut reachable = response_roots.clone();
    let mut frontier: Vec<String> = response_roots.into_iter().collect();
    while let Some(name) = frontier.pop() {
        let Some(schema) = schemas.get(&name) else {
            continue;
        };
        let mut refs = Default::default();
        collect_refs(schema, &mut refs);
        for r in refs {
            if reachable.insert(r.clone()) {
                frontier.push(r);
            }
        }
    }

    let mut offences: Vec<String> = Vec::new();
    for (name, schema) in schemas {
        if !reachable.contains(name.as_str()) {
            continue; // a request-only schema; credentials belong in those
        }
        let Some(props) = schema["properties"].as_object() else {
            continue;
        };
        for prop in props.keys() {
            let p = prop.to_ascii_lowercase();
            if p.starts_with("has_") {
                continue; // `has_password` / `has_secret`: the safe alternative, a boolean
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
    let spec = heldar_server::api_document();
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

/// `operationId` must be unique across the document — OpenAPI requires it, and every generator uses
/// it to name a method.
///
/// It was not. utoipa derives the id from the handler's function name, and `sites::create`,
/// `evidence::create`, `sites::list`, `evidence::list`, `sites::get_one` and `evidence::get_one`
/// collided three ways. The served spec was invalid, and nothing noticed until a generated client
/// tried to compile: TypeScript refused with "duplicate function implementation" and Python quietly
/// produced THIRTEEN methods for fourteen operations — the second silently overwriting the first.
///
/// That silent overwrite is the reason this is a test and not a lint. A generator that keeps going
/// gives you a client missing an endpoint, and you find out when a call you never wrote is missing.
#[test]
fn every_operation_id_is_unique() {
    use std::collections::BTreeMap;

    let spec = heldar_server::api_document();
    let mut seen: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (path, item) in spec["paths"].as_object().expect("paths") {
        for (method, op) in item.as_object().expect("path item") {
            if !["get", "put", "post", "delete", "patch"].contains(&method.as_str()) {
                continue;
            }
            let id = op["operationId"]
                .as_str()
                .unwrap_or_else(|| panic!("{} {} has no operationId", method.to_uppercase(), path));
            seen.entry(id.to_string()).or_default().push(format!(
                "{} {}",
                method.to_uppercase(),
                path
            ));
        }
    }
    let dupes: Vec<_> = seen.iter().filter(|(_, v)| v.len() > 1).collect();
    assert!(
        dupes.is_empty(),
        "duplicate operationId(s), which make the document invalid and silently collapse generated \
         client methods: {dupes:?}"
    );
}

/// The dashboard's hand-written types must agree with the generated contract (#120).
///
/// Replacing `apps/web/src/lib/types.ts` wholesale is not on: it covers 151 routes and the contract
/// covers 14, so the generated file is a strict subset. What IS achievable now — and is the part
/// that actually protects anyone — is that where they overlap they must not disagree. A dashboard
/// field the server no longer returns, or a required field the dashboard thinks is optional, is a
/// runtime `undefined` that no typecheck catches today.
///
/// This compares property NAMES rather than TypeScript types: the two are written in different
/// idioms (`string | null` vs `?:`), and asserting on the rendering would fail on formatting rather
/// than on substance. Names are what a runtime access actually depends on.
#[test]
fn the_dashboard_types_agree_with_the_contract_where_they_overlap() {
    use std::collections::BTreeSet;

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root");
    let dash = std::fs::read_to_string(root.join("apps/web/src/lib/types.ts"))
        .expect("the dashboard's types");
    let spec = heldar_server::api_document();
    let schemas = spec["components"]["schemas"].as_object().expect("schemas");

    // Interfaces the dashboard defines under the same name as a contract schema.
    let mut checked = 0usize;
    for (name, schema) in schemas {
        let Some(props) = schema["properties"].as_object() else {
            continue;
        };
        let needle = format!("export interface {name} {{");
        let Some(start) = dash.find(&needle) else {
            continue; // the dashboard does not model this one; not a disagreement
        };
        let body = &dash[start + needle.len()..];
        let end = body.find("\n}").unwrap_or(body.len());
        let body = &body[..end];

        let dashboard_fields: BTreeSet<String> = body
            .lines()
            .filter_map(|l| {
                let l = l.trim();
                if l.starts_with("//") || l.starts_with("*") || l.starts_with("/*") {
                    return None;
                }
                let name = l.split(['?', ':']).next()?.trim();
                (!name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_'))
                    .then(|| name.to_string())
            })
            .collect();

        let contract_fields: BTreeSet<String> = props.keys().cloned().collect();
        let missing: Vec<_> = contract_fields.difference(&dashboard_fields).collect();
        assert!(
            missing.is_empty(),
            "the contract's {name} has field(s) the dashboard does not model: {missing:?}. A field \
             the server returns and the dashboard has never heard of is not an error today, but it \
             means the dashboard is working from a stale picture of the API."
        );
        checked += 1;
    }
    assert!(
        checked >= 3,
        "only {checked} shared type(s) were compared — if the dashboard renamed its interfaces this \
         test silently stops checking anything"
    );
}

/// Every `#/components/schemas/X` referenced anywhere inside a JSON value.
fn collect_refs(v: &serde_json::Value, out: &mut std::collections::BTreeSet<String>) {
    match v {
        serde_json::Value::Object(o) => {
            if let Some(r) = o.get("$ref").and_then(|r| r.as_str()) {
                if let Some(name) = r.strip_prefix("#/components/schemas/") {
                    out.insert(name.to_string());
                }
            }
            for (_, x) in o {
                collect_refs(x, out);
            }
        }
        serde_json::Value::Array(a) => {
            for x in a {
                collect_refs(x, out);
            }
        }
        _ => {}
    }
}
