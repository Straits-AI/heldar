//! Look before you commit, and commit only what you looked at (#121).
//!
//! An agent or a script cannot see what a change will do before making it. A human reads the
//! dashboard, notices "this will delete 400 GB", and stops; automation sends the request.
//!
//! A dry-run answers "what would this do". A PLAN HASH answers the harder question: is the thing I
//! am about to commit still the thing I was shown? Between planning and committing, another
//! operator can change a setting, cameras can be added, and the disk can fill. Without the hash, a
//! confirm-after-plan flow confirms a plan that no longer describes reality — which is worse than no
//! plan at all, because the operator believes they checked.
//!
//! # What the hash covers
//!
//! The REQUEST plus the STATE the outcome depends on. Not a timestamp, and not the whole database:
//! a hash over everything is a hash that changes constantly and trains people to retry without
//! reading, which is the same failure as no check.
//!
//! # It is not a lock
//!
//! Nothing here reserves anything. Two agents can plan the same change and both commit; the second
//! is refused only because the first moved the state its plan described. That is deliberate — a
//! recorder must not have a settings mutex that a crashed client can hold.

use serde::Serialize;
use sha2::{Digest, Sha256};

/// A dry-run result plus the hash a commit must present.
#[derive(Debug, Clone, Serialize)]
pub struct Plan<T: Serialize> {
    /// What would happen. Shape is the caller's.
    pub effect: T,
    /// Hash over the request and the state this effect was computed from.
    pub plan_hash: String,
    /// Whether committing needs the hash. False for a change that cannot surprise anyone.
    pub confirmation_required: bool,
    pub note: &'static str,
}

/// Build a plan hash from the request and the state it depends on.
///
/// Both are canonical JSON (`serde_json::Value` sorts map keys), so the same inputs always produce
/// the same hash — the property the whole mechanism rests on. A hash that varied run to run would
/// refuse every commit.
pub fn hash(request: &serde_json::Value, state: &serde_json::Value) -> String {
    let mut h = Sha256::new();
    h.update(b"heldar-plan/1\n");
    h.update(serde_json::to_vec(request).unwrap_or_default());
    h.update(b"\n");
    h.update(serde_json::to_vec(state).unwrap_or_default());
    format!("{:x}", h.finalize())
}

/// Why a commit was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The caller supplied a hash from a plan that no longer describes reality.
    Stale { supplied: String, current: String },
}

impl Refusal {
    /// The message an operator or agent actually acts on.
    pub fn message(&self) -> String {
        match self {
            Self::Stale { supplied, current } => format!(
                "the plan you are committing is out of date: it was computed against state \
                 {supplied}, and the box is now at {current}. Something changed between planning \
                 and committing — re-run with `dry_run` and check the new effect before \
                 committing. Sending the new hash without reading the new plan defeats the point \
                 of asking."
            ),
        }
    }
}

/// Check a supplied plan hash against the current one.
///
/// `None` supplied means the caller did not plan. That is ALLOWED here and refused by the caller if
/// the change warrants it — a plan hash is a safety belt for automation, not a way to stop a human
/// with an admin key from changing a setting directly.
pub fn check(supplied: Option<&str>, current: &str) -> Result<(), Refusal> {
    match supplied {
        None => Ok(()),
        Some(s) if s.trim() == current => Ok(()),
        Some(s) => Err(Refusal::Stale {
            supplied: s.trim().to_string(),
            current: current.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The hash must be a pure function of its inputs. If it drifted — a timestamp, a HashMap
    /// iteration order — every commit would be refused and the mechanism would be worse than absent.
    #[test]
    fn the_same_inputs_always_hash_the_same() {
        let req = json!({"max_recordings_gb": 20.0});
        let state = json!({"current_bytes": 1234, "cameras": 4});
        let a = hash(&req, &state);
        for _ in 0..5 {
            assert_eq!(hash(&req, &state), a, "the hash must not vary between runs");
        }
        // Key order in the source JSON must not matter — `serde_json::Value` sorts.
        let reordered: serde_json::Value =
            serde_json::from_str(r#"{"cameras": 4, "current_bytes": 1234}"#).unwrap();
        assert_eq!(hash(&req, &reordered), a);
    }

    #[test]
    fn a_different_request_or_a_changed_state_changes_the_hash() {
        let req = json!({"max_recordings_gb": 20.0});
        let state = json!({"current_bytes": 1234});
        let base = hash(&req, &state);
        assert_ne!(base, hash(&json!({"max_recordings_gb": 21.0}), &state));
        assert_ne!(base, hash(&req, &json!({"current_bytes": 9999})));
    }

    /// The point of the whole mechanism: a plan computed before someone else moved the state must
    /// not commit silently.
    #[test]
    fn a_stale_plan_is_refused_and_says_what_to_do() {
        assert!(check(Some("abc"), "abc").is_ok());
        assert!(check(None, "abc").is_ok(), "not planning is allowed");
        assert!(
            check(Some("  abc  "), "abc").is_ok(),
            "whitespace is not the hash"
        );

        let err = check(Some("old"), "new").expect_err("a moved state must refuse");
        let msg = err.message();
        assert!(msg.contains("out of date"), "{msg}");
        assert!(
            msg.contains("dry_run"),
            "it must say how to recover, not just that it failed: {msg}"
        );
        assert!(
            msg.contains("without reading"),
            "and warn against the obvious wrong fix — pasting the new hash back without looking is \
             exactly what an agent under time pressure will try: {msg}"
        );
    }
}
