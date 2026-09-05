//! `Idempotency-Key` is documented on every operation that honours it, in the SERVED document.
//!
//! The kernel's own test can only see the kernel's paths. This one uses `heldar_server::api_document`
//! — kernel plus entry, movement and search — which is the document a client actually generates
//! from. The app crates are exactly where a cross-cutting fact goes missing: they were the crates
//! with 47 routes and no scope call, and a contract that never merges them proves nothing about
//! them.

use serde_json::Value;

const MUTATING: [&str; 4] = ["post", "put", "patch", "delete"];

#[test]
fn every_mutating_operation_in_the_served_document_declares_the_header() {
    let spec = heldar_server::api_document();
    let paths = spec["paths"].as_object().expect("paths");

    let mut checked = 0usize;
    let mut missing: Vec<String> = Vec::new();
    for (path, item) in paths {
        for method in MUTATING {
            let Some(op) = item.get(method).and_then(Value::as_object) else {
                continue;
            };
            checked += 1;
            let has_header = op
                .get("parameters")
                .and_then(Value::as_array)
                .is_some_and(|ps| ps.iter().any(|p| p["name"] == "Idempotency-Key"));
            let has_conflict = op.get("responses").and_then(|r| r.get("409")).is_some();
            if !has_header || !has_conflict {
                missing.push(format!("{method} {path}"));
            }
        }
    }

    assert!(
        missing.is_empty(),
        "{} mutating operation(s) honour Idempotency-Key without documenting it: {missing:#?}",
        missing.len()
    );
    // The assertion above is satisfied by an empty document, so bound it from below too.
    assert!(
        checked >= 60,
        "only {checked} mutating operations were found — if the document shrank, this test is \
         passing by checking almost nothing"
    );
}

/// A read never consults the header, so documenting it on one would be a claim the contract cannot
/// keep. This is the direction that would go unnoticed: an over-documented GET breaks nothing until
/// somebody relies on it.
#[test]
fn reads_in_the_served_document_do_not_claim_it() {
    let spec = heldar_server::api_document();
    for (path, item) in spec["paths"].as_object().expect("paths") {
        let Some(op) = item.get("get").and_then(Value::as_object) else {
            continue;
        };
        if let Some(params) = op.get("parameters").and_then(Value::as_array) {
            assert!(
                !params.iter().any(|p| p["name"] == "Idempotency-Key"),
                "GET {path} documents a header the layer ignores for reads"
            );
        }
    }
}
