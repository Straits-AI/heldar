//! Heldar kernel library.
//!
//! The domain-agnostic, **open (Apache-2.0)** platform: media/DVR control plane (camera registry,
//! RTSP ingest, recording, timeline index, playback, live view), the perception ingest + sampler
//! framework and its `DetectionConsumer` seam, the zone engine, auth/RBAC, observability, retention,
//! remote-access overlay awareness, and the worker SDK contract. Domain applications link this crate
//! and plug in as consumers / route modules via the composing server binary.
//!
//! Generic reference apps (`heldar-entry`, `heldar-movement`, `heldar-search`) are also
//! Apache-2.0 and live alongside this crate; proprietary vertical/client products depend on the
//! generic ones. See `ARCHITECTURE.md` for the open-core
//! split and `docs/REMOTE-ACCESS.md` for the remote-access model.

pub mod auth;
pub mod camera_url;
pub mod config;
pub mod db;
pub mod error;
pub mod models;
pub mod repo;
pub mod routes;
pub mod services;
pub mod state;
pub mod util;
