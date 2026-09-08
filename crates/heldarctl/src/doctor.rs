//! `heldarctl doctor` — what is wrong with this box, in one command (#122).
//!
//! The flagship workflow. An installer standing in a car park with a laptop should be able to run
//! one thing and be told what to fix, and a CI job should be able to branch on the same answer.
//!
//! # Findings, and why they are the server's where possible
//!
//! The box already knows its own security posture (`/api/v1/system/posture`, #126) and its own
//! camera health. `doctor` does NOT re-derive those — a second implementation of "is this box
//! healthy" is a second answer, and the one an operator sees would eventually disagree with the one
//! the box acts on. It collects, adds the things only a CLIENT can see (can I reach it at all? does
//! my contract version match?), and presents them.
//!
//! # Severity decides the exit code, not the count
//!
//! `doctor` exits non-zero for blocking severities so CI can gate on it. A warning-level finding is
//! not a failure: a box with one camera on plain RTSP is a box that works, and an installer who has
//! been told that is informed rather than blocked. Only things that mean *this box is not doing its
//! job* — or that its answers cannot be trusted — are blocking.

use serde::Serialize;

/// How bad a finding is. Ordered: `Blocking` implies a non-zero exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Worth knowing; the box is doing its job.
    Info,
    /// Degraded or exposed, still recording.
    Warning,
    /// The box is not doing its job, or its answers cannot be trusted.
    Blocking,
}

/// One thing `doctor` found.
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    /// Stable identifier. Branch on this.
    pub code: &'static str,
    pub severity: Severity,
    /// What resource it is about, when it is about one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    pub detail: String,
    /// What to do. Every finding has one — a finding an operator cannot act on is noise.
    pub remediation: &'static str,
}

impl Finding {
    pub fn new(
        code: &'static str,
        severity: Severity,
        detail: impl Into<String>,
        remediation: &'static str,
    ) -> Self {
        Self {
            code,
            severity,
            resource: None,
            detail: detail.into(),
            remediation,
        }
    }

    pub fn about(mut self, resource: impl Into<String>) -> Self {
        self.resource = Some(resource.into());
        self
    }
}

/// The server's posture findings, mapped into doctor's severities.
///
/// `weak` is a Warning, not Blocking: an exposed box that records is still recording, and blocking
/// CI on it would train people to pass `--ignore`. `unknown` is Info — it is a statement about what
/// could not be checked, and treating "unverified" as a failure is as wrong as treating it as a pass.
pub fn from_posture(posture: &serde_json::Value) -> Vec<Finding> {
    let mut out = Vec::new();
    let Some(findings) = posture["findings"].as_array() else {
        return out;
    };
    for f in findings {
        let id = f["id"].as_str().unwrap_or("unknown");
        let detail = f["detail"].as_str().unwrap_or_default().to_string();
        let matters: &'static str = Box::leak(
            f["matters"]
                .as_str()
                .unwrap_or("see `heldarctl doctor --output json`")
                .to_string()
                .into_boxed_str(),
        );
        let code: &'static str = Box::leak(format!("posture.{id}").into_boxed_str());
        match f["status"].as_str() {
            Some("weak") => out.push(Finding::new(code, Severity::Warning, detail, matters)),
            Some("unknown") => out.push(Finding::new(code, Severity::Info, detail, matters)),
            _ => {}
        }
    }
    out
}

/// Does the CLI understand this server's contract?
///
/// A MAJOR mismatch is Blocking — not because the CLI cannot send bytes, but because its answers
/// would be unreliable, and a diagnostic tool that might be wrong is worse than one that refuses.
/// A minor difference is Info: the contract is additive within a major, so an older CLI simply does
/// not know about the newest routes.
pub fn compatibility(cli_contract: &str, server_contract: Option<&str>) -> Finding {
    let Some(server) = server_contract else {
        return Finding::new(
            "compat.unknown",
            Severity::Warning,
            "the server did not report an api_version",
            "upgrade the box; a server that cannot state its contract version cannot be checked \
             against this CLI",
        );
    };
    let major = |v: &str| v.split('.').next().unwrap_or("").to_string();
    if major(cli_contract) != major(server) {
        return Finding::new(
            "compat.major_mismatch",
            Severity::Blocking,
            format!("this CLI speaks contract {cli_contract}; the box speaks {server}"),
            "use a heldarctl built for this box's release. A major difference means request and \
             response shapes have changed, so this tool's answers cannot be relied on",
        );
    }
    if cli_contract != server {
        return Finding::new(
            "compat.minor_difference",
            Severity::Info,
            format!("this CLI speaks contract {cli_contract}; the box speaks {server}"),
            "no action needed — within a major version the contract is additive, so at most this \
             CLI does not know about the box's newest routes",
        );
    }
    Finding::new(
        "compat.ok",
        Severity::Info,
        format!("contract {cli_contract}"),
        "no action needed",
    )
}

/// Cameras that are enabled but not recording.
///
/// THE ONE FINDING THAT IS ALWAYS BLOCKING. A camera an operator believes is recording and is not
/// is the failure a video recorder exists to prevent, and it is invisible from a dashboard that
/// shows a green tile because the camera is reachable.
pub fn camera_health(cameras: &serde_json::Value, statuses: &serde_json::Value) -> Vec<Finding> {
    let mut out = Vec::new();
    let empty = Vec::new();
    let cams = cameras.as_array().unwrap_or(&empty);
    let states = statuses["cameras"]
        .as_array()
        .or(statuses.as_array())
        .unwrap_or(&empty);

    for cam in cams {
        let Some(id) = cam["id"].as_str() else {
            continue;
        };
        if cam["enabled"].as_bool() == Some(false) {
            continue;
        }
        let mode = cam["record_mode"].as_str().unwrap_or("continuous");
        let state = states
            .iter()
            .find(|s| s["camera_id"].as_str() == Some(id))
            .and_then(|s| s["state"].as_str())
            .unwrap_or("unknown");
        match state {
            "recording" => {}
            // A scheduled camera outside its window is correct, not broken. Reporting it would
            // train an operator to ignore this finding, which is how the real one gets missed.
            _ if mode.starts_with("scheduled") || mode == "event" => {}
            "unknown" => out.push(
                Finding::new(
                    "camera.state_unknown",
                    Severity::Warning,
                    format!("{id} reports no recorder state"),
                    "the recorder may not have started for this camera yet; re-check shortly, and \
                     look at `heldarctl doctor` again after a minute",
                )
                .about(id),
            ),
            other => out.push(
                Finding::new(
                    "camera.not_recording",
                    Severity::Blocking,
                    format!("{id} is enabled and set to {mode}, but is {other}"),
                    "check the camera's address and credentials with `heldarctl camera test`, then \
                     the recorder logs. A camera believed to be recording and not recording is the \
                     failure this product exists to prevent",
                )
                .about(id),
            ),
        }
    }
    out
}

/// A check `doctor` could not perform, reported instead of skipped.
///
/// THE BUG THIS EXISTS FOR: the collection path used to drop these errors on the floor — `.ok()` on
/// `/api/v1/cameras` and `/api/v1/health/cameras`, `if let Ok(..)` on the posture. With no camera
/// JSON, [`camera_health`] never ran, so `camera.not_recording` — the finding this module calls the
/// failure this product exists to prevent — could not be produced, [`blocks`] was false, and the
/// command printed "no warnings or blocking findings" and exited 0.
///
/// A box answering 403 or 500 on those routes therefore got a clean bill of health from the tool
/// whose whole job is to say what is wrong with it. `docs/HELDARCTL.md` documents gating CI on the
/// exit code, so that silence was load-bearing.
///
/// BLOCKING, deliberately, and this is the one place `doctor` departs from "unverified is not a
/// failure". That principle is right for [`from_posture`], where `unknown` describes a control that
/// could not be assessed on a box that is otherwise answering. Here the tool cannot answer its own
/// question at all — and the caller is a CI gate reading an exit code, which has no way to tell
/// "nothing is wrong" from "nothing was checked" unless this says so.
pub fn unavailable(endpoint: &'static str, exit_code: i32) -> Finding {
    // 2 is AUTH and 6 is SERVER in `main::exit`; anything else reached us from a path that already
    // printed its own diagnostic.
    let (detail, remediation) = match exit_code {
        2 => (
            format!(
                "{endpoint} refused this credential, so the checks that depend on it were not run"
            ),
            "Use a credential holding `camera:read` (and `system:read` for the posture), or run              doctor with an admin key. A key that cannot read the fleet cannot report on it.",
        ),
        _ => (
            format!("{endpoint} did not answer, so the checks that depend on it were not run"),
            "Check the box is up and this route is reachable. Until it answers, this command              cannot tell you whether the box is recording.",
        ),
    };
    Finding::new(
        "collect.unavailable",
        Severity::Blocking,
        detail,
        remediation,
    )
    .about(endpoint.to_string())
}

/// The verdict covers only part of the fleet, because the credential does.
///
/// A camera-scoped key gets a FILTERED 200 from `/api/v1/cameras` — not an error — so the checks all
/// run and report on a subset while looking exactly like a whole-fleet verdict. Info rather than
/// Warning: the box is doing its job, and what is limited is this report's reach. Saying so is the
/// point; silently narrowing the meaning of "healthy" is what it replaces.
pub fn scope_limited(cameras_visible: usize) -> Finding {
    Finding::new(
        "scope.partial_fleet",
        Severity::Info,
        format!(
            "this credential is camera-scoped, so every finding below covers only the              {cameras_visible} camera(s) it can see — not the whole box"
        ),
        "Run with a fleet-scoped credential for a whole-box verdict.",
    )
}

/// The exit code for a set of findings. `true` when anything blocks.
pub fn blocks(findings: &[Finding]) -> bool {
    findings.iter().any(|f| f.severity == Severity::Blocking)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_major_contract_mismatch_blocks_and_a_minor_one_does_not() {
        assert_eq!(
            compatibility("0.1.0", Some("1.0.0")).severity,
            Severity::Blocking,
            "a diagnostic tool that might be wrong is worse than one that refuses"
        );
        assert_eq!(
            compatibility("0.1.0", Some("0.2.0")).severity,
            Severity::Info,
            "within a major the contract is additive"
        );
        assert_eq!(
            compatibility("0.1.0", Some("0.1.0")).severity,
            Severity::Info
        );
        assert_eq!(compatibility("0.1.0", None).severity, Severity::Warning);
    }

    #[test]
    fn an_enabled_camera_that_is_not_recording_blocks() {
        let cams = json!([{"id": "cam_a", "enabled": true, "record_mode": "continuous"}]);
        let states = json!({"cameras": [{"camera_id": "cam_a", "state": "offline"}]});
        let f = camera_health(&cams, &states);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Blocking);
        assert_eq!(f[0].resource.as_deref(), Some("cam_a"));
        assert!(blocks(&f));
    }

    /// A scheduled camera outside its window is CORRECT. Reporting it would train an operator to
    /// ignore this finding, which is how the real one gets missed.
    #[test]
    fn a_scheduled_camera_outside_its_window_is_not_a_finding() {
        let cams = json!([{"id": "cam_a", "enabled": true, "record_mode": "scheduled"}]);
        let states = json!({"cameras": [{"camera_id": "cam_a", "state": "idle"}]});
        assert!(camera_health(&cams, &states).is_empty());

        // ...and a disabled camera is not a finding either.
        let cams = json!([{"id": "cam_b", "enabled": false, "record_mode": "continuous"}]);
        let states = json!({"cameras": [{"camera_id": "cam_b", "state": "disabled"}]});
        assert!(camera_health(&cams, &states).is_empty());
    }

    /// `weak` is a warning and `unknown` is info — treating "unverified" as a failure is as wrong
    /// as treating it as a pass, and blocking CI on an exposed-but-working box trains people to
    /// pass --ignore.
    #[test]
    fn posture_severities_do_not_block() {
        let posture = json!({"findings": [
            {"id": "service_user", "status": "weak", "detail": "running as root", "matters": "fix it"},
            {"id": "recording_volume_encryption", "status": "unknown", "detail": "n/a", "matters": "x"},
            {"id": "secret_key_source", "status": "ok", "detail": "File", "matters": "y"},
        ]});
        let f = from_posture(&posture);
        assert_eq!(f.len(), 2, "an `ok` finding is not reported: {f:?}");
        assert_eq!(f[0].severity, Severity::Warning);
        assert_eq!(f[1].severity, Severity::Info);
        assert!(
            !blocks(&f),
            "an exposed box that records is not a CI failure"
        );
    }

    #[test]
    fn every_finding_carries_a_remediation() {
        let cams = json!([{"id": "cam_a", "enabled": true, "record_mode": "continuous"}]);
        let states = json!({"cameras": [{"camera_id": "cam_a", "state": "error"}]});
        let mut all = camera_health(&cams, &states);
        all.push(compatibility("0.1.0", Some("9.0.0")));
        all.extend(from_posture(&json!({"findings": [
            {"id": "x", "status": "weak", "detail": "d", "matters": "m"}
        ]})));
        for f in &all {
            assert!(
                !f.remediation.trim().is_empty(),
                "a finding an operator cannot act on is noise: {f:?}"
            );
            assert!(!f.code.is_empty() && !f.detail.trim().is_empty(), "{f:?}");
        }
    }
}

/// `doctor` must never report health it did not verify.
#[cfg(test)]
mod collection_failure_tests {
    use super::*;

    /// THE REGRESSION. The collection path dropped these errors, so a box answering 403 or 500 on
    /// the camera routes produced no camera findings at all — and `blocks()` over the remaining
    /// findings was false, which is exit 0 and "no warnings or blocking findings".
    #[test]
    fn an_unavailable_check_blocks_rather_than_vanishing() {
        for code in [2, 6, 3] {
            let f = unavailable("/api/v1/cameras", code);
            assert_eq!(f.severity, Severity::Blocking, "exit code {code}");
            assert!(blocks(&[f]), "exit code {code} did not block");
        }
    }

    /// The two reasons need different remediations: one is fixed by granting a capability, the
    /// other by fixing the box. A single generic message sends the operator to the wrong place.
    #[test]
    fn the_refusal_and_the_outage_read_differently() {
        let refused = unavailable("/api/v1/cameras", 2);
        let down = unavailable("/api/v1/cameras", 6);
        assert!(
            refused.detail.contains("refused this credential"),
            "{}",
            refused.detail
        );
        assert!(
            refused.remediation.contains("camera:read"),
            "{}",
            refused.remediation
        );
        assert!(down.detail.contains("did not answer"), "{}", down.detail);
        assert_ne!(refused.remediation, down.remediation);
    }

    /// Which check could not run has to survive into the JSON, or a CI gate sees a blocking exit
    /// with nothing naming the cause.
    #[test]
    fn the_finding_names_the_endpoint_it_could_not_reach() {
        let f = unavailable("/api/v1/health/cameras", 6);
        assert_eq!(f.resource.as_deref(), Some("/api/v1/health/cameras"));
        assert!(f.detail.contains("/api/v1/health/cameras"));
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains("collect.unavailable"), "{json}");
    }

    /// A camera-scoped key gets a FILTERED 200, not an error, so every check runs and reports on a
    /// subset while looking like a whole-box verdict. Info, not Blocking: the box is doing its job;
    /// what is limited is this report's reach.
    #[test]
    fn a_partial_fleet_is_labelled_without_failing_the_run() {
        let f = scope_limited(2);
        assert_eq!(f.severity, Severity::Info);
        assert!(
            !blocks(std::slice::from_ref(&f)),
            "a scoped credential must not fail the run by itself"
        );
        assert!(f.detail.contains("2 camera(s)"), "{}", f.detail);
        assert!(f.detail.contains("not the whole box"), "{}", f.detail);
    }

    /// The whole point of the change, stated as the property that was false before: a run that
    /// could not read the cameras must not be indistinguishable from a healthy one.
    #[test]
    fn a_run_that_could_not_check_is_not_a_run_that_found_nothing() {
        let healthy: Vec<Finding> = vec![compatibility("0.1.0", Some("0.1.0"))];
        assert!(!blocks(&healthy), "the baseline must be a passing run");

        let mut could_not_check = healthy.clone();
        could_not_check.push(unavailable("/api/v1/cameras", 2));
        assert!(
            blocks(&could_not_check),
            "a run missing the camera checks reported the same verdict as a healthy one"
        );
    }
}
