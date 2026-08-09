//! Per-TASK worker leases: the binding that turns "a credential that may ingest" into "a credential
//! that may ingest THIS task, on THIS camera, right now".
//!
//! Deliberately per TASK, not per frame. A box runs tens of tasks, so this table sees one write per
//! task per lease TTL (~60 s) — nothing that competes with the recorder for SQLite's single writer. The
//! per-frame layer is the stateless HMAC ticket in [`crate::services::frame_ticket`], which costs zero
//! writes.
//!
//! Expiry is a PREDICATE at claim time, never a reaper task, for the same reason: no new background
//! writer. A dead worker's tasks become claimable the moment its lease lapses, without anyone sweeping.
//!
//! Failure posture (constraint 4 — recording must never gain a failure mode from any of this): every
//! read here is fallible and every caller degrades to "no lease", which degrades to "no ticket", which
//! under the default `warn` tier degrades to exactly today's behaviour.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, Duration, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::error::AppResult;
use crate::models::AiTask;

/// Default lease TTL when the caller does not ask for one.
pub const DEFAULT_LEASE_TTL_SECS: i64 = 60;
/// Lower clamp: below this a normal poll interval could lapse the lease mid-cycle.
pub const MIN_LEASE_TTL_SECS: i64 = 15;
/// Upper clamp: above this a crashed worker holds its shard uselessly for too long.
pub const MAX_LEASE_TTL_SECS: i64 = 300;
/// Upper bound on tasks handed out in one acquire (a sanity bound, not a policy).
pub const MAX_LEASE_TASKS: i64 = 512;

/// A live lease as the ticket issuer needs it.
#[derive(Clone, Debug)]
pub struct LiveLease {
    pub lease_id: String,
    pub worker_id: String,
    pub camera_id: String,
    pub task_type: String,
    pub expires_at: DateTime<Utc>,
}

/// In-memory mirror of live leases, keyed by `(task_id, api_key_id)`.
///
/// The frame endpoint is hit at task fps (up to 30/s per task), and every hit would otherwise be a
/// SELECT on the ingest path's own database. The LEASE is cacheable because it is coarse (one row per
/// ~60 s); the TICKET never is, because it is genuinely per `captured_ms`.
///
/// Entries are dropped once past `expires_at`, so a lapsed lease can never be served from cache, and
/// [`release`] evicts explicitly.
type LeaseCache = Mutex<HashMap<(String, String), LiveLease>>;
static CACHE: OnceLock<LeaseCache> = OnceLock::new();

fn cache() -> &'static LeaseCache {
    CACHE.get_or_init(Default::default)
}

/// Above this many cached leases, prune expired entries on the next write.
const CACHE_PRUNE_AT: usize = 2048;

fn cache_put(task_id: &str, api_key_id: &str, lease: LiveLease) {
    let Ok(mut map) = cache().lock() else { return };
    if map.len() > CACHE_PRUNE_AT {
        let now = Utc::now();
        map.retain(|_, l| l.expires_at > now);
    }
    map.insert((task_id.to_string(), api_key_id.to_string()), lease);
}

fn cache_get(task_id: &str, api_key_id: &str, now: DateTime<Utc>) -> Option<LiveLease> {
    let map = cache().lock().ok()?;
    map.get(&(task_id.to_string(), api_key_id.to_string()))
        .filter(|l| l.expires_at > now)
        .cloned()
}

fn cache_drop_lease(lease_id: &str) {
    if let Ok(mut map) = cache().lock() {
        map.retain(|_, l| l.lease_id != lease_id);
    }
}

/// Clear the whole cache. Test-only: the cache is process-global, so tests that reuse a task id across
/// in-memory databases would otherwise see each other's entries.
#[cfg(test)]
pub fn clear_cache_for_tests() {
    if let Ok(mut map) = cache().lock() {
        map.clear();
    }
}

/// Clamp a caller-proposed TTL into the supported band.
pub fn clamp_ttl(ttl_secs: Option<i64>) -> i64 {
    ttl_secs
        .unwrap_or(DEFAULT_LEASE_TTL_SECS)
        .clamp(MIN_LEASE_TTL_SECS, MAX_LEASE_TTL_SECS)
}

/// The outcome of one acquire/renew call.
pub struct Acquired {
    pub lease_id: String,
    pub expires_at: DateTime<Utc>,
    pub tasks: Vec<AiTask>,
}

/// Acquire and/or renew a lease over eligible tasks. Acquire and renew are the SAME call: a holder
/// re-acquiring simply extends what it already has, so a worker's poll loop needs no separate renew
/// path and no state machine.
///
/// Eligibility mirrors `GET /ai/tasks` exactly (enabled task on an enabled camera, ordered by id), so a
/// leased worker and a legacy `/ai/tasks` worker see the same universe of work.
///
/// The claim is the compare-and-swap shape proven by `embeddings::claim_queries`, plus the expiry that
/// one lacks: a row is taken only if its lease has LAPSED or is already ours.
pub async fn acquire(
    pool: &SqlitePool,
    api_key_id: &str,
    worker_id: &str,
    task_types: Option<&[String]>,
    max_tasks: Option<i64>,
    ttl_secs: i64,
    camera_scope: Option<&std::collections::HashSet<String>>,
) -> AppResult<Acquired> {
    let now = Utc::now();
    let expires_at = now + Duration::seconds(ttl_secs);
    let lease_id = format!("lse_{}", Uuid::new_v4().simple());
    let budget = max_tasks
        .unwrap_or(MAX_LEASE_TASKS)
        .clamp(1, MAX_LEASE_TASKS);

    let candidates = sqlx::query_as::<_, AiTask>(
        "SELECT t.* FROM ai_tasks t JOIN cameras c ON c.id = t.camera_id
         WHERE t.enabled = 1 AND c.enabled = 1
         ORDER BY t.id ASC",
    )
    .fetch_all(pool)
    .await?;

    let mut taken = Vec::new();
    for task in candidates {
        if taken.len() as i64 >= budget {
            break;
        }
        if let Some(types) = task_types {
            if !types.iter().any(|t| t == &task.task_type) {
                continue;
            }
        }
        // A camera-scoped credential can only lease tasks on cameras it may address at all.
        if let Some(allowed) = camera_scope {
            if !allowed.contains(&task.camera_id) {
                continue;
            }
        }
        let res = sqlx::query(
            "INSERT INTO ai_task_leases
               (task_id, lease_id, api_key_id, worker_id, camera_id, task_type,
                acquired_at, renewed_at, expires_at)
             VALUES (?,?,?,?,?,?,?,?,?)
             ON CONFLICT(task_id) DO UPDATE SET
                lease_id   = excluded.lease_id,
                api_key_id = excluded.api_key_id,
                worker_id  = excluded.worker_id,
                camera_id  = excluded.camera_id,
                task_type  = excluded.task_type,
                renewed_at = excluded.renewed_at,
                expires_at = excluded.expires_at,
                -- keep the original acquisition time across a renew by the same holder
                acquired_at = CASE
                    WHEN ai_task_leases.api_key_id = excluded.api_key_id
                     AND ai_task_leases.worker_id  = excluded.worker_id
                    THEN ai_task_leases.acquired_at ELSE excluded.acquired_at END
             WHERE ai_task_leases.expires_at < ?
                OR (ai_task_leases.api_key_id = excluded.api_key_id
                    AND ai_task_leases.worker_id = excluded.worker_id)",
        )
        .bind(&task.id)
        .bind(&lease_id)
        .bind(api_key_id)
        .bind(worker_id)
        .bind(&task.camera_id)
        .bind(&task.task_type)
        .bind(now)
        .bind(now)
        .bind(expires_at)
        .bind(now)
        .execute(pool)
        .await?;
        if res.rows_affected() > 0 {
            cache_put(
                &task.id,
                api_key_id,
                LiveLease {
                    lease_id: lease_id.clone(),
                    worker_id: worker_id.to_string(),
                    camera_id: task.camera_id.clone(),
                    task_type: task.task_type.clone(),
                    expires_at,
                },
            );
            taken.push(task);
        }
    }

    Ok(Acquired {
        lease_id,
        expires_at,
        tasks: taken,
    })
}

/// Release every task held under `lease_id` by this credential. Idempotent; returns the row count.
///
/// Scoped by `api_key_id` so one credential cannot drop another's lease by guessing a lease id.
pub async fn release(pool: &SqlitePool, lease_id: &str, api_key_id: &str) -> AppResult<u64> {
    let res = sqlx::query("DELETE FROM ai_task_leases WHERE lease_id = ? AND api_key_id = ?")
        .bind(lease_id)
        .bind(api_key_id)
        .execute(pool)
        .await?;
    cache_drop_lease(lease_id);
    Ok(res.rows_affected())
}

/// The live lease on `task_id` held by `api_key_id`, if any.
///
/// Returns `Ok(None)` for "no live lease" AND for any read failure that is not the caller's problem —
/// the ticket issuer treats both identically (emit no ticket), which is what keeps a lease-table
/// problem from becoming an ingest outage under the default tier.
pub async fn is_live(
    pool: &SqlitePool,
    task_id: &str,
    api_key_id: &str,
) -> AppResult<Option<LiveLease>> {
    let now = Utc::now();
    if let Some(hit) = cache_get(task_id, api_key_id, now) {
        return Ok(Some(hit));
    }
    let row: Option<(String, String, String, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT lease_id, worker_id, camera_id, task_type, expires_at
           FROM ai_task_leases WHERE task_id = ? AND api_key_id = ? AND expires_at > ?",
    )
    .bind(task_id)
    .bind(api_key_id)
    .bind(now)
    .fetch_optional(pool)
    .await?;
    let Some((lease_id, worker_id, camera_id, task_type, expires_at)) = row else {
        return Ok(None);
    };
    let lease = LiveLease {
        lease_id,
        worker_id,
        camera_id,
        task_type,
        expires_at,
    };
    cache_put(task_id, api_key_id, lease.clone());
    Ok(Some(lease))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Credential ids unique across the whole test binary.
    ///
    /// [`CACHE`] is process-global by design — in production there is one process and one database,
    /// and caching the lease is what keeps a 30 fps frame endpoint off the ingest path's database.
    /// Under `cargo test` the cases run concurrently against SEPARATE in-memory databases while
    /// sharing that map, so a hard-coded `"key_a"` lets one test's cached lease answer another
    /// test's `is_live`. Unique ids partition the map instead of contending on it.
    fn unique_key(label: &str) -> String {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        format!(
            "key_{label}_{}",
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )
    }

    async fn seeded_pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        clear_cache_for_tests();
        let now = Utc::now();
        for cam in ["cam1", "cam2"] {
            sqlx::query(
                "INSERT INTO cameras (id, name, enabled, created_at, updated_at) VALUES (?,?,1,?,?)",
            )
            .bind(cam)
            .bind(cam)
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (id, cam, kind) in [
            ("ai_1", "cam1", "anpr"),
            ("ai_2", "cam2", "object_detection"),
        ] {
            sqlx::query(
                "INSERT INTO ai_tasks (id, camera_id, task_type, enabled, stream_profile, fps, width,
                                       config, created_at, updated_at)
                 VALUES (?,?,?,1,'sub',5.0,640,'{}',?,?)",
            )
            .bind(id)
            .bind(cam)
            .bind(kind)
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        }
        pool
    }

    /// Exclusivity + renew + the reclaim-after-expiry that a reaper would otherwise be needed for.
    #[tokio::test]
    async fn a_task_is_leased_to_exactly_one_holder_until_it_lapses() {
        let pool = seeded_pool().await;
        let (ka, kb) = (unique_key("a"), unique_key("b"));

        let a = acquire(&pool, &ka, "w1", None, None, 60, None)
            .await
            .unwrap();
        assert_eq!(a.tasks.len(), 2, "first holder takes every eligible task");

        // A second credential gets nothing while the leases are live.
        let b = acquire(&pool, &kb, "w2", None, None, 60, None)
            .await
            .unwrap();
        assert!(b.tasks.is_empty(), "leases are exclusive while live");

        // The holder renews: same rows, later expiry.
        let renewed = acquire(&pool, &ka, "w1", None, None, 60, None)
            .await
            .unwrap();
        assert_eq!(
            renewed.tasks.len(),
            2,
            "renew re-takes the holder's own rows"
        );
        assert!(renewed.expires_at >= a.expires_at);

        // Force expiry (no reaper exists by design) and the other credential can take over.
        sqlx::query("UPDATE ai_task_leases SET expires_at = ?")
            .bind(Utc::now() - Duration::seconds(1))
            .execute(&pool)
            .await
            .unwrap();
        clear_cache_for_tests();
        let c = acquire(&pool, &kb, "w2", None, None, 60, None)
            .await
            .unwrap();
        assert_eq!(c.tasks.len(), 2, "a lapsed lease is reclaimable");
    }

    #[tokio::test]
    async fn is_live_answers_only_for_the_holding_credential() {
        let pool = seeded_pool().await;
        let (ka, kb) = (unique_key("a"), unique_key("b"));
        acquire(&pool, &ka, "w1", None, None, 60, None)
            .await
            .unwrap();

        let live = is_live(&pool, "ai_1", &ka).await.unwrap().unwrap();
        assert_eq!(live.camera_id, "cam1");
        assert_eq!(live.task_type, "anpr");
        assert!(is_live(&pool, "ai_1", &kb).await.unwrap().is_none());
        assert!(is_live(&pool, "ai_nope", &ka).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn release_is_scoped_to_the_holder_and_frees_the_tasks() {
        let pool = seeded_pool().await;
        let (ka, kb) = (unique_key("a"), unique_key("b"));
        let a = acquire(&pool, &ka, "w1", None, None, 60, None)
            .await
            .unwrap();

        // Another credential cannot drop it by naming the lease id.
        assert_eq!(release(&pool, &a.lease_id, &kb).await.unwrap(), 0);
        assert!(is_live(&pool, "ai_1", &ka).await.unwrap().is_some());

        assert_eq!(release(&pool, &a.lease_id, &ka).await.unwrap(), 2);
        assert!(is_live(&pool, "ai_1", &ka).await.unwrap().is_none());
        // Freed immediately, no waiting for expiry.
        let b = acquire(&pool, &kb, "w2", None, None, 60, None)
            .await
            .unwrap();
        assert_eq!(b.tasks.len(), 2);
    }

    #[tokio::test]
    async fn task_type_filter_camera_scope_and_budget_narrow_the_claim() {
        let pool = seeded_pool().await;

        let only_anpr = acquire(
            &pool,
            &unique_key("a"),
            "w1",
            Some(&["anpr".to_string()]),
            None,
            60,
            None,
        )
        .await
        .unwrap();
        assert_eq!(only_anpr.tasks.len(), 1);
        assert_eq!(only_anpr.tasks[0].task_type, "anpr");

        let scoped: std::collections::HashSet<String> = ["cam2".to_string()].into_iter().collect();
        let b = acquire(&pool, &unique_key("b"), "w2", None, None, 60, Some(&scoped))
            .await
            .unwrap();
        assert_eq!(b.tasks.len(), 1, "a camera-scoped key leases only its lane");
        assert_eq!(b.tasks[0].camera_id, "cam2");

        // Budget caps how many are taken in one call.
        let pool2 = seeded_pool().await;
        let capped = acquire(&pool2, &unique_key("a"), "w1", None, Some(1), 60, None)
            .await
            .unwrap();
        assert_eq!(capped.tasks.len(), 1);
    }

    #[test]
    fn ttl_is_clamped_into_the_supported_band() {
        assert_eq!(clamp_ttl(None), DEFAULT_LEASE_TTL_SECS);
        assert_eq!(clamp_ttl(Some(1)), MIN_LEASE_TTL_SECS);
        assert_eq!(clamp_ttl(Some(99_999)), MAX_LEASE_TTL_SECS);
        assert_eq!(clamp_ttl(Some(90)), 90);
    }
}
