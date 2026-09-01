//! Shared task-sharding for multi-worker AI deployments.
//!
//! Two paths must agree on which worker owns which task: `GET /ai/tasks` (what a worker is told to
//! analyze) and task-lease acquisition (what it is allowed to lease, and therefore get frame tickets
//! for). When they disagreed, leasing was greedy — the first worker to poll took every task, kept
//! renewing, and starved every peer, so a fleet silently collapsed to one active node while the task
//! list still claimed to be sharded. Keeping the assignment in one place is what stops that drifting
//! apart again.

use std::collections::HashSet;

use sqlx::SqlitePool;

/// How long an AI worker's heartbeat stays live before it is considered gone.
pub const WORKER_LIVENESS_TTL_SECS: i64 = 60;

/// Modulo sharding (task `i` → `live[i % n]`): balanced (each worker gets ~total/n) and stable
/// (reassigns only when the worker SET changes).
///
/// Defensive: returns ALL indices when `live` is empty or `me` is absent, so a worker never silently
/// gets nothing due to a race. Worst case it redoes tasks, which the outbox `frame_id` idempotency
/// dedups — whereas getting nothing would stall analysis entirely.
pub fn assign(total: usize, live: &[String], me: &str) -> Vec<usize> {
    let n = live.len();
    match live.iter().position(|w| w == me) {
        Some(idx) if n > 0 => (0..total).filter(|i| i % n == idx).collect(),
        _ => (0..total).collect(),
    }
}

/// The task indices assigned to `me`, resolved against the currently live workers.
///
/// Read-only: it does NOT heartbeat or prune. Registration and pruning belong to the `GET /ai/tasks`
/// path, so that leasing cannot resurrect or evict a worker as a side effect of asking what it owns.
/// The task indices assigned to `me` among the workers of ONE CREDENTIAL.
///
/// `for_worker` divides by every live worker on the box, which is correct only while every worker
/// sees the same task list. A camera-scoped credential does not: its list is filtered. Counting a
/// scoped worker in the fleet-wide denominator shifts everyone's shard and leaves tasks that fall in
/// the scoped worker's slots owned by nobody — executed, and it silently stopped analysis on a camera
/// unrelated to the scoped credential.
///
/// Workers on one api key share a camera scope and therefore a task view, which makes the api key the
/// correct partition: within it, `assign`'s "every task owned exactly once" invariant holds again.
pub async fn for_credential(
    pool: &SqlitePool,
    total: usize,
    api_key_id: &str,
    me: &str,
) -> HashSet<usize> {
    let cutoff = chrono::Utc::now() - chrono::Duration::seconds(WORKER_LIVENESS_TTL_SECS);
    let live: Vec<String> = sqlx::query_scalar(
        "SELECT worker_id FROM ai_workers
          WHERE last_seen >= ? AND api_key_id IS ? ORDER BY worker_id ASC",
    )
    .bind(cutoff)
    .bind(api_key_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    // A worker that has not registered under this key yet still needs a shard; treat it as present
    // rather than handing it an empty set (which would look like "nothing to do" and stall it).
    let live = if live.iter().any(|w| w == me) {
        live
    } else {
        let mut v = live;
        v.push(me.to_string());
        v.sort();
        v
    };
    assign(total, &live, me).into_iter().collect()
}

pub async fn for_worker(pool: &SqlitePool, total: usize, me: &str) -> HashSet<usize> {
    let cutoff = chrono::Utc::now() - chrono::Duration::seconds(WORKER_LIVENESS_TTL_SECS);
    let live: Vec<String> = sqlx::query_scalar(
        "SELECT worker_id FROM ai_workers WHERE last_seen >= ? ORDER BY worker_id ASC",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    assign(total, &live, me).into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shards_are_balanced_and_disjoint() {
        let live: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        let a = assign(9, &live, "a");
        let b = assign(9, &live, "b");
        let c = assign(9, &live, "c");
        assert_eq!(a, vec![0, 3, 6]);
        assert_eq!(b, vec![1, 4, 7]);
        assert_eq!(c, vec![2, 5, 8]);
        // Every task is owned exactly once — the property that stops two workers double-leasing and
        // stops a task falling through the gaps.
        let mut all: Vec<usize> = a.into_iter().chain(b).chain(c).collect();
        all.sort_unstable();
        assert_eq!(all, (0..9).collect::<Vec<_>>());
    }

    #[test]
    fn a_lone_worker_owns_everything() {
        let live: Vec<String> = vec!["solo".into()];
        assert_eq!(assign(4, &live, "solo"), vec![0, 1, 2, 3]);
    }

    #[test]
    fn an_unknown_or_empty_worker_set_falls_back_to_everything() {
        // Failing OPEN here is deliberate: a worker racing its own registration should redo work
        // (deduped downstream), never stall with an empty assignment.
        assert_eq!(assign(3, &[], "ghost"), vec![0, 1, 2]);
        let live: Vec<String> = vec!["a".into(), "b".into()];
        assert_eq!(assign(3, &live, "not-registered"), vec![0, 1, 2]);
    }
}
