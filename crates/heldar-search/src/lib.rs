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
//! queries are audited.
//!
//! **Semantic retrieval (issue #38).** `POST /api/v1/search/semantic` ranks the kernel's stored
//! detection-crop embeddings (CLIP, produced by the AI worker's `embedding` task) by cosine
//! similarity to a text or image query — the query itself is embedded via the kernel's pull-only
//! `embed_queries` job queue, so no worker means a clean 503, never a fabricated answer. Results
//! are explicitly similarity-ranked retrievals, not facts, and the proof layer says so.
//! Open-vocabulary VLM interpretation over retrieved moments remains a documented future seam.

pub mod config;
pub mod planner;
pub mod proof;
pub mod query;
pub mod retention;
pub mod routes;
pub mod schema;
pub mod semantic;

/// This app's module manifest (served at `GET /api/v1/modules` so the dashboard renders its nav).
pub fn manifest() -> heldar_kernel::modules::ModuleManifest {
    use heldar_kernel::modules::{ModuleKind, ModuleManifest, NavEntry};
    ModuleManifest::new(
        "search",
        "Forensic Search",
        env!("CARGO_PKG_VERSION"),
        "Heldar",
        ModuleKind::Core,
        "Natural-language + structured query over stored event facts, with a traceable proof layer.",
        vec![NavEntry::new("/search", "Search", "search")],
    )
    // The UI is a runtime-loaded bundle this crate serves (see routes.rs), not compiled into the shell.
    .with_runtime_ui("/api/v1/modules/search/ui/index.js")
}
