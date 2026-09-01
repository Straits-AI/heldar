//! Concurrency governor for interactive media jobs (playback session builds, clip exports,
//! snapshots).
//!
//! Every one of these forks ffmpeg/ffprobe and does real disk I/O: a two-hour playback session
//! probes and remuxes every overlapping segment, which at two-second segments is thousands of files.
//! Nothing bounded how many could run at once, so any authenticated caller could start as many as
//! they liked and starve the box — and the process being starved is the RECORDER, which is the one
//! thing that must never miss.
//!
//! The governor is deliberately a global ceiling rather than a fair queue. Interactive media work is
//! a burst of a few operators reviewing footage, not a throughput workload; the failure this exists
//! to prevent is "the appliance stops recording because someone opened twelve playback windows", and
//! a ceiling is the smallest thing that prevents it. A caller that cannot get a permit inside
//! [`ACQUIRE_TIMEOUT`] is told to retry rather than queued indefinitely, so a saturated box answers
//! honestly instead of accumulating a backlog it will never drain.
//!
//! Recording deliberately does NOT pass through here. The recorder's ffmpegs are long-lived and
//! supervised, and making them wait on a permit held by an export is exactly the inversion this
//! module exists to avoid.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::error::{AppError, AppResult};

/// How long a caller waits for a permit before being told to retry. Long enough to ride out a
/// neighbouring snapshot (sub-second) or a short clip, short enough that an HTTP client is not left
/// hanging on a saturated box.
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);

/// Bounds concurrent media jobs across the whole process.
#[derive(Clone)]
pub struct MediaJobGovernor {
    sem: Arc<Semaphore>,
    limit: usize,
}

impl MediaJobGovernor {
    /// `limit` is clamped to at least 1 — a zero would deadlock every export rather than disable the
    /// feature, which is never what an operator means by setting it.
    pub fn new(limit: usize) -> Self {
        let limit = limit.max(1);
        Self {
            sem: Arc::new(Semaphore::new(limit)),
            limit,
        }
    }

    /// Permits currently available (for `/metrics` and the system page).
    pub fn available(&self) -> usize {
        self.sem.available_permits()
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Acquire a slot for `job`, or fail with 503 after [`ACQUIRE_TIMEOUT`].
    ///
    /// The permit is released when the returned guard drops, which happens even if the job fails or
    /// its future is cancelled mid-flight (a client disconnecting mid-export) — so a permit cannot be
    /// leaked by an error path.
    pub async fn acquire(&self, job: &str) -> AppResult<OwnedSemaphorePermit> {
        self.acquire_within(job, ACQUIRE_TIMEOUT).await
    }

    /// [`acquire`](Self::acquire) with an explicit wait budget. Separated so the saturation path is
    /// testable without manipulating the clock (which would need tokio's `test-util` feature).
    async fn acquire_within(&self, job: &str, wait: Duration) -> AppResult<OwnedSemaphorePermit> {
        match tokio::time::timeout(wait, self.sem.clone().acquire_owned()).await {
            Ok(Ok(permit)) => Ok(permit),
            // The semaphore is never closed; this arm exists so a future change cannot turn a closed
            // semaphore into a panic on the request path.
            Ok(Err(_)) => Err(AppError::Unavailable(
                "media job scheduler is shutting down".into(),
            )),
            Err(_) => {
                tracing::warn!(
                    job,
                    limit = self.limit,
                    "media job governor saturated; rejecting with 503"
                );
                Err(AppError::Unavailable(format!(
                    "too many media jobs in flight ({} running); retry shortly",
                    self.limit
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn permits_are_bounded_and_released_on_drop() {
        let g = MediaJobGovernor::new(2);
        assert_eq!(g.available(), 2);
        let a = g.acquire("clip").await.unwrap();
        let b = g.acquire("clip").await.unwrap();
        assert_eq!(g.available(), 0);
        drop(a);
        assert_eq!(g.available(), 1);
        drop(b);
        assert_eq!(g.available(), 2);
    }

    #[tokio::test]
    async fn a_saturated_governor_rejects_rather_than_queueing_forever() {
        let g = MediaJobGovernor::new(1);
        let _held = g.acquire("playback").await.unwrap();
        let err = g
            .acquire_within("playback", Duration::from_millis(50))
            .await
            .unwrap_err();
        assert!(
            matches!(err, AppError::Unavailable(_)),
            "a saturated governor must answer 503, got {err:?}"
        );
    }

    #[tokio::test]
    async fn a_zero_limit_is_clamped_rather_than_deadlocking() {
        let g = MediaJobGovernor::new(0);
        assert_eq!(g.limit(), 1);
        let _p = g.acquire("snapshot").await.expect("must still be usable");
    }
}
