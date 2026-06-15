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

use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::models::{BackupDestination, BackupJob, BackupPolicy, BackupTestResult, Segment};
use crate::repo;
use crate::state::AppState;

/// Hardcoded archiver (the environment provides /usr/bin/zip + tar).
const ZIP_BIN: &str = "/usr/bin/zip";

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
        match create_policy_job(state, &p).await {
            Ok(job_id) => spawn_job(state.clone(), job_id),
            Err(e) => tracing::error!(policy = %p.id, error = %e, "backup: failed to create job"),
        }
    }
    Ok(())
}

/// Insert a `policy` job from a policy and claim the policy (`last_run_at`/`last_job_id`) so the next
/// tick does not re-trigger it. Returns the new job id.
async fn create_policy_job(state: &AppState, p: &BackupPolicy) -> anyhow::Result<String> {
    let now = Utc::now();
    let job_id = format!("bkj_{}", Uuid::new_v4().simple());
    let from_time = if p.lookback_hours > 0 {
        Some(now - chrono::Duration::hours(p.lookback_hours))
    } else {
        None
    };
    let to_time = Some(now);
    sqlx::query(
        "INSERT INTO backup_jobs
           (id, policy_id, destination_id, kind, camera_ids, from_time, to_time,
            incident_lock_only, status, created_at)
         VALUES (?, ?, ?, 'policy', ?, ?, ?, ?, 'pending', ?)",
    )
    .bind(&job_id)
    .bind(&p.id)
    .bind(&p.destination_id)
    .bind(SqlxJson(p.camera_ids.0.clone()))
    .bind(from_time)
    .bind(to_time)
    .bind(p.incident_lock_only)
    .bind(now)
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
pub async fn trigger_policy(state: &AppState, policy: &BackupPolicy) -> anyhow::Result<String> {
    let job_id = create_policy_job(state, policy).await?;
    spawn_job(state.clone(), job_id.clone());
    Ok(job_id)
}

/// Spawn a detached task that acquires a concurrency permit then executes the job under the timeout.
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

    // Read-lock the source segments so retention cannot prune them mid-transfer; always released
    // after the (possibly timed-out) transfer future settles.
    let seg_ids: Vec<String> = segments.iter().map(|s| s.id.clone()).collect();
    repo::set_segments_locked(&state.pool, &seg_ids, true).await;
    let outcome =
        tokio::time::timeout(timeout, copy_segments(state, job_id, &dest, &segments)).await;
    repo::set_segments_locked(&state.pool, &seg_ids, false).await;

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
    job_id: &str,
    dest: &BackupDestination,
    segments: &[Segment],
) -> anyhow::Result<(u64, u64)> {
    match dest.kind.as_str() {
        "local" => copy_local(state, job_id, dest, segments).await,
        "sftp" | "ftp" | "s3" => copy_rclone(state, job_id, dest, segments).await,
        other => anyhow::bail!("unknown backup destination kind `{other}`"),
    }
}

/// Local / NAS-mount destination: std fs copy into `{path}/{camera_id}/{file}`.
async fn copy_local(
    state: &AppState,
    job_id: &str,
    dest: &BackupDestination,
    segments: &[Segment],
) -> anyhow::Result<(u64, u64)> {
    let base = cfg_str(&dest.config.0, "path");
    if base.is_empty() {
        anyhow::bail!("local destination has no `path` configured");
    }
    let base = Path::new(&base);
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
                tracing::warn!(job = job_id, path = %seg.path, "backup: source segment vanished; skipping");
            }
            Err(e) => anyhow::bail!("copying {}: {e}", seg.path),
        }
        update_progress(state, job_id, copied, bytes).await;
    }
    Ok((copied, bytes))
}

/// Remote destination (sftp/ftp/s3) via rclone. Degrades to a clear error when rclone is missing.
async fn copy_rclone(
    state: &AppState,
    job_id: &str,
    dest: &BackupDestination,
    segments: &[Segment],
) -> anyhow::Result<(u64, u64)> {
    let bin = &state.cfg.rclone_bin;
    if !binary_available(bin).await {
        anyhow::bail!(
            "rclone binary `{bin}` is not available; install rclone or set HELDAR_RCLONE_BIN \
             (remote sftp/ftp/s3 backup requires it; local/NAS destinations do not)"
        );
    }
    let (remote, base, secrets) = build_remote(bin, &dest.kind, &dest.config.0).await?;
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
        update_progress(state, job_id, copied, bytes).await;
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

/// Build a `.zip` of the selected segments and record it as an `on_demand_archive` job. Enforces the
/// archive size cap on the source footprint; runs inline (bounded by the job timeout) so the returned
/// job already carries `output_url`.
pub async fn create_archive(
    state: &AppState,
    camera_ids: Vec<String>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    incident_lock_only: bool,
    trim: bool,
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

    tokio::fs::create_dir_all(&state.cfg.archive_dir)
        .await
        .map_err(|e| AppError::Other(e.into()))?;

    let job_id = format!("bkj_{}", Uuid::new_v4().simple());
    let now = Utc::now();
    let files_total = segments.len() as i64;
    sqlx::query(
        "INSERT INTO backup_jobs
           (id, policy_id, destination_id, kind, camera_ids, from_time, to_time,
            incident_lock_only, status, files_total, started_at, created_at)
         VALUES (?, NULL, NULL, 'on_demand_archive', ?, ?, ?, ?, 'running', ?, ?, ?)",
    )
    .bind(&job_id)
    .bind(SqlxJson(json_from_strs(&camera_ids)))
    .bind(from)
    .bind(to)
    .bind(incident_lock_only)
    .bind(files_total)
    .bind(now)
    .bind(now)
    .execute(&state.pool)
    .await?;

    // Read-lock the sources for the duration of the zip/trim (released on every outcome).
    let seg_ids: Vec<String> = segments.iter().map(|s| s.id.clone()).collect();
    repo::set_segments_locked(&state.pool, &seg_ids, true).await;
    let timeout = Duration::from_secs(state.cfg.backup_job_timeout_s.max(30));
    let outcome = tokio::time::timeout(
        timeout,
        build_archive_zip(state, &job_id, &segments, from, to, trim),
    )
    .await;
    repo::set_segments_locked(&state.pool, &seg_ids, false).await;

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
    let (remote, base, secrets) = build_remote(bin, &dest.kind, &dest.config.0).await?;
    let target = format!("{remote}{base}");
    let out = tokio::time::timeout(
        Duration::from_secs(30),
        Command::new(bin)
            .arg("lsd")
            .arg(&target)
            .args(["--max-depth", "1"])
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
/// Returns (remote_prefix_ending_in_colon, base_path, secrets_to_scrub_from_logs).
async fn build_remote(
    bin: &str,
    kind: &str,
    config: &Value,
) -> anyhow::Result<(String, String, Vec<String>)> {
    let mut secrets: Vec<String> = Vec::new();
    match kind {
        "sftp" | "ftp" => {
            let host = cfg_str(config, "host");
            if host.is_empty() {
                anyhow::bail!("{kind} destination requires `host`");
            }
            let user = cfg_str(config, "user");
            let pass = cfg_str(config, "pass");
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
                let obscured = rclone_obscure(bin, &pass).await?;
                secrets.push(obscured.clone());
                secrets.push(pass.clone());
                parts.push(format!("pass={obscured}"));
            }
            Ok((
                format!("{}:", parts.join(",")),
                cfg_str(config, "path"),
                secrets,
            ))
        }
        "s3" => {
            let bucket = cfg_str(config, "bucket");
            if bucket.is_empty() {
                anyhow::bail!("s3 destination requires `bucket`");
            }
            let access_key = cfg_str(config, "access_key");
            let secret_key = cfg_str(config, "secret_key");
            let endpoint = cfg_str(config, "endpoint");
            let region = cfg_str(config, "region");
            let mut parts = vec![":s3".to_string(), "provider=Other".to_string()];
            if !access_key.is_empty() {
                parts.push(format!("access_key_id={access_key}"));
            }
            if !secret_key.is_empty() {
                secrets.push(secret_key.clone());
                parts.push(format!("secret_access_key={secret_key}"));
            }
            if !endpoint.is_empty() {
                parts.push(format!("endpoint={endpoint}"));
            }
            if !region.is_empty() {
                parts.push(format!("region={region}"));
            }
            let base = join_path("", &[&bucket, &cfg_str(config, "prefix")]);
            Ok((format!("{}:", parts.join(",")), base, secrets))
        }
        other => anyhow::bail!("kind `{other}` does not use rclone"),
    }
}

/// Obscure a plaintext password into rclone's at-rest form (only invoked when rclone is present).
async fn rclone_obscure(bin: &str, pass: &str) -> anyhow::Result<String> {
    let out = Command::new(bin)
        .arg("obscure")
        .arg(pass)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("spawning rclone obscure: {e}"))?;
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
}
