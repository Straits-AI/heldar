//! VisionOps Campus Entry — PROPRIETARY domain application.
//!
//! Built on the open `visionops-kernel` platform. Provides ANPR authorization (plate → registry
//! resolution → canonical entry event), the vehicle/visitor-pass/watchlist registry, the guard
//! confirm/reject workflow, and entry/exception/audit reports. It plugs into the kernel purely
//! through public seams: [`visionops_kernel::services::consumer::DetectionConsumer`] (the ANPR
//! engine), [`visionops_kernel::state::AppState`] + the shared SQLite pool, the auth primitive, and
//! the error/model types. The kernel has no dependency on this crate — the composing server links it.

pub mod anpr;
pub mod models;
pub mod routes;
