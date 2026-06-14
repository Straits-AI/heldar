//! Heldar semantic search — generic, **open (Apache-2.0)** searchable visual-event memory.
//!
//! Turns the platform's accumulated event facts into a queryable memory — *who / what / where / when /
//! confidence / evidence* — under one governing principle:
//!
//! **The LLM is a query PLANNER, never the source of truth.** A natural-language question is translated
//! into a structured query PLAN (a deterministic filter), the plan is executed against the kernel's
//! stored facts (entry_events, zone_events, breach_alerts), and the ANSWER is those rows — not anything
//! the model "said". When no LLM is configured, a transparent rule-based parser produces the same kind
//! of plan, so the feature works fully offline. Either way the plan is shown back to the caller.
//!
//! **Proof layer.** Every answer is decomposed into claim levels — observation → track → event →
//! aggregate → inference — each carrying its evidence (source row ids, clip pointers) and a confidence,
//! so a result can always be traced back to the facts it rests on, and the *interpretation* step (the
//! NL→plan translation) is itself surfaced as an explicitly-fallible inference claim.
//!
//! It is a read-only query layer over stored kernel/app data — not a DetectionConsumer; it owns only a
//! small query log (audit/history) and its routes, and is composed by the server. Identity-bearing
//! queries are audited. Open-vocabulary VLM enrichment + event/clip embedding vector-retrieval are a
//! documented future seam (they need an embedding/VLM worker); this stage ships the deterministic
//! structured + NL-plan + proof core.

pub mod config;
pub mod planner;
pub mod proof;
pub mod query;
pub mod routes;
pub mod schema;
