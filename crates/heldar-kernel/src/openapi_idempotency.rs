//! Document `Idempotency-Key` in the contract, on every operation that honours it (#121).
//!
//! The header works across the whole `/api/v1` surface — the layer is mounted for all of it and
//! applies to every mutation carrying the header — but the contract said nothing about it. An
//! integrator had to learn it from prose, or from a 409 they did not expect. For a header whose
//! whole purpose is safe retries by machines, "discoverable by failing" is the wrong way round.
//!
//! # Why a post-pass rather than an attribute on each handler
//!
//! Same reasoning as `openapi_security`: this is ONE fact about a hundred operations. As
//! `#[utoipa::path(...)]` attributes it would be a hundred copies to keep in step, and the first one
//! somebody forgot would be indistinguishable from an operation that genuinely does not support it.
//! As a post-pass over the generated document it is derived from the HTTP method, which is exactly
//! what the middleware itself branches on — so the documentation cannot disagree with the behaviour
//! without the two disagreeing about what a mutation is.

use serde_json::{json, Value};

/// The methods the layer treats as mutations. Mirrors `idempotency::layer`; the test below asserts
/// the two lists stay identical rather than trusting this comment.
pub const MUTATING: [&str; 4] = ["post", "put", "patch", "delete"];

const HEADER_DESCRIPTION: &str = "\
Optional. Replay-safe retries: send the same key with the same body and the original response is \
returned without repeating the side effect. The same key with a DIFFERENT body is a 409 \
`idempotency_key_reused`, which is a bug in the caller rather than a state to retry through. \
Keys are scoped to the calling credential, so two callers may use the same string without \
colliding, and are honoured for 24 hours. \
\
Requests with no `Content-Length` (chunked) or a body over 64 KiB run UNPROTECTED — replay needs a \
stored body, and one that cannot be bounded without reading it cannot be stored. Such a request \
succeeds normally; it simply is not deduplicated, so a retry may repeat the effect.";

/// Add the header parameter and the conflict response to every mutating operation.
pub fn decorate(spec: &mut Value) {
    let Some(paths) = spec.get_mut("paths").and_then(Value::as_object_mut) else {
        return;
    };
    for (_path, item) in paths.iter_mut() {
        let Some(methods) = item.as_object_mut() else {
            continue;
        };
        for method in MUTATING {
            let Some(op) = methods.get_mut(method).and_then(Value::as_object_mut) else {
                continue;
            };

            let params = op
                .entry("parameters")
                .or_insert_with(|| json!([]))
                .as_array_mut();
            if let Some(params) = params {
                // Idempotent in itself: regenerating the document must not accumulate copies.
                let already = params
                    .iter()
                    .any(|p| p.get("name").and_then(Value::as_str) == Some("Idempotency-Key"));
                if !already {
                    params.push(json!({
                        "name": "Idempotency-Key",
                        "in": "header",
                        "required": false,
                        "description": HEADER_DESCRIPTION,
                        "schema": {"type": "string", "maxLength": 200},
                    }));
                }
            }

            if let Some(responses) = op.get_mut("responses").and_then(Value::as_object_mut) {
                // Never overwrite a 409 the handler documents for its own reasons — a conflicting
                // resource state and a reused key are different things, and replacing the
                // handler's description with this one would be a documentation bug of its own.
                responses.entry("409").or_insert_with(|| {
                    json!({
                        "description": "An `Idempotency-Key` already used by this credential for a \
                                        different request body. The original request is unaffected; \
                                        this one did nothing.",
                        "content": {
                            "application/json": {
                                "schema": {"$ref": "#/components/schemas/ErrorBody"}
                            }
                        },
                    })
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Value {
        crate::openapi::document()
    }

    /// The point of the change: every operation that honours the header says so.
    #[test]
    fn every_mutating_operation_documents_the_header() {
        let spec = doc();
        let paths = spec["paths"].as_object().expect("paths");
        let mut checked = 0usize;
        for (path, item) in paths {
            for method in MUTATING {
                // `as_object()`, not `get()`: a path contributed by an app crate appears here with a
                // NON-OBJECT placeholder for its operation, and `get()` hands back that placeholder
                // rather than None. Treating it as an operation made this test demand documentation
                // on something with nowhere to put it. The full served document — app crates merged
                // — is asserted in heldar-server/tests/openapi_idempotency.rs, which is the only
                // place all the real operations exist.
                let Some(op) = item.get(method).and_then(Value::as_object) else {
                    continue;
                };
                let params = op
                    .get("parameters")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    params.iter().any(|p| p["name"] == "Idempotency-Key"),
                    "{method} {path} honours Idempotency-Key but does not document it"
                );
                assert!(
                    op.get("responses").and_then(|r| r.get("409")).is_some(),
                    "{method} {path} can answer 409 on a reused key but does not document it"
                );
                checked += 1;
            }
        }
        assert!(
            checked >= 50,
            "only {checked} mutating operations were checked — if the document shrank, this test \
             is passing by covering almost nothing"
        );
    }

    /// A read cannot use the header, so documenting it there would be a lie the contract tells.
    #[test]
    fn reads_are_left_alone() {
        let spec = doc();
        for (path, item) in spec["paths"].as_object().expect("paths") {
            let Some(op) = item.get("get").and_then(Value::as_object) else {
                continue;
            };
            let params = op
                .get("parameters")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                !params.iter().any(|p| p["name"] == "Idempotency-Key"),
                "GET {path} documents a header the layer ignores for reads"
            );
        }
    }

    /// The document is generated more than once per process in tests and tooling. A decoration that
    /// accumulated would produce a contract with the same parameter listed repeatedly.
    #[test]
    fn decorating_twice_changes_nothing() {
        let mut once = doc();
        let before = once.clone();
        decorate(&mut once);
        assert_eq!(once, before, "the decoration is not idempotent");
    }

    /// This module's idea of "a mutation" must be the middleware's idea of one. If they drift, the
    /// contract documents the header on operations that ignore it, or omits it from ones that honour
    /// it — and either way the document is wrong in a way nobody would notice.
    #[test]
    fn the_documented_methods_match_the_ones_the_layer_acts_on() {
        let layer_src = include_str!("idempotency.rs");
        let line = layer_src
            .lines()
            .find(|l| l.contains("let is_mutation = matches!"))
            .expect("the layer still decides what a mutation is on one line");
        for m in MUTATING {
            assert!(
                line.to_ascii_lowercase().contains(m),
                "the contract documents {m} but the layer does not treat it as a mutation: {line}"
            );
        }
        // ...and nothing the layer treats as a mutation is missing from the documented set.
        for m in ["POST", "PUT", "PATCH", "DELETE"] {
            assert!(
                line.contains(m) == MUTATING.contains(&m.to_ascii_lowercase().as_str()),
                "{m} is handled by the layer but not documented (or vice versa)"
            );
        }
    }
}
