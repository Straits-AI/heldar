//! What each documented route REQUIRES, machine-readably (#120).
//!
//! An integrator can guess what `GET /api/v1/cameras` returns from its name. What they cannot guess
//! is that it needs `camera:read`, that a camera-scoped credential gets a FILTERED list rather than
//! a 403, and that `PUT /api/v1/system/timezone` refuses a scoped credential outright. That is the
//! part of the contract worth publishing, and until now it existed only in prose and in the code.
//!
//! # Why a table and not per-route annotations
//!
//! The obvious approach is `#[utoipa::path(security(...))]` on each handler. That works, and it is
//! also exactly how the error-code enumeration drifted: a hand-written statement sitting beside the
//! code, agreeing with it right up until someone changes one and not the other.
//!
//! So this is a table, and `every_declared_capability_is_one_the_kernel_enforces` drives the REAL router with a
//! credential lacking each declared capability and asserts a refusal. A declaration that does not
//! match what the kernel enforces fails the build. The table is data the test can iterate; a
//! scattering of attributes is not.
//!
//! # This does not authorize anything
//!
//! The kernel is the enforcement point and stays so. This document describes what it enforces, and
//! the test is what keeps the description honest. A client reading `x-heldar-capability` learns
//! which credential to present, not which check to skip.

use serde_json::{json, Value};

use crate::auth::Cap;

/// How a route treats a camera-scoped credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scoping {
    /// The camera is in the path; a credential not holding it gets 404 (never 403 — the boundary
    /// must not be an existence oracle).
    CameraKeyed,
    /// Answers, but only about the caller's own cameras. A refusal here would be worse: a complete
    /// inventory is what the per-camera checks exist to prevent, and an empty list is not one.
    Filtered,
    /// Refuses a camera-scoped credential outright, because the effect is fleet-wide by nature.
    FleetOnly,
    /// Names no camera and discloses none.
    Neutral,
}

impl Scoping {
    fn slug(self) -> &'static str {
        match self {
            Scoping::CameraKeyed => "camera-keyed",
            Scoping::Filtered => "scope-filtered",
            Scoping::FleetOnly => "fleet-only",
            Scoping::Neutral => "scope-neutral",
        }
    }
}

/// One documented route's requirements.
pub struct Requirement {
    pub path: &'static str,
    pub method: &'static str,
    /// The capability the handler checks first. `None` for routes gated on role rather than a
    /// capability (admin-only settings), which `admin_only` then marks.
    pub capability: Option<Cap>,
    pub admin_only: bool,
    pub scoping: Scoping,
}

/// Every route currently in the OpenAPI document.
///
/// Grows with the document; the test refuses an entry whose declaration the kernel does not enforce,
/// and `openapi_contract`'s coverage test refuses a documented path missing from here.
pub const REQUIREMENTS: &[Requirement] = &[
    Requirement {
        path: "/api/v1/cameras",
        method: "get",
        capability: Some(Cap::CameraRead),
        admin_only: false,
        scoping: Scoping::Filtered,
    },
    Requirement {
        path: "/api/v1/cameras/{id}",
        method: "get",
        capability: Some(Cap::CameraRead),
        admin_only: false,
        scoping: Scoping::CameraKeyed,
    },
    Requirement {
        // Deleting a camera PURGES its recordings, so it is an admin action, not a manager one —
        // I first declared this `registry:manage` from the shape of its siblings, and the
        // enforcement test caught it on the first run. That is the entire argument for the test.
        path: "/api/v1/cameras/{id}",
        method: "delete",
        capability: Some(Cap::Admin),
        admin_only: false,
        scoping: Scoping::CameraKeyed,
    },
    Requirement {
        path: "/api/v1/evidence/exports",
        method: "post",
        capability: Some(Cap::VideoExport),
        admin_only: false,
        scoping: Scoping::CameraKeyed,
    },
    Requirement {
        path: "/api/v1/evidence/exports",
        method: "get",
        capability: Some(Cap::VideoExport),
        admin_only: false,
        scoping: Scoping::Filtered,
    },
    Requirement {
        path: "/api/v1/evidence/exports/{id}",
        method: "get",
        capability: Some(Cap::VideoExport),
        admin_only: false,
        scoping: Scoping::CameraKeyed,
    },
    Requirement {
        path: "/api/v1/evidence/signing-key",
        method: "get",
        capability: Some(Cap::CameraRead),
        admin_only: false,
        scoping: Scoping::Neutral,
    },
    Requirement {
        path: "/api/v1/system/timezone",
        method: "get",
        capability: Some(Cap::SystemRead),
        admin_only: false,
        scoping: Scoping::Neutral,
    },
    Requirement {
        path: "/api/v1/system/timezone",
        method: "put",
        capability: None,
        admin_only: true,
        scoping: Scoping::FleetOnly,
    },
    Requirement {
        path: "/api/v1/sites",
        method: "get",
        capability: Some(Cap::CameraRead),
        admin_only: false,
        scoping: Scoping::Filtered,
    },
    Requirement {
        path: "/api/v1/sites",
        method: "post",
        capability: None,
        admin_only: true,
        scoping: Scoping::FleetOnly,
    },
    Requirement {
        path: "/api/v1/sites/{id}",
        method: "get",
        capability: Some(Cap::CameraRead),
        admin_only: false,
        scoping: Scoping::CameraKeyed,
    },
    Requirement {
        path: "/api/v1/sites/{id}",
        method: "patch",
        capability: None,
        admin_only: true,
        scoping: Scoping::FleetOnly,
    },
    Requirement {
        path: "/api/v1/sites/{id}",
        method: "delete",
        capability: None,
        admin_only: true,
        scoping: Scoping::FleetOnly,
    },
    Requirement {
        path: "/api/v1/cameras/{id}/segments",
        method: "get",
        capability: Some(Cap::VideoPlayback),
        admin_only: false,
        scoping: Scoping::CameraKeyed,
    },
    Requirement {
        path: "/api/v1/cameras/{id}/timeline",
        method: "get",
        capability: Some(Cap::VideoPlayback),
        admin_only: false,
        scoping: Scoping::CameraKeyed,
    },
    Requirement {
        path: "/api/v1/cameras/{id}/gaps",
        method: "get",
        capability: Some(Cap::VideoPlayback),
        admin_only: false,
        scoping: Scoping::CameraKeyed,
    },
    Requirement {
        path: "/api/v1/cameras/{id}/clip",
        method: "post",
        capability: Some(Cap::VideoExport),
        admin_only: false,
        scoping: Scoping::CameraKeyed,
    },
    Requirement {
        path: "/api/v1/cameras/{id}/snapshot",
        method: "get",
        capability: Some(Cap::VideoPlayback),
        admin_only: false,
        scoping: Scoping::CameraKeyed,
    },
    Requirement {
        path: "/api/v1/cameras/{id}/playback/sessions",
        method: "post",
        capability: Some(Cap::VideoPlayback),
        admin_only: false,
        scoping: Scoping::CameraKeyed,
    },
    Requirement {
        path: "/api/v1/playback/sessions/{session_id}",
        method: "delete",
        capability: Some(Cap::VideoPlayback),
        admin_only: false,
        scoping: Scoping::CameraKeyed,
    },
    Requirement {
        path: "/api/v1/cameras/{id}/record-trigger",
        method: "post",
        capability: Some(Cap::RegistryManage),
        admin_only: false,
        scoping: Scoping::CameraKeyed,
    },
];

/// Inject the security scheme and the per-operation requirements into a generated document.
///
/// Done as a post-pass rather than through attributes so the source of truth stays one table the
/// test can iterate. `x-heldar-*` are extensions, not standard fields — a generator that does not
/// know them ignores them, which is the correct behaviour for information no standard expresses.
pub fn decorate(spec: &mut Value) {
    let comps = spec.as_object_mut().and_then(|o| {
        o.entry("components")
            .or_insert_with(|| json!({}))
            .as_object_mut()
    });
    if let Some(c) = comps {
        c.insert(
            "securitySchemes".into(),
            json!({
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "description":
                        "An API key minted by `POST /api/v1/api-keys`, or a session cookie from \
                         `POST /api/v1/auth/login`. When auth is disabled (the LAN-appliance \
                         default) every route answers as a fleet-wide admin.",
                }
            }),
        );
    }
    // Applied globally: every `/api/v1` route is behind the authentication floor, so declaring it
    // once is both simpler and more accurate than per-operation repetition.
    spec["security"] = json!([{ "bearerAuth": [] }]);

    for req in REQUIREMENTS {
        let op = &mut spec["paths"][req.path][req.method];
        if !op.is_object() {
            continue;
        }
        if let Some(cap) = req.capability {
            op["x-heldar-capability"] = json!(cap.slug());
        }
        if req.admin_only {
            op["x-heldar-admin-only"] = json!(true);
        }
        op["x-heldar-scope"] = json!(req.scoping.slug());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// No duplicate (path, method), and every capability slug is one the kernel knows.
    #[test]
    fn the_table_is_internally_coherent() {
        let mut seen = BTreeSet::new();
        for r in REQUIREMENTS {
            assert!(
                seen.insert((r.path, r.method)),
                "{} {} declared twice",
                r.method,
                r.path
            );
            if let Some(cap) = r.capability {
                assert!(
                    Cap::parse(cap.slug()).is_some(),
                    "{} is not a slug the kernel parses",
                    cap.slug()
                );
            }
            assert!(
                !(r.admin_only && r.capability.is_some()),
                "{} {} declares both an admin gate and a capability — say which one the handler \
                 checks FIRST, because that is the one a caller actually hits",
                r.method,
                r.path
            );
        }
    }

    /// A fleet-only route must not also be camera-keyed: those are contradictory claims about what
    /// a camera-scoped credential gets.
    #[test]
    fn scoping_claims_do_not_contradict_each_other() {
        for r in REQUIREMENTS {
            if r.scoping == Scoping::FleetOnly {
                assert!(
                    r.admin_only || r.capability.is_some(),
                    "{} {} is fleet-only but declares no gate at all",
                    r.method,
                    r.path
                );
            }
        }
    }
}
