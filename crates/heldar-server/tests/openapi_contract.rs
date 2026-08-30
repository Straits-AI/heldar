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
const NOT_YET_DOCUMENTED: &[&str] = &[];

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
    let mut response_roots: BTreeSet<String> = Default::default();
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
        let mut refs: BTreeSet<String> = Default::default();
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

/// The dashboard must ALIAS the contract's types, never re-declare them (#120).
///
/// This replaces an earlier test that compared the two field by field. That test found five real
/// drifts — a field the server returns that the dashboard had never heard of — which is exactly why
/// it should not be the mechanism: it detected drift AFTER it shipped. `apps/web/src/lib/types.ts`
/// now aliases the generated `contract.ts`, so the shapes are the same type and cannot diverge at
/// all; this holds that arrangement in place.
///
/// A few aliases legitimately REFINE the contract (`Omit<Contract.X, "f"> & { f: Narrower }`) where
/// the published schema is less precise than the server's behaviour — a Rust `String` that only
/// holds four values. Those still derive from the contract, so they are allowed; re-declaring the
/// whole shape is not.
#[test]
fn the_dashboard_aliases_contract_types_rather_than_redeclaring_them() {
    let root = repo_root();
    let contract = std::fs::read_to_string(root.join("apps/web/src/lib/contract.ts"))
        .expect("apps/web/src/lib/contract.ts");
    let types = std::fs::read_to_string(root.join("apps/web/src/lib/types.ts"))
        .expect("apps/web/src/lib/types.ts");

    let contract_names: BTreeSet<String> = contract
        .lines()
        .filter_map(|l| {
            l.strip_prefix("export interface ")
                .or_else(|| l.strip_prefix("export type "))
                .and_then(|r| r.split_whitespace().next())
                .map(str::to_string)
        })
        .collect();
    assert!(
        contract_names.len() > 50,
        "only {} types found in contract.ts — the parser is looking at the wrong shape and this \
         test is asserting nothing",
        contract_names.len()
    );

    let redeclared: Vec<&String> = contract_names
        .iter()
        .filter(|n| types.contains(&format!("export interface {n} {{")))
        .collect();
    assert!(
        redeclared.is_empty(),
        "the dashboard re-declares {redeclared:?}, which the contract already defines. A second \
         declaration is a second source of truth, and it drifts silently — alias it instead:\n  \
         export type X = Contract.X;\n  export type X = Omit<Contract.X, \"f\"> & {{ f: Narrower }};"
    );

    let aliased = types.matches("= Contract.").count();
    assert!(
        aliased >= 40,
        "only {aliased} alias(es) to the contract found in types.ts — if the dashboard stopped \
         aliasing, this test would pass while proving nothing"
    );
}
/// Every `#/components/schemas/X` referenced anywhere inside a JSON value.
fn collect_refs(v: &serde_json::Value, out: &mut BTreeSet<String>) {
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

/// The shipped example must keep working against the generated client (#120).
///
/// An example that calls a method the generator no longer emits is worse than no example: it is the
/// first thing an integrator copies, and it fails at their keyboard rather than in our CI. This
/// checks every `client.x(...)` the example names still exists on the regenerated client.
///
/// It does NOT run the example against a box — that needs a camera and footage. It checks the
/// vocabulary, which is the part that goes stale when a route is renamed.
#[test]
fn the_shipped_example_calls_methods_the_generated_client_still_has() {
    let root = repo_root();
    let example = root.join("examples/api-client/recording_health.py");
    assert!(
        example.is_file(),
        "the example named by the README is missing: {}",
        example.display()
    );

    let out = std::process::Command::new("python3")
        .arg("-c")
        .arg(
            "import re,sys\n\
             sys.path.insert(0, sys.argv[2])\n\
             import heldar_client as h\n\
             src = open(sys.argv[1]).read()\n\
             have = {m for m in dir(h.HeldarClient) if not m.startswith('_')}\n\
             called = set(re.findall(r'client\\.(\\w+)\\(', src)) - {'HeldarClient'}\n\
             print('MISSING:' + ','.join(sorted(called - have)))\n\
             print('CHECKED:' + str(len(called)))",
        )
        .arg(&example)
        .arg(root.join("clients/python"))
        .output()
        .expect("running the vocabulary check");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "could not check the example against the generated client: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let missing = stdout
        .lines()
        .find_map(|l| l.strip_prefix("MISSING:"))
        .unwrap_or("?");
    assert!(
        missing.is_empty(),
        "the example calls client method(s) the generator no longer emits: {missing}. An example \
         that does not run is worse than none — it is the first thing an integrator copies."
    );

    let checked: usize = stdout
        .lines()
        .find_map(|l| l.strip_prefix("CHECKED:"))
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    assert!(
        checked >= 2,
        "only {checked} client call(s) were found in the example — if it stopped using the \
         generated client, this test is asserting nothing"
    );
}

/// The dashboard's generated contract types must be current (#120).
///
/// `apps/web/src/lib/contract.ts` is generated from the served document, and `types.ts` aliases it
/// rather than re-declaring shapes. That makes drift impossible instead of merely detectable — but
/// only while the generated file is in step. A stale one is a hand-written file wearing a
/// "GENERATED" header, which is worse than no claim at all.
#[test]
fn the_dashboards_generated_contract_types_are_current() {
    let root = repo_root();
    let target = root.join("apps/web/src/lib/contract.ts");
    let before = std::fs::read_to_string(&target).unwrap_or_default();
    assert!(
        !before.is_empty(),
        "apps/web/src/lib/contract.ts is missing — types.ts aliases it, so the dashboard will not \
         typecheck without it"
    );

    let doc = root.join("target/openapi.json");
    std::fs::write(
        &doc,
        serde_json::to_vec_pretty(&heldar_server::api_document()).expect("spec"),
    )
    .expect("writing the served document");
    let scratch = root.join("target/contract-freshness");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(scratch.join("apps/web/src/lib")).expect("scratch");

    let out = std::process::Command::new("python3")
        .arg(root.join("scripts/gen_clients.py"))
        .arg(&doc)
        .arg(scratch.join("clients"))
        .current_dir(&scratch)
        .output()
        .expect("running the generator");
    assert!(
        out.status.success(),
        "the client generator failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let regenerated = std::fs::read_to_string(scratch.join("apps/web/src/lib/contract.ts"))
        .expect("the generator did not write the dashboard types — its output path moved");
    assert_eq!(
        before.trim(),
        regenerated.trim(),
        "apps/web/src/lib/contract.ts is out of date. It is GENERATED and the dashboard aliases it, \
         so a stale copy is a hand-written file wearing a generated header. Regenerate:\n  \
         cargo test -p heldar-server --test openapi_contract write_the_served_document\n  \
         python3 scripts/gen_clients.py target/openapi.json clients"
    );
}
