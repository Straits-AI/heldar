//! Backup subsystem: scheduled policy jobs, on-demand archive export, and the shared transfer
//! plumbing.
//!
//! The scheduler (spawned from `main` when `HELDAR_BACKUP_ENABLED`) ticks every
//! `HELDAR_BACKUP_SCHEDULER_INTERVAL_S`, creates a `backup_job` for each due enabled policy, and runs
//! it under a process-wide concurrency [`Semaphore`] (also shared by manual triggers) with a
//! per-job timeout. A job resolves its segment files (camera selection + time window, optionally only
//! evidence-locked footage) and ships them:
//!   - `local` destinations copy via std fs into `{dest path}/{camera_id}/` (NAS mounts, no rclone).
//!   - `sftp` / `ftp` / `s3` destinations shell out to rclone (`HELDAR_RCLONE_BIN`). When rclone is not
//!     installed the job is marked `error` with a clear message — the build/tests never require it.
//!     Destination secrets (S3 keys, SFTP/FTP password) are passed to the rclone child via
//!     backend-specific ENV vars (`RCLONE_S3_SECRET_ACCESS_KEY`, `RCLONE_SFTP_PASS`, …), never as argv,
//!     so they don't leak through the world-readable `/proc/<pid>/cmdline`; the password obscure step
//!     likewise feeds the plaintext on stdin (`rclone obscure -`).
//!
//! On-demand archive export ([`create_archive`]) builds a `.zip` of the selected segments via
//! `/usr/bin/zip` into `HELDAR_ARCHIVE_DIR/{job_id}.zip` (served at `/media/archives`), enforcing
//! `HELDAR_ARCHIVE_MAX_BYTES`. It reuses `backup_jobs` with `kind='on_demand_archive'` + `output_url`.

use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::types::Json as SqlxJson;
use sqlx::SqlitePool;
use tokio::process::Command;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::auth::{Principal, PrincipalKind};
use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::models::{BackupDestination, BackupJob, BackupPolicy, BackupTestResult, Segment};
use crate::repo;
use crate::state::{AppState, CameraSelection};

/// Hardcoded archiver (the environment provides /usr/bin/zip + tar).
const ZIP_BIN: &str = "/usr/bin/zip";

/// How often a RUNNING transfer re-asks whether the credential that ordered it is still good.
///
/// A compromise, and the numbers are the argument. Per file is one indexed seek per segment — a
/// thousand-segment nightly backup would put a thousand extra reads on the same SQLite the recorder
/// is writing to. Never is a full `HELDAR_BACKUP_JOB_TIMEOUT_S` (default 3600 s) of footage leaving
/// the box after an operator revoked the key. Five seconds costs at most one read per five seconds
/// of transfer and bounds the post-revocation window to that plus the current file.
const CREATOR_RECHECK_INTERVAL: Duration = Duration::from_secs(5);

/// Who ORDERED a job, persisted on the job row so work that outlives the request can re-check them.
///
/// `kind` is stored rather than inferred from the id, so the re-check cannot be pointed at the wrong
/// table by an id that happens to look like the other kind's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobCreator {
    pub id: String,
    pub kind: &'static str,
}

impl JobCreator {
    pub const KIND_API_KEY: &'static str = "api_key";
    pub const KIND_USER: &'static str = "user";
    pub const KIND_SYSTEM: &'static str = "system";

    /// The creator record for a request's principal.
    pub fn of(p: &Principal) -> JobCreator {
        JobCreator {
            id: p.id.clone(),
            kind: match p.kind {
                PrincipalKind::ApiKey => Self::KIND_API_KEY,
                PrincipalKind::User => Self::KIND_USER,
                // Auth disabled. Recorded so the row is not indistinguishable from a scheduler job,
                // but re-checked as permanently authorized — there is no credential to withdraw.
                PrincipalKind::System => Self::KIND_SYSTEM,
            },
        }
    }
}

/// Whether the credential that ordered a job may still have it move footage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreatorStanding {
    /// Keep going: no credential ordered this job, or the one that did is still good for it.
    Authorized,
    /// Stop: the credential was withdrawn, or no longer holds every camera the job covers.
    Withdrawn(String),
}

/// Re-ask, from the database, whether `job`'s ordering credential still authorizes it.
///
/// # Why this exists
///
/// `spawn_job` detaches the transfer. The response goes out; the copy keeps running for up to
/// `HELDAR_BACKUP_JOB_TIMEOUT_S` (default 3600 s), and for `sftp`/`ftp`/`s3` destinations it is
/// moving recorded footage OFF the box over rclone, where no later `/media` guard and no later API
/// check will ever see it again. Every authorization decision about that job was made once, at
/// request time. Revoking the key afterwards is a deliberate operator act meaning "this credential
/// is compromised" — and until this check existed it did nothing at all to the bytes already in
/// flight. Narrowing `scope_cameras` is the same shape and worse for this boundary: the job goes on
/// shipping a camera the credential no longer holds.
///
/// # Why the answer is not simply "abort on anything unexpected"
///
/// A false abort destroys a backup, which is the box's durability feature, so each arm is chosen:
///
/// - **No creator** (`created_by IS NULL`) — the background scheduler holds no principal, and rows
///   predating migration 0015 cannot be attributed. Authorized. This is the arm that keeps the
///   scheduler, and every upgraded box, behaving exactly as before.
/// - **Auth disabled** (`cfg.auth_enabled == false`) — Authorized, whatever the job records. This is
///   keyed on the CONFIG, not on the principal kind, because `PrincipalKind::System` does not mean
///   "auth is disabled": `resolve_request_principal` tries the presented token first and only falls
///   back to the synthetic admin when none RESOLVES. So an auth-disabled box handed a real key
///   produces `PrincipalKind::ApiKey` and records that key as the creator. Keying the exemption on
///   `system` therefore left such a job withdrawable — and on a box where auth is off, revoking a key
///   removes no access at all (the holder simply omits the header), so aborting its backup is a pure
///   false deny for an act that means nothing there.
/// - **`system`** — the synthetic admin, i.e. no credential resolved. Nothing to re-check.
/// - **`user`** — checked against `users.active` ONLY. Not against the session: sessions end on
///   logout, idle timeout and TTL, none of which means "compromised", and aborting a running backup
///   because the operator who started it closed their laptop is a false deny with no upside.
///   Deactivating or deleting the user IS the operator act, and that one stops the job.
/// - **`api_key`** — re-resolved through [`crate::auth::api_key_principal_now`], the same decision
///   the request path makes, then its CURRENT scope is required to cover every camera in the job.
/// - **A database read failure** — Authorized, loudly. The recorder shares this SQLite and a busy
///   timeout is a normal event under write load; letting one abort a transfer would convert routine
///   contention into lost backups. Only a DEFINITE answer denies.
pub async fn creator_standing(state: &AppState, job: &BackupJob) -> CreatorStanding {
    // Auth disabled: no credential carries access here, so no credential can lose it. Checked FIRST
    // and on the config rather than on the recorded kind — see the doc above for why those differ.
    if !state.cfg.auth_enabled {
        return CreatorStanding::Authorized;
    }
    let (Some(id), Some(kind)) = (job.created_by.as_deref(), job.created_by_kind.as_deref()) else {
        return CreatorStanding::Authorized;
    };
    match kind {
        JobCreator::KIND_SYSTEM => CreatorStanding::Authorized,
        JobCreator::KIND_USER => {
            match sqlx::query_scalar::<_, bool>("SELECT active FROM users WHERE id = ?")
                .bind(id)
                .fetch_optional(&state.pool)
                .await
            {
                Ok(Some(true)) => CreatorStanding::Authorized,
                Ok(Some(false)) => CreatorStanding::Withdrawn(
                    "the user account that created this job has been deactivated".into(),
                ),
                Ok(None) => CreatorStanding::Withdrawn(
                    "the user account that created this job no longer exists".into(),
                ),
                Err(e) => {
                    tracing::warn!(
                        job = %job.id, error = %e,
                        "backup: could not re-check the creating user; continuing the transfer"
                    );
                    CreatorStanding::Authorized
                }
            }
        }
        JobCreator::KIND_API_KEY => {
            let principal = match crate::auth::api_key_principal_now(
                &state.pool,
                id,
                state.cfg.machine_auth,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(
                        job = %job.id, error = %e,
                        "backup: could not re-check the creating API key; continuing the transfer"
                    );
                    return CreatorStanding::Authorized;
                }
            };
            let Some(principal) = principal else {
                return CreatorStanding::Withdrawn(
                    "the API key that created this job has been revoked, deactivated, deleted or \
                     has expired"
                        .into(),
                );
            };
            let cameras = json_to_string_vec(&job.camera_ids.0);
            // `[]` on a job row means EVERY camera (see `resolve_segments`), so a key that has since
            // been given a camera scope can no longer authorize it — there is nothing to subset
            // against. Only a fleet-wide credential may keep a fleet-wide job running.
            if cameras.is_empty() {
                return match principal.camera_scope() {
                    None => CreatorStanding::Authorized,
                    Some(_) => CreatorStanding::Withdrawn(
                        "the API key that created this fleet-wide job has since been narrowed to a \
                         camera list"
                            .into(),
                    ),
                };
            }
            match cameras.iter().find(|c| !principal.camera_allowed(c)) {
                None => CreatorStanding::Authorized,
                Some(lost) => CreatorStanding::Withdrawn(format!(
                    "the API key that created this job is no longer scoped to camera `{lost}`"
                )),
            }
        }
        // Only `JobCreator::of` writes this column, and it writes one of three constants. A fourth
        // value means the row was edited out of band, and "we cannot tell who ordered this transfer"
        // is not a state in which to keep shipping footage off the box.
        other => CreatorStanding::Withdrawn(format!(
            "this job records an unrecognised creator kind `{other}`"
        )),
    }
}

/// Rate limiter for [`creator_standing`] inside a running transfer: answers `Authorized` without
/// touching the database until [`CREATOR_RECHECK_INTERVAL`] has elapsed since the last real check.
///
/// Interior mutability (and `&self` on [`CreatorWatch::check`]) so the per-file callback in
/// `copy_local` can share it: a closure returning a future cannot lend out a `&mut` capture, and the
/// alternative — one re-check per file — is the database cost this type exists to avoid. The lock is
/// never held across an `await`.
struct CreatorWatch {
    last: std::sync::Mutex<std::time::Instant>,
}

impl CreatorWatch {
    /// Start the clock. Callers construct this immediately AFTER a full check, so the first
    /// in-loop re-check is one interval away rather than immediate.
    fn started() -> Self {
        CreatorWatch {
            last: std::sync::Mutex::new(std::time::Instant::now()),
        }
    }

    /// A watch whose first in-loop re-check is due IMMEDIATELY.
    ///
    /// `CREATOR_RECHECK_INTERVAL` is 5s with no seam, so a fast test could never make the in-loop
    /// half of this fix fire — and an untestable guard is one that can be deleted with the suite
    /// still green, which is exactly what happened: both in-loop re-checks were removed and
    /// `cargo test --workspace` stayed at exit 0. This constructor is the seam that lets the test
    /// below prove the loop actually consults `creator_standing`, not merely that the plumbing
    /// propagates an error from an arbitrary closure.
    #[cfg(test)]
    fn due_now() -> Self {
        CreatorWatch {
            last: std::sync::Mutex::new(
                std::time::Instant::now()
                    .checked_sub(CREATOR_RECHECK_INTERVAL)
                    .unwrap_or_else(std::time::Instant::now),
            ),
        }
    }

    /// Whether a real check is due, claiming the slot if so. Poison-tolerant: a re-check that panics
    /// must not turn every later call into an abort.
    fn due(&self) -> bool {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if last.elapsed() < CREATOR_RECHECK_INTERVAL {
            return false;
        }
        *last = std::time::Instant::now();
        true
    }

    async fn check(&self, state: &AppState, job: &BackupJob) -> CreatorStanding {
        if !self.due() {
            return CreatorStanding::Authorized;
        }
        creator_standing(state, job).await
    }
}

/// Process-wide job concurrency gate, sized from config on first use and shared by the scheduler +
/// manual triggers. A `OnceLock` (not reset on scheduler respawn) keeps the bound stable for the
/// process lifetime.
fn job_semaphore(cfg: &Config) -> Arc<Semaphore> {
    static SEM: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEM.get_or_init(|| Arc::new(Semaphore::new(cfg.backup_max_concurrent_jobs.max(1))))
        .clone()
}

/// Background scheduler loop. Returns immediately (no respawn churn) when backups are disabled — but
/// `main` already guards the spawn, mirroring the notifier.
pub async fn run(state: AppState) {
    if !state.cfg.backup_enabled {
        tracing::info!("backup: scheduler disabled (HELDAR_BACKUP_ENABLED=false)");
        return;
    }
    let interval_s = state.cfg.backup_scheduler_interval_s.max(5);
    tracing::info!(
        interval_s,
        max_concurrent = state.cfg.backup_max_concurrent_jobs,
        "backup: scheduler started"
    );
    let mut tick = tokio::time::interval(Duration::from_secs(interval_s));
    loop {
        tick.tick().await;
        if let Err(e) = sweep(&state).await {
            tracing::error!(error = %e, "backup: scheduler tick failed");
        }
    }
}

/// Create + dispatch a job for every due enabled policy.
async fn sweep(state: &AppState) -> anyhow::Result<()> {
    let now = Utc::now();
    let policies: Vec<BackupPolicy> =
        sqlx::query_as::<_, BackupPolicy>("SELECT * FROM backup_policies WHERE enabled = 1")
            .fetch_all(&state.pool)
            .await?;
    for p in policies {
        let due = match p.last_run_at {
            None => true,
            Some(last) => last + chrono::Duration::seconds(p.schedule_interval_s.max(1)) <= now,
        };
        if !due {
            continue;
        }
        // The scheduler holds no principal, so the policy's stored list IS the authority here and an
        // empty one legitimately means the whole fleet. The request path must never build a
        // selection this way — see [`trigger_policy`].
        let selection = stored_selection(&p);
        // ...and it holds no credential either, so the job records no creator and the in-flight
        // re-check has nothing to withdraw. A scheduled backup must not become revocable by proxy.
        match create_policy_job(state, &p, &selection, None).await {
            Ok(job_id) => spawn_job(state.clone(), job_id),
            Err(e) => tracing::error!(policy = %p.id, error = %e, "backup: failed to create job"),
        }
    }
    Ok(())
}

/// The selection a policy's STORED `camera_ids` denotes, where empty means the whole fleet.
///
/// Only the unprincipled background sweep may build a selection this way. A request path must derive
/// it from the CALLER (`state::camera_selection`), because the stored list is attacker-influenced
/// input the moment a camera-scoped credential can reach the row.
fn stored_selection(p: &BackupPolicy) -> CameraSelection {
    let ids = json_to_string_vec(&p.camera_ids.0);
    if ids.is_empty() {
        CameraSelection::All
    } else {
        CameraSelection::Only(ids)
    }
}

/// Insert a `policy` job from a policy and claim the policy (`last_run_at`/`last_job_id`) so the next
/// tick does not re-trigger it. Returns the new job id.
///
/// The camera selection is a PARAMETER rather than a read of `p.camera_ids`, so the two callers
/// cannot be confused: the scheduler passes the stored list (fleet-wide is legitimate for it), a
/// manual trigger passes the list confined to its caller's scope. Reading the policy here is what
/// let a scoped trigger ship the whole fleet no matter what the caller was entitled to.
///
/// `creator` is `None` for the scheduler and `Some` for a request, and is a parameter for the same
/// reason: the transfer outlives the request, so the row has to carry whoever can still be withdrawn.
async fn create_policy_job(
    state: &AppState,
    p: &BackupPolicy,
    selection: &CameraSelection,
    creator: Option<&JobCreator>,
) -> anyhow::Result<String> {
    let now = Utc::now();
    let job_id = format!("bkj_{}", Uuid::new_v4().simple());
    let camera_ids = match selection.ids() {
        // `[]` is how a job row spells "every camera" to `resolve_segments`, so a selection of
        // NOTHING has no encoding here and must not be stored as one — it would invert into the
        // widest job the box can run. Only a caller that confined a selection down to nothing gets
        // here, and it has nothing it may ship.
        Some([]) => anyhow::bail!("backup selection names no cameras"),
        Some(ids) => json_from_strs(ids),
        None => json_from_strs(&[]),
    };
    let from_time = if p.lookback_hours > 0 {
        Some(now - chrono::Duration::hours(p.lookback_hours))
    } else {
        None
    };
    let to_time = Some(now);
    sqlx::query(
        "INSERT INTO backup_jobs
           (id, policy_id, destination_id, kind, camera_ids, from_time, to_time,
            incident_lock_only, status, created_at, created_by, created_by_kind)
         VALUES (?, ?, ?, 'policy', ?, ?, ?, ?, 'pending', ?, ?, ?)",
    )
    .bind(&job_id)
    .bind(&p.id)
    .bind(&p.destination_id)
    .bind(SqlxJson(camera_ids))
    .bind(from_time)
    .bind(to_time)
    .bind(p.incident_lock_only)
    .bind(now)
    .bind(creator.map(|c| c.id.clone()))
    .bind(creator.map(|c| c.kind))
    .execute(&state.pool)
    .await?;
    sqlx::query(
        "UPDATE backup_policies SET last_run_at = ?, last_job_id = ?, updated_at = ? WHERE id = ?",
    )
    .bind(now)
    .bind(&job_id)
    .bind(now)
    .bind(&p.id)
    .execute(&state.pool)
    .await?;
    Ok(job_id)
}

/// Manually trigger a policy: create its job and dispatch it (returns the job id immediately).
///
/// `selection` is the caller's CONFINED camera list, not the policy's stored one. Triggering is the
/// step that actually moves the bytes, so it is the step that must be scoped: a policy may have been
/// written fleet-wide by an unscoped admin long before a camera-scoped credential pressed the button.
///
/// `creator` is the pressing credential, recorded on the job because the transfer keeps running
/// after this function returns — see [`creator_standing`].
pub async fn trigger_policy(
    state: &AppState,
    policy: &BackupPolicy,
    selection: &CameraSelection,
    creator: &JobCreator,
) -> anyhow::Result<String> {
    let job_id = create_policy_job(state, policy, selection, Some(creator)).await?;
    spawn_job(state.clone(), job_id.clone());
    Ok(job_id)
}

/// Spawn a detached task that acquires a concurrency permit then executes the job under the timeout.
///
/// Detached deliberately: a nightly NAS sync of hours of footage cannot be held open on an HTTP
/// request. The cost of detaching is that the authorization behind the job is now older than the
/// work, which is why [`execute_job`] re-checks it — before the first byte and periodically after.
/// Queuing behind the semaphore makes that gap wider still: a job can sit here for the whole of
/// another job's timeout before its own first byte moves.
fn spawn_job(state: AppState, job_id: String) {
    let sem = job_semaphore(&state.cfg);
    let timeout = Duration::from_secs(state.cfg.backup_job_timeout_s.max(30));
    tokio::spawn(async move {
        let _permit = match sem.acquire_owned().await {
            Ok(p) => p,
            Err(_) => return,
        };
        execute_job(&state, &job_id, timeout).await;
    });
}

/// Execute a destination-backed job: resolve its segments, copy them, and record progress + outcome.
async fn execute_job(state: &AppState, job_id: &str, timeout: Duration) {
    let Some(job) = sqlx::query_as::<_, BackupJob>("SELECT * FROM backup_jobs WHERE id = ?")
        .bind(job_id)
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten()
    else {
        return;
    };

    let dest = match &job.destination_id {
        Some(d) => {
            sqlx::query_as::<_, BackupDestination>("SELECT * FROM backup_destinations WHERE id = ?")
                .bind(d)
                .fetch_optional(&state.pool)
                .await
                .ok()
                .flatten()
        }
        None => None,
    };
    let Some(dest) = dest else {
        set_job_error(state, job_id, "backup destination not found or removed").await;
        return;
    };
    if !dest.enabled {
        set_job_error(state, job_id, "backup destination is disabled").await;
        return;
    }

    let camera_ids = json_to_string_vec(&job.camera_ids.0);
    let segments = match resolve_segments(
        &state.pool,
        &camera_ids,
        job.from_time,
        job.to_time,
        job.incident_lock_only,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            set_job_error(state, job_id, &format!("resolving segments: {e}")).await;
            return;
        }
    };

    let files_total = segments.len() as i64;
    let _ = sqlx::query(
        "UPDATE backup_jobs SET status = 'running', files_total = ?, started_at = ? WHERE id = ?",
    )
    .bind(files_total)
    .bind(Utc::now())
    .bind(job_id)
    .execute(&state.pool)
    .await;

    if segments.is_empty() {
        let _ = sqlx::query(
            "UPDATE backup_jobs SET status = 'completed', finished_at = ? WHERE id = ?",
        )
        .bind(Utc::now())
        .bind(job_id)
        .execute(&state.pool)
        .await;
        return;
    }

    // THE POINT OF NO RETURN. Everything above is local bookkeeping; below this line bytes start
    // leaving, and for an sftp/ftp/s3 destination they leave the box for good. The job may have been
    // sitting on the semaphore for the length of another job's timeout since anyone checked its
    // credential, so ask again before the first copy rather than trusting the request that queued it.
    if let CreatorStanding::Withdrawn(why) = creator_standing(state, &job).await {
        tracing::warn!(
            target: "heldar::security",
            job = job_id, reason = %why,
            "backup: refusing to start a transfer whose ordering credential no longer authorizes it"
        );
        set_job_error(state, job_id, &format!("backup aborted: {why}")).await;
        return;
    }

    // Read-lock the source segments so retention cannot prune them mid-transfer; always released
    // after the (possibly timed-out) transfer future settles.
    let seg_ids: Vec<String> = segments.iter().map(|s| s.id.clone()).collect();
    let _read_lock = repo::SegReadLock::acquire(&state.pool, seg_ids).await;
    let outcome = tokio::time::timeout(
        timeout,
        copy_segments(state, &job, &dest, &segments, &CreatorWatch::started()),
    )
    .await;

    match outcome {
        Err(_) => set_job_error(state, job_id, "backup job timed out").await,
        Ok(Err(e)) => set_job_error(state, job_id, &e.to_string()).await,
        Ok(Ok((copied, bytes))) => {
            let _ = sqlx::query(
                "UPDATE backup_jobs SET status = 'completed', files_copied = ?, bytes_copied = ?, finished_at = ? WHERE id = ?",
            )
            .bind(copied as i64)
            .bind(bytes as i64)
            .bind(Utc::now())
            .bind(job_id)
            .execute(&state.pool)
            .await;
            tracing::info!(job = job_id, files = copied, bytes, "backup: job completed");
        }
    }
}

async fn set_job_error(state: &AppState, job_id: &str, msg: &str) {
    tracing::warn!(job = job_id, error = msg, "backup: job failed");
    let _ = sqlx::query(
        "UPDATE backup_jobs SET status = 'error', error = ?, finished_at = ? WHERE id = ?",
    )
    .bind(msg)
    .bind(Utc::now())
    .bind(job_id)
    .execute(&state.pool)
    .await;
}

/// Dispatch the transfer by destination kind. Returns (files_copied, bytes_copied).
async fn copy_segments(
    state: &AppState,
    job: &BackupJob,
    dest: &BackupDestination,
    segments: &[Segment],
    watch: &CreatorWatch,
) -> anyhow::Result<(u64, u64)> {
    match dest.kind.as_str() {
        "local" => copy_local(state, job, dest, segments, watch).await,
        "sftp" | "ftp" | "s3" => copy_rclone(state, job, dest, segments, watch).await,
        other => anyhow::bail!("unknown backup destination kind `{other}`"),
    }
}

/// Local / NAS-mount destination: std fs copy into `{path}/{camera_id}/{file}`.
async fn copy_local(
    state: &AppState,
    job: &BackupJob,
    dest: &BackupDestination,
    segments: &[Segment],
    watch: &CreatorWatch,
) -> anyhow::Result<(u64, u64)> {
    let base = cfg_str(&dest.config.0, "path");
    if base.is_empty() {
        anyhow::bail!("local destination has no `path` configured");
    }
    copy_segments_to_dir(Path::new(&base), segments, |copied, bytes| async move {
        update_progress(state, &job.id, copied, bytes).await;
        // Between batches: a job long enough to matter is long enough for an operator to revoke the
        // key halfway through, and every remaining file would otherwise still be shipped.
        match watch.check(state, job).await {
            CreatorStanding::Authorized => Ok(()),
            CreatorStanding::Withdrawn(why) => Err(anyhow::anyhow!("backup aborted: {why}")),
        }
    })
    .await
}

/// Copy each segment file to `{base}/{camera_id}/{basename}`, returning `(files_copied, bytes_copied)`.
/// A source that vanished between selection and copy is skipped (not an error) — retention or a
/// concurrent delete can race the backup. `on_file(copied, bytes)` is awaited after each file so the
/// caller can persist progress AND decide whether to keep going: returning `Err` stops the transfer
/// there, leaving the files already copied in place (they are backups, not spoils — deleting them
/// would be the destructive answer to a revocation). The fs logic is separated from those side
/// effects so it stays testable without an `AppState`/DB.
async fn copy_segments_to_dir<F, Fut>(
    base: &Path,
    segments: &[Segment],
    mut on_file: F,
) -> anyhow::Result<(u64, u64)>
where
    F: FnMut(u64, u64) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let mut copied = 0u64;
    let mut bytes = 0u64;
    for seg in segments {
        let cam_dir = base.join(&seg.camera_id);
        tokio::fs::create_dir_all(&cam_dir)
            .await
            .map_err(|e| anyhow::anyhow!("creating {}: {e}", cam_dir.display()))?;
        let target = cam_dir.join(file_name_of(&seg.path));
        match tokio::fs::copy(&seg.path, &target).await {
            Ok(n) => {
                copied += 1;
                bytes += n;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::warn!(path = %seg.path, "backup: source segment vanished; skipping");
            }
            Err(e) => anyhow::bail!("copying {}: {e}", seg.path),
        }
        on_file(copied, bytes).await?;
    }
    Ok((copied, bytes))
}

/// Remote destination (sftp/ftp/s3) via rclone. Degrades to a clear error when rclone is missing.
async fn copy_rclone(
    state: &AppState,
    job: &BackupJob,
    dest: &BackupDestination,
    segments: &[Segment],
    watch: &CreatorWatch,
) -> anyhow::Result<(u64, u64)> {
    let bin = &state.cfg.rclone_bin;
    if !binary_available(bin).await {
        anyhow::bail!(
            "rclone binary `{bin}` is not available; install rclone or set HELDAR_RCLONE_BIN \
             (remote sftp/ftp/s3 backup requires it; local/NAS destinations do not)"
        );
    }
    let (remote, base, secrets, env) = build_remote(bin, &dest.kind, &dest.config.0).await?;
    let mut copied = 0u64;
    let mut bytes = 0u64;
    for seg in segments {
        let rel = join_path(&base, &[&seg.camera_id, &file_name_of(&seg.path)]);
        let target = format!("{remote}{rel}");
        let out = Command::new(bin)
            .arg("copyto")
            .arg(&seg.path)
            .arg(&target)
            .arg("--no-traverse")
            // Credentials are supplied per-child via env (not argv) so they never hit /proc/<pid>/cmdline.
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|e| anyhow::anyhow!("spawning rclone: {e}"))?;
        if out.status.success() {
            copied += 1;
            bytes += seg.size_bytes.max(0) as u64;
        } else {
            let err = scrub(&String::from_utf8_lossy(&out.stderr), &secrets);
            anyhow::bail!(
                "rclone copy failed for {}: {}",
                file_name_of(&seg.path),
                err.trim()
            );
        }
        update_progress(state, &job.id, copied, bytes).await;
        // The remote destinations are the ones this matters most for: these bytes have left the box
        // and no later guard can reach them, so every file past a revocation is unrecoverable.
        if let CreatorStanding::Withdrawn(why) = watch.check(state, job).await {
            anyhow::bail!("backup aborted: {why}");
        }
    }
    Ok((copied, bytes))
}

async fn update_progress(state: &AppState, job_id: &str, copied: u64, bytes: u64) {
    let _ = sqlx::query("UPDATE backup_jobs SET files_copied = ?, bytes_copied = ? WHERE id = ?")
        .bind(copied as i64)
        .bind(bytes as i64)
        .bind(job_id)
        .execute(&state.pool)
        .await;
}

// ---- On-demand archive export ----

/// Sum of regular-file sizes directly under `dir` (non-recursive — exports are flat `.zip` files).
/// Best-effort: an unreadable directory or entry contributes 0 rather than failing the export.
async fn dir_size_bytes(dir: &std::path::Path) -> u64 {
    let mut total = 0u64;
    if let Ok(mut rd) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            if let Ok(md) = entry.metadata().await {
                if md.is_file() {
                    total = total.saturating_add(md.len());
                }
            }
        }
    }
    total
}

/// Build a `.zip` of the selected segments and record it as an `on_demand_archive` job. Enforces the
/// archive size cap on the source footprint; runs inline (bounded by the job timeout) so the returned
/// job already carries `output_url`.
///
/// # Why this one is NOT re-checked mid-flight, unlike a policy transfer
///
/// It never outlives its request. There is no `spawn_job` here: the caller awaits this whole
/// function, so if the client goes away the future is dropped and the export stops with it. And the
/// product of the export stays ON the box, at `archive_dir/{job}.zip`, reachable only through
/// `/media/archives/...` — which `media_scope::guard` re-authorizes on EVERY fetch against the
/// caller's current scope. A key revoked one second after the 201 cannot download the zip it just
/// made. That is the whole difference from a destination-backed job, where the bytes go somewhere
/// no later check of ours can follow.
///
/// `creator` is still recorded, so the ledger says who ordered every row in `backup_jobs` rather
/// than only the policy ones.
pub async fn create_archive(
    state: &AppState,
    camera_ids: Vec<String>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    incident_lock_only: bool,
    trim: bool,
    creator: &JobCreator,
) -> AppResult<BackupJob> {
    if trim && (from.is_none() || to.is_none()) {
        return Err(AppError::BadRequest(
            "`trim` requires both `from` and `to`".into(),
        ));
    }
    let segments = resolve_segments(&state.pool, &camera_ids, from, to, incident_lock_only).await?;
    if segments.is_empty() {
        return Err(AppError::NotFound(
            "no recorded footage matches the requested archive selection".into(),
        ));
    }
    let total_bytes: i64 = segments.iter().map(|s| s.size_bytes.max(0)).sum();
    if total_bytes as u64 > state.cfg.archive_max_bytes {
        return Err(AppError::BadRequest(format!(
            "archive selection is {total_bytes} bytes; exceeds the limit of {} bytes (HELDAR_ARCHIVE_MAX_BYTES)",
            state.cfg.archive_max_bytes
        )));
    }

    // Bound on-demand exports with the same semaphore as the scheduler, so a burst of export requests
    // can't run unbounded ffmpeg/zip jobs in parallel and starve recording. Binding the Arc keeps the
    // permit alive for the whole build.
    let sem = job_semaphore(&state.cfg);
    let _permit = sem
        .acquire()
        .await
        .map_err(|_| AppError::Other(anyhow::anyhow!("export queue is shutting down")))?;

    tokio::fs::create_dir_all(&state.cfg.archive_dir)
        .await
        .map_err(|e| AppError::Other(e.into()))?;

    // Free-disk precondition: the .zip lands under archive_dir on the recordings filesystem, so refuse
    // an export that could fill it (which would drive the retention sweeper into evicting recordings).
    const DISK_HEADROOM_BYTES: u64 = 1024 * 1024 * 1024; // 1 GiB margin above the footprint estimate
    if let Some(stats) =
        crate::services::storage::disk_stats_async(state.cfg.archive_dir.clone()).await
    {
        let needed = total_bytes as u64 + DISK_HEADROOM_BYTES;
        if stats.free_bytes < needed {
            return Err(AppError::BadRequest(format!(
                "not enough free disk for this export: need ~{needed} bytes, {} free",
                stats.free_bytes
            )));
        }
    }

    // Cumulative archive-directory cap: keep accumulated exports bounded so they never fill the disk.
    let archive_used = dir_size_bytes(&state.cfg.archive_dir).await;
    if archive_used.saturating_add(total_bytes as u64) > state.cfg.archive_dir_max_bytes {
        return Err(AppError::BadRequest(format!(
            "archive directory is at {archive_used} bytes; this export would exceed the {} byte cap \
             (HELDAR_ARCHIVE_DIR_MAX_BYTES) — delete old exports first",
            state.cfg.archive_dir_max_bytes
        )));
    }

    let job_id = format!("bkj_{}", Uuid::new_v4().simple());
    let now = Utc::now();
    let files_total = segments.len() as i64;
    sqlx::query(
        "INSERT INTO backup_jobs
           (id, policy_id, destination_id, kind, camera_ids, from_time, to_time,
            incident_lock_only, status, files_total, started_at, created_at,
            created_by, created_by_kind)
         VALUES (?, NULL, NULL, 'on_demand_archive', ?, ?, ?, ?, 'running', ?, ?, ?, ?, ?)",
    )
    .bind(&job_id)
    .bind(SqlxJson(json_from_strs(&camera_ids)))
    .bind(from)
    .bind(to)
    .bind(incident_lock_only)
    .bind(files_total)
    .bind(now)
    .bind(now)
    .bind(&creator.id)
    .bind(creator.kind)
    .execute(&state.pool)
    .await?;

    // Read-lock the sources for the duration of the zip/trim (released on every outcome).
    let seg_ids: Vec<String> = segments.iter().map(|s| s.id.clone()).collect();
    let _read_lock = repo::SegReadLock::acquire(&state.pool, seg_ids).await;
    let timeout = Duration::from_secs(state.cfg.backup_job_timeout_s.max(30));
    let outcome = tokio::time::timeout(
        timeout,
        build_archive_zip(state, &job_id, &segments, from, to, trim),
    )
    .await;

    let out_path = state.cfg.archive_dir.join(format!("{job_id}.zip"));
    match outcome {
        Err(_) => {
            let _ = tokio::fs::remove_file(&out_path).await;
            set_job_error(state, &job_id, "archive export timed out").await;
            return Err(AppError::Other(anyhow::anyhow!("archive export timed out")));
        }
        Ok(Err(e)) => {
            let _ = tokio::fs::remove_file(&out_path).await;
            set_job_error(state, &job_id, &e.to_string()).await;
            return Err(AppError::Other(e));
        }
        Ok(Ok(zip_bytes)) => {
            let url = format!("/media/archives/{job_id}.zip");
            // Attribute the .zip to the cameras whose footage is inside it. `archives/<job>.zip`
            // names no camera anywhere on disk, so without this row the media guard resolves it
            // `Unattributed` and refuses it — including to the credential that just exported it.
            // That is a FALSE DENY, not a leak: the scope layer would break the very feature it had
            // authorised, and every archive on the box would be unreadable to every scoped key.
            //
            // Owners come from the RESOLVED segments, never from the caller's `camera_ids`: a
            // fleet-wide export sends that field empty (empty = the whole box downstream), and
            // attributing an empty list writes nothing at all — landing back on `Unattributed` by a
            // different road. The segments are what actually went into the zip.
            //
            // Keyed `archives/{job_id}.zip` because that is exactly what `media_scope::artifact_key`
            // derives from the `url` above; a key that does not match is a row the guard never finds,
            // which is the same 403 with extra steps (the bug fixed for evidence snapshots last round).
            // Written only after the zip exists, so an attribution never outlives a failed export.
            let mut owners: Vec<String> = segments.iter().map(|s| s.camera_id.clone()).collect();
            owners.sort();
            owners.dedup();
            crate::services::media_scope::attribute(
                &state.pool,
                &format!("archives/{job_id}.zip"),
                &owners,
                crate::services::media_scope::KIND_ARCHIVE,
            )
            .await;
            sqlx::query(
                "UPDATE backup_jobs SET status = 'completed', files_copied = ?, bytes_copied = ?, output_path = ?, output_url = ?, finished_at = ? WHERE id = ?",
            )
            .bind(files_total)
            .bind(zip_bytes as i64)
            .bind(out_path.to_string_lossy().to_string())
            .bind(&url)
            .bind(Utc::now())
            .bind(&job_id)
            .execute(&state.pool)
            .await?;
        }
    }

    let job = sqlx::query_as::<_, BackupJob>("SELECT * FROM backup_jobs WHERE id = ?")
        .bind(&job_id)
        .fetch_one(&state.pool)
        .await?;
    Ok(job)
}

/// Stage the selected segments under a temp dir (symlinks, or ffmpeg-trimmed copies) then zip them.
/// Returns the produced zip's size in bytes. The staging dir is always removed.
async fn build_archive_zip(
    state: &AppState,
    job_id: &str,
    segments: &[Segment],
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    trim: bool,
) -> anyhow::Result<u64> {
    let staging = state.cfg.archive_dir.join(format!("{job_id}.stage"));
    let out_path = state.cfg.archive_dir.join(format!("{job_id}.zip"));
    let _ = tokio::fs::remove_dir_all(&staging).await;
    let _ = tokio::fs::remove_file(&out_path).await;

    let inner = async {
        tokio::fs::create_dir_all(&staging).await?;
        for seg in segments {
            let cam_dir = staging.join(&seg.camera_id);
            tokio::fs::create_dir_all(&cam_dir).await?;
            let link = cam_dir.join(file_name_of(&seg.path));
            if trim {
                // from/to are guaranteed Some when trim is set (validated by the caller).
                trim_segment(state, seg, from.unwrap(), to.unwrap(), &link).await?;
            } else {
                match tokio::fs::symlink(&seg.path, &link).await {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(anyhow::anyhow!("staging {}: {e}", seg.path)),
                }
            }
        }
        // zip recursively from the staging dir; the output lives in the parent (archive_dir), so the
        // archive never tries to include itself. zip follows symlinks by default (stores content).
        let out = Command::new(ZIP_BIN)
            .current_dir(&staging)
            .arg("-r")
            .arg("-q")
            .arg(&out_path)
            .arg(".")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|e| anyhow::anyhow!("spawning zip ({ZIP_BIN}): {e}"))?;
        if !out.status.success() {
            anyhow::bail!(
                "zip failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let size = tokio::fs::metadata(&out_path)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        Ok::<u64, anyhow::Error>(size)
    }
    .await;

    let _ = tokio::fs::remove_dir_all(&staging).await;
    inner
}

/// Re-mux the [from, to] overlap of a segment into `out` (`-c copy`, keyframe-aligned like clip export).
async fn trim_segment(
    state: &AppState,
    seg: &Segment,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    out: &Path,
) -> anyhow::Result<()> {
    let win_start = from.max(seg.start_time);
    let win_end = to.min(seg.end_time);
    let ss = ((win_start - seg.start_time).num_milliseconds() as f64 / 1000.0).max(0.0);
    let dur = (win_end - win_start).num_milliseconds() as f64 / 1000.0;
    if dur <= 0.0 {
        // No meaningful overlap (resolve_segments already filters to overlapping rows, so this is a
        // rare edge); fall back to staging the whole segment.
        let _ = tokio::fs::symlink(&seg.path, out).await;
        return Ok(());
    }
    let out_status = Command::new(&state.cfg.ffmpeg_bin)
        .kill_on_drop(true)
        .args(["-hide_banner", "-loglevel", "error"])
        .args(["-ss", &format!("{ss:.3}")])
        .arg("-i")
        .arg(&seg.path)
        .args(["-t", &format!("{dur:.3}")])
        .args([
            "-c",
            "copy",
            "-avoid_negative_ts",
            "make_zero",
            "-movflags",
            "+faststart",
        ])
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("spawning ffmpeg: {e}"))?;
    if !out_status.status.success() {
        anyhow::bail!(
            "ffmpeg trim failed for {}: {}",
            file_name_of(&seg.path),
            String::from_utf8_lossy(&out_status.stderr).trim()
        );
    }
    Ok(())
}

// ---- Destination connectivity test ----

/// Probe a destination: writability for `local`, a short rclone connectivity check for remotes.
pub async fn test_destination(state: &AppState, dest: &BackupDestination) -> BackupTestResult {
    let start = std::time::Instant::now();
    let res = match dest.kind.as_str() {
        "local" => test_local(&dest.config.0).await,
        "sftp" | "ftp" | "s3" => test_rclone(state, dest).await,
        other => Err(anyhow::anyhow!("unknown destination kind `{other}`")),
    };
    let latency_ms = start.elapsed().as_millis() as i64;
    match res {
        Ok(()) => BackupTestResult {
            ok: true,
            error: None,
            latency_ms,
        },
        Err(e) => BackupTestResult {
            ok: false,
            error: Some(e.to_string()),
            latency_ms,
        },
    }
}

async fn test_local(config: &Value) -> anyhow::Result<()> {
    let base = cfg_str(config, "path");
    if base.is_empty() {
        anyhow::bail!("local destination requires `path`");
    }
    tokio::fs::create_dir_all(&base)
        .await
        .map_err(|e| anyhow::anyhow!("cannot create {base}: {e}"))?;
    let probe = Path::new(&base).join(".heldar_backup_probe");
    tokio::fs::write(&probe, b"ok")
        .await
        .map_err(|e| anyhow::anyhow!("path not writable: {e}"))?;
    let _ = tokio::fs::remove_file(&probe).await;
    Ok(())
}

async fn test_rclone(state: &AppState, dest: &BackupDestination) -> anyhow::Result<()> {
    let bin = &state.cfg.rclone_bin;
    if !binary_available(bin).await {
        anyhow::bail!(
            "rclone binary `{bin}` is not available; install rclone or set HELDAR_RCLONE_BIN \
             (remote sftp/ftp/s3 backup requires it)"
        );
    }
    let (remote, base, secrets, env) = build_remote(bin, &dest.kind, &dest.config.0).await?;
    let target = format!("{remote}{base}");
    let out = tokio::time::timeout(
        Duration::from_secs(30),
        Command::new(bin)
            .arg("lsd")
            .arg(&target)
            .args(["--max-depth", "1"])
            // Credentials per-child via env, not argv (see build_remote / copy_rclone).
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("rclone connectivity test timed out"))?
    .map_err(|e| anyhow::anyhow!("spawning rclone: {e}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "rclone could not reach destination: {}",
            scrub(&String::from_utf8_lossy(&out.stderr), &secrets).trim()
        );
    }
    Ok(())
}

// ---- shared helpers ----

/// Fetch the segments a job/archive should ship: optionally bounded by camera ids + a [from, to)
/// overlap window, optionally restricted to evidence-locked footage.
async fn resolve_segments(
    pool: &SqlitePool,
    camera_ids: &[String],
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    incident_lock_only: bool,
) -> sqlx::Result<Vec<Segment>> {
    let mut sql = String::from("SELECT * FROM segments WHERE 1 = 1");
    if !camera_ids.is_empty() {
        let placeholders = vec!["?"; camera_ids.len()].join(",");
        sql.push_str(&format!(" AND camera_id IN ({placeholders})"));
    }
    sql.push_str(" AND (? IS NULL OR start_time < ?) AND (? IS NULL OR end_time > ?)");
    if incident_lock_only {
        sql.push_str(" AND evidence_locked = 1");
    }
    sql.push_str(" ORDER BY camera_id ASC, start_time ASC");

    let mut q = sqlx::query_as::<_, Segment>(&sql);
    for id in camera_ids {
        q = q.bind(id);
    }
    q = q.bind(to).bind(to).bind(from).bind(from);
    q.fetch_all(pool).await
}

/// Whether an external binary is runnable (so missing rclone degrades to a clear error, not a panic).
async fn binary_available(bin: &str) -> bool {
    Command::new(bin)
        .arg("version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Build an rclone on-the-fly connection-string remote (no persisted config) for a destination kind.
/// Returns `(remote_prefix_ending_in_colon, base_path, secrets_to_scrub_from_logs, env)`.
///
/// Credentials are returned as `env` (RCLONE_<BACKEND>_<OPTION> pairs), NOT baked into the connection
/// string — so they never appear in the rclone child's argv (`/proc/<pid>/cmdline`, `ps`, world-
/// readable). The caller sets `env` on the specific copyto/lsd child via `Command::env`. rclone's
/// option precedence fills the omitted secret from the backend env var (`RCLONE_S3_SECRET_ACCESS_KEY`,
/// `RCLONE_SFTP_PASS`, …); the option is simply absent from the string, so there is no ambiguity.
async fn build_remote(
    bin: &str,
    kind: &str,
    config: &Value,
) -> anyhow::Result<(String, String, Vec<String>, Vec<(String, String)>)> {
    let mut secrets: Vec<String> = Vec::new();
    let mut env: Vec<(String, String)> = Vec::new();
    match kind {
        "sftp" | "ftp" => {
            let host = cfg_str(config, "host");
            if host.is_empty() {
                anyhow::bail!("{kind} destination requires `host`");
            }
            let user = cfg_str(config, "user");
            let pass = cfg_secret(config, "pass");
            let port = config
                .get("port")
                .and_then(|p| p.as_i64())
                .map(|p| p.to_string())
                .unwrap_or_default();
            let mut parts = vec![format!(":{kind}"), format!("host={host}")];
            if !port.is_empty() {
                parts.push(format!("port={port}"));
            }
            if !user.is_empty() {
                parts.push(format!("user={user}"));
            }
            if !pass.is_empty() {
                // rclone wants the password obscured; pass it via env (RCLONE_SFTP_PASS/RCLONE_FTP_PASS)
                // rather than in the connection string so it isn't in argv.
                let obscured = rclone_obscure(bin, &pass).await?;
                secrets.push(obscured.clone());
                secrets.push(pass.clone());
                env.push((format!("RCLONE_{}_PASS", kind.to_uppercase()), obscured));
            }
            Ok((
                format!("{}:", parts.join(",")),
                cfg_str(config, "path"),
                secrets,
                env,
            ))
        }
        "s3" => {
            let bucket = cfg_str(config, "bucket");
            if bucket.is_empty() {
                anyhow::bail!("s3 destination requires `bucket`");
            }
            let access_key = cfg_str(config, "access_key");
            let secret_key = cfg_secret(config, "secret_key");
            let endpoint = cfg_str(config, "endpoint");
            let region = cfg_str(config, "region");
            let mut parts = vec![":s3".to_string(), "provider=Other".to_string()];
            // Both keys travel via env, not the connection string (the access key id is also sensitive).
            if !access_key.is_empty() {
                env.push(("RCLONE_S3_ACCESS_KEY_ID".to_string(), access_key.clone()));
                secrets.push(access_key.clone());
            }
            if !secret_key.is_empty() {
                env.push((
                    "RCLONE_S3_SECRET_ACCESS_KEY".to_string(),
                    secret_key.clone(),
                ));
                secrets.push(secret_key.clone());
            }
            if !endpoint.is_empty() {
                parts.push(format!("endpoint={endpoint}"));
            }
            if !region.is_empty() {
                parts.push(format!("region={region}"));
            }
            let base = join_path("", &[&bucket, &cfg_str(config, "prefix")]);
            Ok((format!("{}:", parts.join(",")), base, secrets, env))
        }
        other => anyhow::bail!("kind `{other}` does not use rclone"),
    }
}

/// Obscure a plaintext password into rclone's at-rest form (only invoked when rclone is present). The
/// plaintext is fed on STDIN (`rclone obscure -`), never as an argv arg — otherwise it would leak via
/// the world-readable `/proc/<pid>/cmdline` for the lifetime of this short-lived child.
async fn rclone_obscure(bin: &str, pass: &str) -> anyhow::Result<String> {
    use tokio::io::AsyncWriteExt;
    let mut child = Command::new(bin)
        .arg("obscure")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| anyhow::anyhow!("spawning rclone obscure: {e}"))?;
    // `rclone obscure -` reads the first line of stdin as the password.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(pass.as_bytes())
            .await
            .map_err(|e| anyhow::anyhow!("writing password to rclone obscure: {e}"))?;
        drop(stdin); // close stdin so rclone sees EOF
    }
    let out = child
        .wait_with_output()
        .await
        .map_err(|e| anyhow::anyhow!("running rclone obscure: {e}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "rclone obscure failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Join a base path with extra segments using `/`, preserving a leading slash on `base` (absolute
/// remote paths) but never producing a double slash.
fn join_path(base: &str, parts: &[&str]) -> String {
    let mut out = base.trim_end_matches('/').to_string();
    for p in parts {
        let p = p.trim_matches('/');
        if p.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(p);
    }
    out
}

/// Replace any known secret substrings in a log/error string with `***`.
fn scrub(s: &str, secrets: &[String]) -> String {
    let mut out = s.to_string();
    for sec in secrets {
        if !sec.is_empty() {
            out = out.replace(sec.as_str(), "***");
        }
    }
    out
}

/// Read a SECRET config value, unsealing it.
///
/// Credentials are sealed at rest, so reading them with [`cfg_str`] would hand rclone the ciphertext
/// and every transfer would fail to authenticate with a confusing upstream error. An unsealable value
/// (wrong or absent HELDAR_SECRET_KEY) returns empty rather than ciphertext: the destination then
/// fails its own "requires `pass`" validation with a message an operator can act on, and ciphertext is
/// never passed to a subprocess or written into a connection string.
fn cfg_secret(config: &Value, key: &str) -> String {
    let stored = cfg_str(config, key);
    if stored.is_empty() {
        return stored;
    }
    match crate::services::secrets::decrypt_stored(&stored) {
        Ok(plain) => plain,
        Err(e) => {
            tracing::error!(
                key,
                error = %e,
                "backup: could not decrypt a destination credential; treating it as unset"
            );
            String::new()
        }
    }
}

fn cfg_str(config: &Value, key: &str) -> String {
    config
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn file_name_of(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("segment.mp4")
        .to_string()
}

fn json_to_string_vec(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn json_from_strs(v: &[String]) -> Value {
    Value::Array(v.iter().map(|s| Value::String(s.clone())).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn test_state() -> AppState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let cfg = Arc::new(Config::from_env());
        AppState {
            recorder: crate::services::recorder::RecorderManager::new(pool.clone(), cfg.clone()),
            sampler: crate::services::sampler::SamplerManager::new(pool.clone(), cfg.clone()),
            live: crate::services::live_publisher::LivePublisherManager::new(
                pool.clone(),
                cfg.clone(),
                reqwest::Client::new(),
            ),
            mirror: None,
            consumers: Arc::new(Vec::new()),
            modules: Arc::new(Vec::new()),
            catalog: Arc::new(crate::services::registry::CatalogService::new(&cfg)),
            http: reqwest::Client::new(),
            media_jobs: crate::services::media_jobs::MediaJobGovernor::new(2),
            started_at: Utc::now(),
            pool,
            cfg,
        }
    }

    /// A fleet-wide policy: `camera_ids = []`, which `resolve_segments` reads as EVERY camera.
    async fn seed_fleet_policy(state: &AppState) -> BackupPolicy {
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO backup_destinations (id, name, kind, config, enabled, created_at, updated_at)
             VALUES ('bkd_1', 'nas', 'local', '{}', 1, ?, ?)",
        )
        .bind(now)
        .bind(now)
        .execute(&state.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO backup_policies
               (id, name, destination_id, camera_ids, incident_lock_only, schedule_interval_s,
                lookback_hours, enabled, created_at, updated_at)
             VALUES ('bkp_fleet', 'fleet', 'bkd_1', '[]', 0, 86400, 0, 1, ?, ?)",
        )
        .bind(now)
        .bind(now)
        .execute(&state.pool)
        .await
        .unwrap();
        sqlx::query_as::<_, BackupPolicy>("SELECT * FROM backup_policies WHERE id = 'bkp_fleet'")
            .fetch_one(&state.pool)
            .await
            .unwrap()
    }

    /// The job must ship the selection it was HANDED, not the one stored on the policy.
    ///
    /// This is the shape of the F1 escape: the route confined the policy's camera list, dropped the
    /// result, and passed the stored policy downstream — so a camera-scoped credential triggering a
    /// fleet-wide (`[]`) policy ran a backup of every camera on the box. Reading `p.camera_ids` here
    /// is what made that possible, so the selection is a parameter and this test pins it.
    #[tokio::test]
    async fn a_policy_job_ships_the_selection_it_was_given_not_the_stored_one() {
        let state = test_state().await;
        let policy = seed_fleet_policy(&state).await;

        let selection = CameraSelection::Only(vec!["cam_a".to_string()]);
        let job_id = create_policy_job(&state, &policy, &selection, None)
            .await
            .unwrap();
        let stored: String = sqlx::query_scalar("SELECT camera_ids FROM backup_jobs WHERE id = ?")
            .bind(&job_id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(
            stored, r#"["cam_a"]"#,
            "the job widened back to the policy's fleet-wide selection"
        );

        // The scheduler path is unchanged: no principal, so the stored list is the authority and an
        // empty one still means the whole fleet.
        let job_id = create_policy_job(&state, &policy, &stored_selection(&policy), None)
            .await
            .unwrap();
        let stored: String = sqlx::query_scalar("SELECT camera_ids FROM backup_jobs WHERE id = ?")
            .bind(&job_id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(stored, "[]");
    }

    /// `Only([])` selects NOTHING but a job row spells "no cameras" as `[]`, which means EVERY
    /// camera. There is no encoding for it, so it must be refused rather than inverted.
    #[tokio::test]
    async fn an_empty_selection_is_refused_rather_than_stored_as_the_whole_fleet() {
        let state = test_state().await;
        let policy = seed_fleet_policy(&state).await;
        let err = create_policy_job(&state, &policy, &CameraSelection::Only(Vec::new()), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("names no cameras"), "{err}");
        let jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backup_jobs")
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert_eq!(jobs, 0, "a selection of nothing created a job anyway");
    }

    #[test]
    fn stored_selection_reads_empty_as_the_whole_fleet() {
        let mut p = BackupPolicy {
            id: "bkp_1".into(),
            name: "p".into(),
            destination_id: "bkd_1".into(),
            camera_ids: SqlxJson(json!([])),
            incident_lock_only: false,
            schedule_interval_s: 86_400,
            lookback_hours: 0,
            last_run_at: None,
            last_job_id: None,
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        assert_eq!(stored_selection(&p), CameraSelection::All);
        p.camera_ids = SqlxJson(json!(["cam_a"]));
        assert_eq!(
            stored_selection(&p),
            CameraSelection::Only(vec!["cam_a".to_string()])
        );
    }

    #[test]
    fn join_path_preserves_leading_slash() {
        assert_eq!(
            join_path("/srv/backups", &["cam1", "f.mp4"]),
            "/srv/backups/cam1/f.mp4"
        );
        assert_eq!(
            join_path("backups/", &["cam1", "f.mp4"]),
            "backups/cam1/f.mp4"
        );
        assert_eq!(join_path("", &["cam1", "f.mp4"]), "cam1/f.mp4");
        assert_eq!(join_path("bucket", &["", "p"]), "bucket/p");
    }

    #[test]
    fn scrub_masks_secrets() {
        let s = "auth failed for pass=hunter2 token=hunter2";
        assert_eq!(
            scrub(s, &["hunter2".into()]),
            "auth failed for pass=*** token=***"
        );
        assert_eq!(scrub("nothing", &["".into()]), "nothing");
    }

    #[test]
    fn json_string_vec_roundtrip() {
        let v = json!(["a", "b", 3, "c"]);
        assert_eq!(json_to_string_vec(&v), vec!["a", "b", "c"]);
        assert_eq!(json_to_string_vec(&json!("nope")), Vec::<String>::new());
        assert_eq!(json_from_strs(&["x".into(), "y".into()]), json!(["x", "y"]));
    }

    #[test]
    fn cfg_str_reads_and_trims() {
        let c = json!({ "host": "  example.com ", "port": 22 });
        assert_eq!(cfg_str(&c, "host"), "example.com");
        assert_eq!(cfg_str(&c, "missing"), "");
        // non-string fields read as empty
        assert_eq!(cfg_str(&c, "port"), "");
    }

    #[test]
    fn file_name_of_extracts_basename() {
        assert_eq!(
            file_name_of("/data/recordings/cam1/20260613_120000.mp4"),
            "20260613_120000.mp4"
        );
        assert_eq!(file_name_of(""), "segment.mp4");
    }

    // ---- backup copy execution (the real fs round-trip, minus the AppState/DB progress side effect) --

    fn seg(camera_id: &str, path: &str) -> Segment {
        Segment {
            id: format!("seg_{camera_id}"),
            camera_id: camera_id.into(),
            path: path.into(),
            start_time: chrono::Utc::now(),
            end_time: chrono::Utc::now(),
            duration_s: 60.0,
            codec: None,
            width: None,
            height: None,
            size_bytes: 0,
            container: "mp4".into(),
            locked: false,
            evidence_locked: false,
            incident_id: None,
            created_at: chrono::Utc::now(),
        }
    }

    /// A unique temp dir for one test, removed on drop.
    struct TmpDir(std::path::PathBuf);
    impl TmpDir {
        fn new(tag: &str) -> Self {
            let p =
                std::env::temp_dir().join(format!("heldar-backup-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            TmpDir(p)
        }
    }
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn copy_segments_to_dir_copies_files_byte_identical() {
        let src = TmpDir::new("src-rt");
        let dst = TmpDir::new("dst-rt");
        // Two source segments across two cameras, with distinct contents.
        let p1 = src.0.join("a.mp4");
        let p2 = src.0.join("b.mp4");
        std::fs::write(&p1, b"hello-cam-a").unwrap();
        std::fs::write(&p2, b"world-cam-b-XYZ").unwrap();
        let segs = vec![
            seg("camA", p1.to_str().unwrap()),
            seg("camB", p2.to_str().unwrap()),
        ];

        let mut progress: Vec<(u64, u64)> = Vec::new();
        let (copied, bytes) = copy_segments_to_dir(&dst.0, &segs, |c, b| {
            progress.push((c, b));
            std::future::ready(Ok(()))
        })
        .await
        .unwrap();

        assert_eq!(copied, 2);
        assert_eq!(
            bytes,
            (b"hello-cam-a".len() + b"world-cam-b-XYZ".len()) as u64
        );
        // Files land at {dest}/{camera_id}/{basename}, byte-identical to the source.
        assert_eq!(
            std::fs::read(dst.0.join("camA/a.mp4")).unwrap(),
            b"hello-cam-a"
        );
        assert_eq!(
            std::fs::read(dst.0.join("camB/b.mp4")).unwrap(),
            b"world-cam-b-XYZ"
        );
        // Progress was reported after each file (cumulative).
        assert_eq!(progress, vec![(1, 11), (2, 26)]);
    }

    #[tokio::test]
    async fn copy_segments_to_dir_skips_vanished_source_without_erroring() {
        let src = TmpDir::new("src-skip");
        let dst = TmpDir::new("dst-skip");
        let good = src.0.join("good.mp4");
        std::fs::write(&good, b"present").unwrap();
        // Second segment points at a file that was deleted (raced by retention) — must be SKIPPED.
        let segs = vec![
            seg("camA", good.to_str().unwrap()),
            seg("camA", src.0.join("gone.mp4").to_str().unwrap()),
        ];
        let (copied, bytes) =
            copy_segments_to_dir(&dst.0, &segs, |_, _| std::future::ready(Ok(())))
                .await
                .unwrap();
        assert_eq!(copied, 1, "the vanished source is skipped, not counted");
        assert_eq!(bytes, b"present".len() as u64);
        assert!(dst.0.join("camA/good.mp4").exists());
        assert!(!dst.0.join("camA/gone.mp4").exists());
    }

    #[tokio::test]
    async fn dir_size_bytes_sums_flat_files_and_ignores_subdirs() {
        let d = TmpDir::new("size");
        std::fs::write(d.0.join("a"), b"1234").unwrap(); // 4
        std::fs::write(d.0.join("b"), b"567").unwrap(); // 3
        std::fs::create_dir_all(d.0.join("sub")).unwrap();
        std::fs::write(d.0.join("sub/c"), b"ignored-9chars").unwrap(); // must NOT count
        assert_eq!(dir_size_bytes(&d.0).await, 7);
        // A nonexistent dir is best-effort 0, no panic.
        assert_eq!(dir_size_bytes(&d.0.join("nope")).await, 0);
    }

    // ---- authorization that outlives the request: the detached transfer's creator re-check -------
    //
    // Every credential below is minted, revoked and re-scoped through the REAL router
    // (`POST`/`PATCH /api/v1/api-keys`). Writing to `api_keys` directly would let these tests assert
    // against a grant shape the product refuses to issue, which is how a scope test has already
    // shipped vacuously in this repo once.

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::Service as _;

    /// Drive the composed API router the way a client does.
    async fn api(
        st: &AppState,
        token: &str,
        method: &str,
        path: &str,
        body: &str,
    ) -> (StatusCode, String) {
        let mut app = crate::routes::api_router().with_state(st.clone());
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header("X-API-Key", token)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = app.call(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1_048_576)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    /// The unscoped admin every mint needs. Seeded directly BECAUSE it is the bootstrap, never the
    /// subject of an assertion — the subject is always minted through the API below.
    async fn bootstrap_admin(st: &AppState) -> String {
        let token = format!("vok_{}", Uuid::new_v4().simple());
        sqlx::query(
            "INSERT INTO api_keys (id, name, key_hash, key_prefix, role, active, created_at,
                                   capabilities, scope_kind, scope_cameras, expires_at)
             VALUES (?,?,?,?,'admin',1,?,NULL,'all',NULL,NULL)",
        )
        .bind(format!("key_{}", Uuid::new_v4().simple()))
        .bind("bootstrap")
        .bind(crate::auth::token_hash(&token))
        .bind(&token[..8])
        .bind(Utc::now())
        .execute(&st.pool)
        .await
        .unwrap();
        token
    }

    const SCOPED_CAPS: [&str; 5] = [
        "registry:manage",
        "camera:read",
        "video:playback",
        "video:export",
        "system:read",
    ];

    /// The widest grant the product will actually pair with a camera scope. Returns `(token, id)`.
    async fn mint_scoped(st: &AppState, admin: &str, cameras: &[&str]) -> (String, String) {
        let body = json!({
            "name": "backup-integrator",
            "role": "integration",
            "capabilities": SCOPED_CAPS,
            "scope_kind": "cameras",
            "scope_cameras": cameras,
            "confirm_privileged": true,
        })
        .to_string();
        let (status, resp) = api(st, admin, "POST", "/api/v1/api-keys", &body).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "the API refused to mint a credential scoped to {cameras:?}; a test built on a grant the \
             product will not issue asserts nothing: {resp}"
        );
        let v: Value = serde_json::from_str(&resp).unwrap();
        (
            v["key"].as_str().expect("plaintext key").to_string(),
            v["id"].as_str().expect("key id").to_string(),
        )
    }

    /// Soft-revoke through the real PATCH — the operator act meaning "this credential is burned".
    async fn revoke(st: &AppState, admin: &str, key_id: &str) {
        let body = json!({ "revoked_at": Utc::now() }).to_string();
        let (status, resp) = api(
            st,
            admin,
            "PATCH",
            &format!("/api/v1/api-keys/{key_id}"),
            &body,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "revoke failed: {resp}");
    }

    /// Re-scope through the real PATCH (narrowing, not revoking).
    async fn narrow(st: &AppState, admin: &str, key_id: &str, cameras: &[&str]) {
        let body = json!({
            "capabilities": SCOPED_CAPS,
            "scope_kind": "cameras",
            "scope_cameras": cameras,
            "confirm_privileged": true,
        })
        .to_string();
        let (status, resp) = api(
            st,
            admin,
            "PATCH",
            &format!("/api/v1/api-keys/{key_id}"),
            &body,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "re-scope failed: {resp}");
    }

    async fn seed_camera(st: &AppState, id: &str) {
        let now = Utc::now();
        sqlx::query("INSERT INTO cameras (id, name, created_at, updated_at) VALUES (?,?,?,?)")
            .bind(id)
            .bind(id)
            .bind(now)
            .bind(now)
            .execute(&st.pool)
            .await
            .unwrap();
    }

    /// A destination writing into `dest_dir`, and a policy covering exactly `cameras`.
    async fn seed_dest_and_policy(st: &AppState, dest_dir: &Path, cameras: &[&str]) -> String {
        let now = Utc::now();
        let dest_id = format!("bkd_{}", Uuid::new_v4().simple());
        sqlx::query(
            "INSERT INTO backup_destinations (id, name, kind, config, enabled, created_at, updated_at)
             VALUES (?, 'nas', 'local', ?, 1, ?, ?)",
        )
        .bind(&dest_id)
        .bind(SqlxJson(json!({ "path": dest_dir.to_string_lossy() })))
        .bind(now)
        .bind(now)
        .execute(&st.pool)
        .await
        .unwrap();
        let policy_id = format!("bkp_{}", Uuid::new_v4().simple());
        sqlx::query(
            "INSERT INTO backup_policies
               (id, name, destination_id, camera_ids, incident_lock_only, schedule_interval_s,
                lookback_hours, enabled, created_at, updated_at)
             VALUES (?, 'nightly', ?, ?, 0, 86400, 0, 1, ?, ?)",
        )
        .bind(&policy_id)
        .bind(&dest_id)
        .bind(SqlxJson(json!(cameras)))
        .bind(now)
        .bind(now)
        .execute(&st.pool)
        .await
        .unwrap();
        policy_id
    }

    async fn load_policy_row(st: &AppState, policy_id: &str) -> BackupPolicy {
        sqlx::query_as::<_, BackupPolicy>("SELECT * FROM backup_policies WHERE id = ?")
            .bind(policy_id)
            .fetch_one(&st.pool)
            .await
            .unwrap()
    }

    /// One recorded segment with real bytes on disk, so a transfer has something to actually move.
    async fn seed_segment(st: &AppState, src: &Path, camera_id: &str) {
        let path = src.join(format!("{camera_id}.mp4"));
        std::fs::write(&path, format!("footage-of-{camera_id}")).unwrap();
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO segments (id, camera_id, path, start_time, end_time, duration_s, container,
                                   size_bytes, locked, evidence_locked, created_at)
             VALUES (?,?,?,?,?,60.0,'mp4',?,0,0,?)",
        )
        .bind(format!("seg_{}", Uuid::new_v4().simple()))
        .bind(camera_id)
        .bind(path.to_string_lossy())
        .bind(now - chrono::Duration::minutes(10))
        .bind(now - chrono::Duration::minutes(9))
        .bind(std::fs::metadata(&path).unwrap().len() as i64)
        .bind(now)
        .execute(&st.pool)
        .await
        .unwrap();
    }

    /// Trigger a policy through the REAL route and wait for the detached job it spawns to settle.
    ///
    /// The detached run is deliberately given nothing to copy (segments are seeded afterwards), so it
    /// finishes as a no-op; waiting for it is what makes the second, directly-driven run
    /// deterministic instead of racing `spawn_job`.
    async fn trigger_and_settle(st: &AppState, token: &str, policy_id: &str) -> String {
        let (status, resp) = api(
            st,
            token,
            "POST",
            &format!("/api/v1/backup/policies/{policy_id}/trigger"),
            "{}",
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "trigger failed: {resp}");
        let job_id = serde_json::from_str::<Value>(&resp).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        for _ in 0..300 {
            let status_now: String =
                sqlx::query_scalar("SELECT status FROM backup_jobs WHERE id = ?")
                    .bind(&job_id)
                    .fetch_one(&st.pool)
                    .await
                    .unwrap();
            if status_now == "completed" || status_now == "error" {
                return job_id;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the detached job never settled");
    }

    async fn job_row(st: &AppState, job_id: &str) -> BackupJob {
        sqlx::query_as::<_, BackupJob>("SELECT * FROM backup_jobs WHERE id = ?")
            .bind(job_id)
            .fetch_one(&st.pool)
            .await
            .unwrap()
    }

    /// Files under `{dest}/{camera}/`, i.e. what actually left for the destination.
    fn copied_files(dest: &Path, camera: &str) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(dest.join(camera))
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }

    /// A state whose auth is ON — a credential can only be withdrawn where credentials exist.
    async fn auth_state() -> AppState {
        let st = test_state().await;
        let mut cfg = Config::from_env();
        cfg.auth_enabled = true;
        AppState {
            cfg: Arc::new(cfg),
            ..st
        }
    }

    /// An AUTH-DISABLED box that is nonetheless handed a real API key.
    ///
    /// This is the shape the `system` arm did not cover, and the reason the exemption is keyed on the
    /// CONFIG rather than on the recorded principal kind. `resolve_request_principal` tries the
    /// presented token FIRST and only falls back to the synthetic admin when none resolves — so a key
    /// presented to an auth-disabled box resolves normally and the job records
    /// `created_by_kind = "api_key"`, not `"system"`. Keyed on the kind, such a job stayed
    /// withdrawable, and revoking the key aborted a backup on a box where revocation removes no
    /// access whatsoever (the holder simply omits the header). A pure false deny, destroying the
    /// durability feature for an act that means nothing there.
    #[tokio::test]
    async fn an_auth_disabled_box_never_withdraws_a_job_even_from_a_revoked_key() {
        let st = test_state().await;
        assert!(
            !st.cfg.auth_enabled,
            "this test is about the auth-DISABLED path"
        );
        let src = TmpDir::new("noauth-src");
        let dst = TmpDir::new("noauth-dst");
        seed_camera(&st, "cam_a").await;
        let admin = bootstrap_admin(&st).await;
        let (scoped, key_id) = mint_scoped(&st, &admin, &["cam_a"]).await;
        let policy = seed_dest_and_policy(&st, &dst.0, &["cam_a"]).await;
        let job_id = trigger_and_settle(&st, &scoped, &policy).await;

        // The key really was recorded — i.e. this box does NOT take the `system` arm, which is the
        // whole point of the test.
        let job = job_row(&st, &job_id).await;
        assert_eq!(
            job.created_by_kind.as_deref(),
            Some(JobCreator::KIND_API_KEY),
            "an auth-disabled box that is handed a key records it; if this ever becomes `system` the \
             config-keyed exemption below is no longer what is being tested"
        );

        seed_segment(&st, &src.0, "cam_a").await;
        revoke(&st, &admin, &key_id).await;
        execute_job(&st, &job_id, Duration::from_secs(60)).await;

        let job = job_row(&st, &job_id).await;
        assert_eq!(
            job.status, "completed",
            "a revoked key aborted a backup on a box where revocation removes no access at all \
             (error was {:?})",
            job.error
        );
        assert_eq!(
            copied_files(&dst.0, "cam_a"),
            vec!["cam_a.mp4".to_string()],
            "the footage should have been backed up"
        );
    }

    /// The `user` arm: deactivating the operator stops their job; logging out never does.
    ///
    /// The distinction is the whole point. A session ends on logout, idle timeout and TTL — an admin
    /// who starts a nightly NAS sync and shuts their laptop has done nothing wrong, and killing the
    /// backup would be a false deny with no security benefit. So the check reads `users.active`
    /// (and existence) and never touches `sessions`.
    #[tokio::test]
    async fn a_users_job_survives_logout_but_not_deactivation() {
        let st = auth_state().await;
        let dst = TmpDir::new("user-arm");
        seed_camera(&st, "cam_a").await;
        let admin = bootstrap_admin(&st).await;
        let policy_id = seed_dest_and_policy(&st, &dst.0, &["cam_a"]).await;
        let policy = load_policy_row(&st, &policy_id).await;

        let body = json!({
            "username": "night-op", "password": "correct horse battery staple",
            "role": "manager", "active": true,
        })
        .to_string();
        let (status, resp) = api(&st, &admin, "POST", "/api/v1/users", &body).await;
        assert_eq!(status, StatusCode::CREATED, "{resp}");
        let user_id = serde_json::from_str::<Value>(&resp).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();

        let creator = JobCreator {
            id: user_id.clone(),
            kind: JobCreator::KIND_USER,
        };
        let job_id = create_policy_job(&st, &policy, &stored_selection(&policy), Some(&creator))
            .await
            .unwrap();
        let job = job_row(&st, &job_id).await;
        assert_eq!(job.created_by_kind.as_deref(), Some("user"));

        // No session exists for this user at all — the strongest form of "logged out". The transfer
        // must be unaffected.
        let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE user_id = ?")
            .bind(&user_id)
            .fetch_one(&st.pool)
            .await
            .unwrap();
        assert_eq!(
            sessions, 0,
            "the fixture must have no session to begin with"
        );
        assert_eq!(
            creator_standing(&st, &job).await,
            CreatorStanding::Authorized,
            "a backup was withdrawn because its operator was not currently logged in"
        );

        // Deactivation IS the operator act, and it stops the job.
        let (status, resp) = api(
            &st,
            &admin,
            "PATCH",
            &format!("/api/v1/users/{user_id}"),
            r#"{"active":false}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{resp}");
        match creator_standing(&st, &job).await {
            CreatorStanding::Withdrawn(why) => assert!(why.contains("deactivated"), "{why}"),
            other => panic!("a deactivated user's backup kept running: {other:?}"),
        }
    }

    /// The job row must remember WHO ordered it — without that, a revocation has nothing to act on.
    #[tokio::test]
    async fn a_triggered_job_records_the_credential_that_ordered_it() {
        let st = auth_state().await;
        let dst = TmpDir::new("creator-record");
        seed_camera(&st, "cam_a").await;
        let admin = bootstrap_admin(&st).await;
        let (scoped, key_id) = mint_scoped(&st, &admin, &["cam_a"]).await;
        let policy = seed_dest_and_policy(&st, &dst.0, &["cam_a"]).await;

        let job_id = trigger_and_settle(&st, &scoped, &policy).await;
        let job = job_row(&st, &job_id).await;
        assert_eq!(job.created_by.as_deref(), Some(key_id.as_str()));
        assert_eq!(job.created_by_kind.as_deref(), Some("api_key"));
    }

    /// THE HEADLINE: revoking the key stops the transfer it left running.
    ///
    /// `spawn_job` detaches, so the copy outlives the 202 by up to HELDAR_BACKUP_JOB_TIMEOUT_S
    /// (default an hour). Before this re-check, revocation — the operator saying "this credential is
    /// compromised" — did nothing at all to footage already on its way off the box.
    #[tokio::test]
    async fn a_revoked_credential_stops_the_backup_it_left_running() {
        let st = auth_state().await;
        let src = TmpDir::new("revoke-src");
        let dst = TmpDir::new("revoke-dst");
        seed_camera(&st, "cam_a").await;
        let admin = bootstrap_admin(&st).await;
        let (scoped, key_id) = mint_scoped(&st, &admin, &["cam_a"]).await;
        let policy = seed_dest_and_policy(&st, &dst.0, &["cam_a"]).await;
        let job_id = trigger_and_settle(&st, &scoped, &policy).await;

        // Now there IS footage to move, and the operator burns the key before it moves.
        seed_segment(&st, &src.0, "cam_a").await;
        revoke(&st, &admin, &key_id).await;

        execute_job(&st, &job_id, Duration::from_secs(60)).await;

        let job = job_row(&st, &job_id).await;
        assert_eq!(
            job.status, "error",
            "a revoked key's backup ran to completion"
        );
        let err = job.error.unwrap_or_default();
        assert!(
            err.contains("revoked"),
            "the job must say why it stopped, so an operator is not left guessing: {err}"
        );
        assert!(
            copied_files(&dst.0, "cam_a").is_empty(),
            "footage reached the destination after the ordering credential was revoked"
        );
    }

    /// The IN-LOOP half of the fix, which was untestable and therefore untested: both in-loop
    /// re-checks could be deleted outright and `cargo test --workspace` stayed at exit 0, because the
    /// only thing covered was `copy_segments_to_dir` propagating an `Err` from an arbitrary
    /// caller-supplied closure — never the closure production actually installs.
    ///
    /// Two segments, creator revoked after the first file lands. `CreatorWatch::due_now` makes the
    /// re-check fire on the first callback instead of five seconds in.
    #[tokio::test]
    async fn the_in_loop_recheck_stops_the_copy_between_files() {
        let st = auth_state().await;
        let src = TmpDir::new("inloop-src");
        let dst = TmpDir::new("inloop-dst");
        seed_camera(&st, "cam_a").await;
        seed_camera(&st, "cam_b").await;
        let admin = bootstrap_admin(&st).await;
        let (scoped, key_id) = mint_scoped(&st, &admin, &["cam_a", "cam_b"]).await;
        let policy = seed_dest_and_policy(&st, &dst.0, &["cam_a", "cam_b"]).await;
        let job_id = trigger_and_settle(&st, &scoped, &policy).await;
        seed_segment(&st, &src.0, "cam_a").await;
        seed_segment(&st, &src.0, "cam_b").await;

        let job = job_row(&st, &job_id).await;
        let dest = sqlx::query_as::<_, BackupDestination>(
            "SELECT * FROM backup_destinations WHERE id = ?",
        )
        .bind(
            job.destination_id
                .as_deref()
                .expect("policy job has a destination"),
        )
        .fetch_one(&st.pool)
        .await
        .expect("destination row");
        let camera_ids = crate::state::camera_ids_from_json(&job.camera_ids.0).expect("camera ids");
        let segments = resolve_segments(
            &st.pool,
            &camera_ids,
            job.from_time,
            job.to_time,
            job.incident_lock_only,
        )
        .await
        .expect("segments resolve");
        assert_eq!(segments.len(), 2, "the fixture needs two files to copy");

        // Revoke BEFORE the copy starts, with a watch that is already due: the first callback — after
        // file one has landed — must stop the transfer, leaving file two behind.
        revoke(&st, &admin, &key_id).await;
        let watch = CreatorWatch::due_now();
        let err = copy_local(&st, &job, &dest, &segments, &watch)
            .await
            .expect_err("the in-loop re-check did not stop a revoked creator's copy");
        assert!(
            format!("{err}").contains("revoked"),
            "the abort must name the reason: {err}"
        );

        let landed = copied_files(&dst.0, "cam_a").len() + copied_files(&dst.0, "cam_b").len();
        assert_eq!(
            landed, 1,
            "expected exactly the file that was already in flight to remain, got {landed} — 0 means \
             the check fired too early to prove the LOOP consults it, 2 means it never fired"
        );
    }

    /// The control that keeps the test above from being vacuous: the SAME fixture, credential intact,
    /// must copy the footage. A re-check that denies everything is not a fix, it is an outage.
    #[tokio::test]
    async fn an_intact_credential_completes_the_very_same_transfer() {
        let st = auth_state().await;
        let src = TmpDir::new("intact-src");
        let dst = TmpDir::new("intact-dst");
        seed_camera(&st, "cam_a").await;
        let admin = bootstrap_admin(&st).await;
        let (scoped, _key_id) = mint_scoped(&st, &admin, &["cam_a"]).await;
        let policy = seed_dest_and_policy(&st, &dst.0, &["cam_a"]).await;
        let job_id = trigger_and_settle(&st, &scoped, &policy).await;

        seed_segment(&st, &src.0, "cam_a").await;
        // ...and nothing is revoked.
        execute_job(&st, &job_id, Duration::from_secs(60)).await;

        let job = job_row(&st, &job_id).await;
        assert_eq!(job.status, "completed", "error was: {:?}", job.error);
        assert_eq!(copied_files(&dst.0, "cam_a"), vec!["cam_a.mp4".to_string()]);
    }

    /// Narrowing (not revoking) is the camera-scope shape of the same defect: the job keeps shipping
    /// a camera the credential no longer holds.
    #[tokio::test]
    async fn narrowing_a_scope_stops_the_job_covering_the_camera_it_lost() {
        let st = auth_state().await;
        let src = TmpDir::new("narrow-src");
        let dst = TmpDir::new("narrow-dst");
        seed_camera(&st, "cam_a").await;
        seed_camera(&st, "cam_b").await;
        let admin = bootstrap_admin(&st).await;
        let (scoped, key_id) = mint_scoped(&st, &admin, &["cam_a", "cam_b"]).await;
        let policy = seed_dest_and_policy(&st, &dst.0, &["cam_a", "cam_b"]).await;
        let job_id = trigger_and_settle(&st, &scoped, &policy).await;

        seed_segment(&st, &src.0, "cam_a").await;
        seed_segment(&st, &src.0, "cam_b").await;
        narrow(&st, &admin, &key_id, &["cam_a"]).await;

        execute_job(&st, &job_id, Duration::from_secs(60)).await;

        let job = job_row(&st, &job_id).await;
        assert_eq!(job.status, "error");
        let err = job.error.unwrap_or_default();
        assert!(
            err.contains("cam_b"),
            "the abort must name the camera that was lost: {err}"
        );
        assert!(copied_files(&dst.0, "cam_b").is_empty());
        assert!(
            copied_files(&dst.0, "cam_a").is_empty(),
            "the check runs BEFORE the first byte, so not even the still-held camera is shipped"
        );
    }

    /// FALSE-DENY CONTROLS. The scheduler holds no principal and an auth-disabled box holds no
    /// credential; neither may ever be stopped by a mechanism about withdrawing credentials.
    #[tokio::test]
    async fn the_scheduler_and_the_auth_disabled_principal_are_never_withdrawn() {
        let st = auth_state().await;
        let dst = TmpDir::new("noprincipal");
        seed_camera(&st, "cam_a").await;
        let policy_id = seed_dest_and_policy(&st, &dst.0, &["cam_a"]).await;
        let policy = load_policy_row(&st, &policy_id).await;

        // The scheduler's own path: no creator at all.
        let job_id = create_policy_job(&st, &policy, &stored_selection(&policy), None)
            .await
            .unwrap();
        let job = job_row(&st, &job_id).await;
        assert_eq!(job.created_by, None);
        assert_eq!(
            creator_standing(&st, &job).await,
            CreatorStanding::Authorized
        );

        // Auth disabled: `Principal::system_admin()` is recorded but is not a withdrawable credential.
        let system = JobCreator::of(&Principal::system_admin());
        assert_eq!(system.kind, JobCreator::KIND_SYSTEM);
        let job_id = create_policy_job(&st, &policy, &stored_selection(&policy), Some(&system))
            .await
            .unwrap();
        let job = job_row(&st, &job_id).await;
        assert_eq!(job.created_by_kind.as_deref(), Some("system"));
        assert_eq!(
            creator_standing(&st, &job).await,
            CreatorStanding::Authorized
        );
    }

    /// A key that is fleet-wide when it triggers, and camera-scoped by the time the job runs, can no
    /// longer authorize a fleet-wide (`[]` = every camera) job: there is nothing to subset against.
    #[tokio::test]
    async fn a_fleet_wide_job_is_withdrawn_once_its_key_gains_a_camera_scope() {
        let st = auth_state().await;
        let dst = TmpDir::new("fleet-narrow");
        seed_camera(&st, "cam_a").await;
        let admin = bootstrap_admin(&st).await;
        let policy_id = seed_dest_and_policy(&st, &dst.0, &[]).await;
        let policy = load_policy_row(&st, &policy_id).await;

        // Minted fleet-wide, so `stored_selection` (= All) is legitimately its to order.
        let body = json!({
            "name": "fleet", "role": "integration",
            "capabilities": ["registry:manage", "video:export"],
            "scope_kind": "all", "scope_cameras": [], "confirm_privileged": true,
        })
        .to_string();
        let (status, resp) = api(&st, &admin, "POST", "/api/v1/api-keys", &body).await;
        assert_eq!(status, StatusCode::CREATED, "{resp}");
        let key_id = serde_json::from_str::<Value>(&resp).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();

        let creator = JobCreator {
            id: key_id.clone(),
            kind: JobCreator::KIND_API_KEY,
        };
        let job_id = create_policy_job(&st, &policy, &stored_selection(&policy), Some(&creator))
            .await
            .unwrap();
        let job = job_row(&st, &job_id).await;
        assert_eq!(
            job.camera_ids.0,
            json!([]),
            "the fixture must be fleet-wide"
        );
        assert_eq!(
            creator_standing(&st, &job).await,
            CreatorStanding::Authorized
        );

        narrow(&st, &admin, &key_id, &["cam_a"]).await;
        match creator_standing(&st, &job).await {
            CreatorStanding::Withdrawn(why) => assert!(why.contains("narrowed"), "{why}"),
            other => panic!("a fleet-wide job survived its key being scoped: {other:?}"),
        }
    }

    /// The in-transfer half: a guard that denies part-way stops the copy THERE, and what already
    /// landed is left alone (they are backups, not spoils).
    #[tokio::test]
    async fn a_denial_between_files_stops_the_copy_and_keeps_what_landed() {
        let src = TmpDir::new("between-src");
        let dst = TmpDir::new("between-dst");
        let p1 = src.0.join("one.mp4");
        let p2 = src.0.join("two.mp4");
        std::fs::write(&p1, b"first").unwrap();
        std::fs::write(&p2, b"second").unwrap();
        let segs = vec![
            seg("camA", p1.to_str().unwrap()),
            seg("camA", p2.to_str().unwrap()),
        ];
        let err = copy_segments_to_dir(&dst.0, &segs, |copied, _| {
            std::future::ready(if copied >= 1 {
                Err(anyhow::anyhow!("backup aborted: revoked"))
            } else {
                Ok(())
            })
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("revoked"), "{err}");
        assert_eq!(
            copied_files(&dst.0, "camA"),
            vec!["one.mp4".to_string()],
            "the copy continued past the denial"
        );
    }
}
