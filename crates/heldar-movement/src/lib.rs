//! Heldar Movement intelligence — generic, **open (Apache-2.0)** cross-camera ReID + trails +
//! breach alerts.
//!
//! A domain-neutral correlation layer: it links the kernel's per-camera observations into
//! cross-camera journeys and flags red-zone breaches — under strict privacy gates:
//!
//! * **Multi-signal, never pure visual embedding.** Vehicle ReID is anchored on the PLATE (already
//!   resolved by `heldar-entry` into `entry_events`), fused with vehicle attributes (color/type),
//!   the
//!   operator's camera-topology graph, and transit-time plausibility. Person ReID has no plate and no
//!   appearance embedding here, so it is offered only as a low-confidence, on-demand candidate search
//!   over topology + time + coarse attributes — explicitly weak, for human triage.
//! * **Candidate matching, not identity.** Every cross-camera link is a scored *candidate* with
//!   per-signal evidence and a confidence; a human confirms or rejects it. Nothing is asserted as
//!   legal identity.
//! * **Audited.** Every identity-like query (search) writes a kernel audit-log entry.
//!
//! It is an analytics/correlation layer over stored kernel data (entry_events, detections,
//! zone_events) — not a DetectionConsumer. It owns its schema, config, background engines (candidate
//! proposer + breach rule engine), retention, and routes, and is composed by the server. The kernel
//! has no dependency on it. Single-tenant-per-deployment ⇒ shares the kernel pool.

pub mod breach;
pub mod config;
pub mod models;
pub mod reid;
pub mod routes;
pub mod schema;
