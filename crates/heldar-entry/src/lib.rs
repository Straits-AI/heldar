//! Heldar Access Control — generic, **open (Apache-2.0)** reference app.
//!
//! Built on the open `heldar-kernel` platform. Provides ANPR authorization (plate → registry
//! resolution → canonical entry event), the vehicle/visitor-pass/watchlist registry, the guard
//! confirm/reject workflow, and entry/exception/audit reports. It is domain-neutral — any gated-entry
//! deployment (campus, residential, corporate, industrial) uses it as-is. It plugs into the kernel
//! purely through public seams: [`heldar_kernel::services::consumer::DetectionConsumer`] (the ANPR
//! engine), [`heldar_kernel::state::AppState`] + the shared SQLite pool, the auth primitive, and
//! the error/model types. The kernel has no dependency on this crate — the composing server links it.
//!
//! Vertical/client products (e.g. a proprietary `heldar-campus-entry` adding students/guardians,
//! pickup/dismissal, and parental-app integration) depend on THIS crate and layer their specifics on
//! top; the generic access-control core stays open. See `ARCHITECTURE.md` for the open-core split.

pub mod anpr;
pub mod config;
pub mod models;
pub mod retention;
pub mod routes;
pub mod schema;
