//! The kernel's perception-consumer seam.
//!
//! The media + AI-task kernel is domain-agnostic: it samples frames, accepts detections from workers,
//! and persists them. Anything that *interprets* those detections into domain events — the zone engine
//! (spatial rules), the ANPR/Campus-Entry engine (plate authorization), and future apps like
//! BakerySense (retail behaviour) — plugs in here as a [`DetectionConsumer`] rather than being wired
//! into the ingest handler or [`crate::state::AppState`] directly.
//!
//! This inverts the dependency: ingest fans a committed batch out to a registry of consumers; a
//! consumer self-declares which `task_type`s it cares about. New apps are *added* to the registry (and,
//! after the crate split, linked in by the composing binary or fed over the event stream) — the kernel
//! never gains an `if task_type == "..."` branch.

use chrono::{DateTime, Utc};

use crate::models::DetectionIngest;

/// One committed batch of detections handed to consumers after it is persisted. Carries the
/// site/camera/task context so a consumer needs no extra lookups (and so the seam is tenant-aware for
/// distributed deployments).
pub struct DetectionBatch<'a> {
    pub camera_id: &'a str,
    pub site_id: Option<&'a str>,
    // `task_type` and `timestamp` are part of the stable seam contract for consumers that need them
    // (e.g. BakerySense will window by time); the current consumers select by task_type at the
    // registry level and use server time, so they are not read here yet. (Once the kernel is its own
    // library crate these are externally-public and the lint is moot.)
    #[allow(dead_code)]
    pub task_type: &'a str,
    pub detections: &'a [DetectionIngest],
    /// Worker-supplied capture time (engines that need trustworthy timing use server time instead).
    #[allow(dead_code)]
    pub timestamp: DateTime<Utc>,
}

/// A pluggable interpreter of detection batches. Implementors live in their own module/crate (zones is
/// kernel-open; ANPR/entry is a proprietary app) and are registered into [`crate::state::AppState`].
#[async_trait::async_trait]
pub trait DetectionConsumer: Send + Sync {
    /// Stable name for logs/metrics.
    fn name(&self) -> &'static str;

    /// Whether this consumer wants batches of the given `task_type`. Return `true` for all types when
    /// the consumer is task-agnostic (e.g. the zone engine evaluates any tracked detection).
    fn interested_in(&self, task_type: &str) -> bool;

    /// Process a persisted batch. Must not panic; errors are the consumer's own to log/handle.
    async fn consume(&self, batch: &DetectionBatch<'_>);
}
