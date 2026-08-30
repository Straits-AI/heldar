//! The tools this sidecar exposes, and why it can only expose these (#123).
//!
//! # Read-only is structural, not instructed
//!
//! The issue's sharpest requirement: "bind read-only mode structurally, not through prompt
//! instructions." A system prompt saying "do not modify anything" is not a control — it is a
//! request, and the first jailbreak or confused-deputy chain ignores it.
//!
//! So a tool here carries an HTTP METHOD it is allowed to use, and that method is `GET`. There is no
//! branch a model can talk its way down, because there is no code path from a tool call to a
//! non-GET request: [`Tool::method`] is the only method the dispatcher will send, and
//! `every_tool_is_read_only` fails the build's tests if one is ever added that is not.
//!
//! Adding a mutation later means adding a capability group and a deliberate flag, which is what the
//! issue asks for — not relaxing a string.
//!
//! # What a tool returns
//!
//! Structured facts, ids, timestamps. Never an RTSP URL, a credential, a session cookie or a media
//! capability — those are what an agent would happily paste into a summary, and a signed media URL
//! in a model's context is a media capability that has left the building.

use serde::Serialize;

/// One exposed tool.
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    /// The HTTP method. Always GET — see the module docs.
    pub method: &'static str,
    /// The path, with `{}` where the single argument goes.
    pub path: &'static str,
    /// The argument this tool takes, if any.
    pub arg: Option<&'static str>,
    pub arg_description: &'static str,
}

/// Every tool. Read-only by construction.
pub const TOOLS: &[Tool] = &[
    Tool {
        name: "get_system_health",
        description: "This box: version, uptime, camera counts, and whether it is recording.",
        method: "GET",
        path: "/api/v1/system",
        arg: None,
        arg_description: "",
    },
    Tool {
        name: "list_cameras",
        description: "Cameras this credential can see. A camera outside its scope is absent, not \
                      refused — the same answer the kernel gives, so the list cannot be used to \
                      discover a fleet.",
        method: "GET",
        path: "/api/v1/cameras",
        arg: None,
        arg_description: "",
    },
    Tool {
        name: "get_camera_health",
        description: "Recorder state for every visible camera: recording, offline, error, idle.",
        method: "GET",
        path: "/api/v1/health/cameras",
        arg: None,
        arg_description: "",
    },
    Tool {
        name: "get_timeline",
        description: "The recorded ranges for one camera — what footage actually exists.",
        method: "GET",
        path: "/api/v1/cameras/{}/timeline",
        arg: Some("camera_id"),
        arg_description: "Camera id.",
    },
    Tool {
        name: "get_recording_gaps",
        description: "Intervals where a camera was expected to be recording and was not. The \
                      answer to 'was this covered?', which a timeline alone does not give.",
        method: "GET",
        path: "/api/v1/cameras/{}/recording-gaps",
        arg: Some("camera_id"),
        arg_description: "Camera id.",
    },
    Tool {
        name: "get_incident",
        description: "The segments locked to one incident.",
        method: "GET",
        path: "/api/v1/incidents/{}/segments",
        arg: Some("incident_id"),
        arg_description: "Incident id.",
    },
    Tool {
        name: "list_ai_workers",
        description: "AI worker leases: which workers hold which cameras, and how fresh.",
        method: "GET",
        path: "/api/v1/ai/samplers",
        arg: None,
        arg_description: "",
    },
    Tool {
        name: "get_security_posture",
        description: "This box's security posture as findings — key source, process visibility, \
                      service user, volume encryption, plaintext credentials. `unknown` means a \
                      control could not be assessed, which is NOT a pass.",
        method: "GET",
        path: "/api/v1/system/posture",
        arg: None,
        arg_description: "",
    },
    Tool {
        name: "get_retention_limits",
        description: "The effective recording size cap and free-disk floor.",
        method: "GET",
        path: "/api/v1/system/retention",
        arg: None,
        arg_description: "",
    },
    Tool {
        name: "get_backup_status",
        description: "Recent backup jobs and their outcomes.",
        method: "GET",
        path: "/api/v1/backup/jobs",
        arg: None,
        arg_description: "",
    },
];

/// The MCP `tools/list` shape.
#[derive(Serialize)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

pub fn specs() -> Vec<ToolSpec> {
    TOOLS
        .iter()
        .map(|t| ToolSpec {
            name: t.name,
            description: t.description,
            input_schema: match t.arg {
                None => serde_json::json!({"type": "object", "properties": {}}),
                Some(a) => serde_json::json!({
                    "type": "object",
                    "properties": { a: {"type": "string", "description": t.arg_description} },
                    "required": [a],
                }),
            },
        })
        .collect()
}

pub fn find(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.name == name)
}

/// Strip anything that must never reach a model's context.
///
/// The kernel already keeps credentials out of its responses — `CameraView` carries `has_password`,
/// not `password`, and `record_url_masked` masks the userinfo. This is defence in depth for the
/// fields that are legitimately present and still must not travel: a stream URL contains userinfo,
/// a signed media URL IS a capability, and a username is half a credential. An agent will paste any
/// of them into a summary without a second thought, and a capability in a transcript has left the
/// building.
///
/// A camera's ADDRESS is deliberately NOT redacted. It appears in `record_url_masked` regardless, so
/// stripping the field alone would be theatre; and an agent diagnosing "which device is this" needs
/// it. That makes device addresses part of what this sidecar discloses, which is a reason to scope
/// its key — and it is stated in the docs rather than left for someone to discover.
pub fn redact(mut v: serde_json::Value) -> serde_json::Value {
    fn walk(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(map) => {
                let drop: Vec<String> = map
                    .keys()
                    .filter(|k| {
                        let k = k.to_ascii_lowercase();
                        k.contains("password")
                            && !k.starts_with("has_")
                            // Half a credential, and an agent has no diagnostic use for it. The
                            // kernel already masks the credential in `record_url_masked`; leaving
                            // the username beside it hands over the other half.
                            || k == "username"
                            || k.ends_with("_url") && k.contains("stream")
                            || k == "url"
                            || k == "token"
                            || k.ends_with("_token")
                            || k == "secret"
                            || k.ends_with("_secret") && !k.starts_with("has_")
                    })
                    .cloned()
                    .collect();
                for k in drop {
                    map.insert(k, serde_json::Value::String("<redacted>".into()));
                }
                for (_, x) in map.iter_mut() {
                    walk(x);
                }
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(&mut v);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE STRUCTURAL GUARANTEE. If this ever fails, a mutation has become reachable — and the
    /// point of the sidecar is that no prompt can talk it into one.
    #[test]
    fn every_tool_is_read_only() {
        for t in TOOLS {
            assert_eq!(
                t.method, "GET",
                "{} is not read-only. Read-only is bound structurally here, not by instruction: \
                 adding a mutation means adding a capability group and a deliberate flag, not \
                 changing this method",
                t.name
            );
        }
    }

    /// Nothing on the excluded list can be reached, however the caller phrases it.
    #[test]
    fn no_tool_touches_an_excluded_surface() {
        // From the issue: gate/relay actuation, PTZ, deletion, retention weakening, key creation,
        // credential retrieval, plugin installation, auth/CORS/remote-trust changes.
        const FORBIDDEN: &[&str] = &[
            "/gate",
            "/relay",
            "/ptz",
            "/api-keys",
            "/auth/",
            "/registry/install",
            "/control/io",
        ];
        for t in TOOLS {
            for f in FORBIDDEN {
                assert!(
                    !t.path.contains(f),
                    "{} reaches an explicitly excluded surface ({f})",
                    t.name
                );
            }
        }
    }

    #[test]
    fn every_tool_has_a_schema_matching_its_argument() {
        for (t, s) in TOOLS.iter().zip(specs()) {
            let required = s.input_schema["required"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            match t.arg {
                None => assert!(required.is_empty(), "{} takes no argument", t.name),
                Some(a) => {
                    assert_eq!(required.len(), 1, "{} takes exactly one", t.name);
                    assert_eq!(required[0].as_str(), Some(a));
                    assert!(
                        !t.arg_description.is_empty(),
                        "{} must describe its argument — a schema an agent cannot read is a schema \
                         it will guess at",
                        t.name
                    );
                }
            }
            assert!(
                t.path.contains("{}") == t.arg.is_some(),
                "{}'s path and argument disagree",
                t.name
            );
        }
    }

    /// A signed media URL is a capability. An RTSP URL carries credentials. Neither may reach a
    /// model's context, where it becomes a line in a transcript someone pastes.
    #[test]
    fn redaction_removes_capabilities_and_credentials() {
        let v = serde_json::json!({
            "id": "cam_a",
            "has_password": true,
            "password": "hunter2",
            "main_stream_url": "rtsp://admin:pw@10.0.0.5/Streaming",
            "nested": [{"url": "/media/clips/x.mp4?sig=abc", "token": "vok_live"}],
            "name": "Front Gate",
        });
        let out = redact(v);
        let s = out.to_string();
        for leaked in ["hunter2", "10.0.0.5", "sig=abc", "vok_live"] {
            assert!(!s.contains(leaked), "redaction let {leaked:?} through: {s}");
        }
        // A username is half a credential and has no diagnostic value.
        let out2 = redact(serde_json::json!({"username": "admin", "address": "10.0.0.5"}));
        assert_eq!(out2["username"], "<redacted>");
        // The ADDRESS deliberately survives: it appears in `record_url_masked` regardless, so
        // stripping the field alone would be theatre, and an agent asking "which device is this"
        // needs it. Asserted so the decision is visible rather than incidental.
        assert_eq!(out2["address"], "10.0.0.5");

        // The USEFUL fields survive — a redactor that removes everything is a redactor nobody keeps.
        assert_eq!(out["id"], "cam_a");
        assert_eq!(out["name"], "Front Gate");
        assert_eq!(
            out["has_password"], true,
            "`has_password` is the safe alternative and must not be stripped with the real one"
        );
    }
}
