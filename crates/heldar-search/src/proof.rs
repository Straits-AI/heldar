//! The proof layer. Every answer is decomposed into claim levels, each with its
//! evidence + confidence, so a result is always traceable to the facts it rests on — and the one
//! genuinely uncertain step (the natural-language → plan interpretation) is surfaced as such.
//!
//! Claim ladder, lowest (most certain) to highest (most interpretive):
//!   observation → track → event → aggregate → inference
//! The platform stores facts at the `event` level (entry/zone/breach rows), each backed by
//! `observation`/`track` provenance in the kernel `detections` table and an evidence frame; this layer
//! adds the `aggregate` (the executed count/breakdown) and the `inference` (how the question was read).

use serde_json::{json, Value};

use crate::query::{breakdown, QueryPlan, SearchHit};

/// Build the proof object for a result set. `planner` is "rules" | "llm" | "structured".
pub fn build(query: Option<&str>, planner: &str, plan: &QueryPlan, hits: &[SearchHit]) -> Value {
    let n = hits.len();
    let mut levels: Vec<Value> = Vec::new();

    // inference — the only non-deterministic step: how the question was turned into a plan.
    if let Some(q) = query {
        levels.push(json!({
            "level": "inference",
            "statement": format!("Interpreted the question \"{q}\" as the structured plan below."),
            "confidence": if planner == "llm" { "medium" } else { "medium-low" },
            "fallible": true,
            "evidence": { "planner": planner, "plan": plan },
            "caveat": "This interpretation is the only inference in the answer. Verify the plan reflects \
                       your intent; the results are exactly what the plan selected, nothing more.",
        }));
    }

    // aggregate — the executed query result. Deterministic over stored facts. Report the EFFECTIVE
    // window actually scanned (the default 7-day window when the plan left from/to unset).
    let (eff_from, eff_to) = crate::query::window(plan);
    let defaulted = plan.from.is_none() || plan.to.is_none();
    levels.push(json!({
        "level": "aggregate",
        "statement": format!("{n} stored event(s) match the executed plan in the queried window."),
        "confidence": "high",
        "basis": "Deterministic SQL over kernel fact tables (entry_events, zone_events, breach_alerts); \
                  the answer is these rows, not model output.",
        "evidence": {
            "count": n,
            "breakdown": breakdown(hits),
            "window": { "from": eff_from.to_rfc3339(), "to": eff_to.to_rfc3339(), "defaulted": defaulted },
        },
    }));

    // event — each hit is an event-level claim; its provenance + evidence are spelled out.
    levels.push(json!({
        "level": "event",
        "statement": format!("{n} event claim(s); each links to its source row + evidence frame."),
        "confidence": "per-event (see auth_status / plate_confidence / severity on each hit)",
        "provenance": "Each event was derived by the kernel from observation+track data in the \
                       `detections` table; pull the clip for any hit via the kernel clip API \
                       (POST /api/v1/cameras/{camera_id}/clip) using its timestamp window, and the \
                       evidence frame via its evidence_path.",
        "evidence": { "hit_ids": hits.iter().take(50).map(|h| json!({ "source": h.source, "id": h.id, "evidence_path": h.evidence_path })).collect::<Vec<_>>() },
    }));

    json!({
        "claim_levels": levels,
        "note": "Proof ladder observation→track→event→aggregate→inference. Facts are at the event level \
                 and below (kernel-produced); this search adds the aggregate (a deterministic query) and \
                 the inference (the NL→plan reading). No layer asserts identity or causation.",
    })
}
