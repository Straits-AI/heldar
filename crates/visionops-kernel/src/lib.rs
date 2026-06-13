//! VisionOps kernel library.
//!
//! The domain-agnostic platform: media/DVR control plane (camera registry, RTSP ingest, recording,
//! timeline index, playback, live view), the perception ingest + sampler framework and its
//! `DetectionConsumer` seam, the zone engine, the auth primitive, observability, retention, and the
//! worker SDK contract. Domain applications (Campus Entry, BakerySense, …) link this crate and plug
//! in as consumers / route modules via the composing server binary.
//!
//! NOTE: the Campus Entry app modules (`services::anpr`, `routes::entry`, the RBAC half of `auth`,
//! and migration 0005) currently live here too; they are slated to move into a separate proprietary
//! crate, at which point this crate becomes Apache-2.0.

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
