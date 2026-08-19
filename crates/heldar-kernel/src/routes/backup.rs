//! Backup subsystem API: destinations, scheduled policies, the job ledger, and on-demand archive
//! export.
//!
//! Destinations + policies are managed by manager+; their listings (with destination credentials
//! MASKED) and the job/export ledger are readable by any authenticated principal. The actual
//! transfers run in the background backup service ([`crate::services::backup`]). All mutations are
//! written to the immutable audit log.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use sqlx::types::Json as SqlxJson;
use uuid::Uuid;

use crate::auth::{self, Cap, Principal};
use crate::error::{AppError, AppResult};
use crate::models::{
    ArchiveExportRequest, BackupDestination, BackupDestinationCreate, BackupDestinationUpdate,
    BackupDestinationView, BackupJob, BackupPolicy, BackupPolicyCreate, BackupPolicyUpdate,
    BackupTestResult, BACKUP_SECRET_KEYS,
};
use crate::services::backup;
use crate::state::AppState;
use crate::state::{camera_ids_from_json, camera_selection, confine_camera_ids};
use crate::util;
use chrono::{DateTime, Utc};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/backup/destinations",
            get(list_destinations).post(create_destination),
        )
        .route(
            "/api/v1/backup/destinations/{id}",
            axum::routing::patch(update_destination).delete(delete_destination),
        )
        .route(
            "/api/v1/backup/destinations/{id}/test",
            post(test_destination),
        )
        .route(
            "/api/v1/backup/policies",
            get(list_policies).post(create_policy),
        )
        .route(
            "/api/v1/backup/policies/{id}",
            axum::routing::patch(update_policy).delete(delete_policy),
        )
        .route("/api/v1/backup/policies/{id}/trigger", post(trigger_policy))
        .route("/api/v1/backup/jobs", get(list_jobs))
        .route("/api/v1/backup/jobs/{id}", get(get_job).delete(delete_job))
        .route("/api/v1/archive/export", post(archive_export))
        .route("/api/v1/archive/exports", get(list_archive_exports))
}

const VALID_KINDS: &[&str] = &["local", "sftp", "ftp", "s3"];

fn valid_kind(kind: &str) -> bool {
    VALID_KINDS.contains(&kind)
}

async fn load_destination(pool: &sqlx::SqlitePool, id: &str) -> AppResult<BackupDestination> {
    sqlx::query_as::<_, BackupDestination>("SELECT * FROM backup_destinations WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("backup destination {id} not found")))
}

async fn load_policy(pool: &sqlx::SqlitePool, id: &str) -> AppResult<BackupPolicy> {
    sqlx::query_as::<_, BackupPolicy>("SELECT * FROM backup_policies WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("backup policy {id} not found")))
}

async fn load_job(pool: &sqlx::SqlitePool, id: &str) -> AppResult<BackupJob> {
    sqlx::query_as::<_, BackupJob>("SELECT * FROM backup_jobs WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("backup job {id} not found")))
}

// ---- Camera scope for backup rows ----
//
// A policy, job or archive export is OWNED by the cameras named in its stored `camera_ids`. That one
// rule decides visibility and every mutation below, so it is written once here rather than five
// times inline — a rule spelled out five times is a rule that ends up spelled differently in five
// places, which is how the four bare SELECTs got shipped.

/// Whether a stored `camera_ids` selection belongs to this caller.
///
/// An EMPTY stored list means "every camera on the box" to `backup::resolve_segments`, so a row with
/// one is FLEET-level and a camera-scoped credential owns no part of it: not to read (the row
/// carries `output_path`, `bytes_copied` and the backup window of cameras it does not hold) and not
/// to act on. Anything else is owned only if it is a SUBSET of the caller's scope — a subset test
/// rather than a blanket refusal precisely because a scoped credential legitimately creates archive
/// exports and must keep seeing its own.
///
/// Contents that will not parse as an id list fail closed, for the reason `camera_ids_from_json`
/// rejects them: a selection nobody can read is not one to grant access from.
///
/// Unscoped credentials — every human role, and every key minted without a camera list — own
/// everything, so this is a structural no-op off the scoped path.
fn owns_selection(principal: &Principal, stored: &serde_json::Value) -> bool {
    if principal.camera_scope().is_none() {
        return true;
    }
    match camera_ids_from_json(stored) {
        Ok(ids) => !ids.is_empty() && ids.iter().all(|c| principal.camera_allowed(c)),
        Err(_) => false,
    }
}

/// The same visibility rule as [`owns_selection`], as a SQL predicate.
///
/// Filtering in Rust after `LIMIT` looks equivalent and is not: the limit then bounds rows EXAMINED,
/// not rows returned, so a scoped caller's own rows fall off the end behind newer fleet rows. With
/// no offset or cursor on these endpoints, past the 2000 clamp they become unreachable by any query
/// the API accepts — and policy jobs accrue a row per policy per tick, so it is only a matter of
/// uptime. The predicate has to run BEFORE the limit.
///
/// Returns `None` for an unscoped caller (no predicate at all — the overwhelming default).
fn owns_selection_sql(principal: &Principal, column: &str) -> Option<(String, Vec<String>)> {
    let ids = principal.camera_scope()?;
    let mut sorted: Vec<String> = ids.iter().cloned().collect();
    sorted.sort();
    if sorted.is_empty() {
        // A scope permitting nothing owns nothing. `AND 0` rather than an empty `IN ()`.
        return Some((" AND 0".to_string(), Vec::new()));
    }
    let placeholders = vec!["?"; sorted.len()].join(",");
    // Non-empty (an empty stored list means the whole fleet, which is fleet-level) AND every element
    // held — the subset rule, expressed as "no element falls outside the scope".
    Some((
        format!(
            " AND json_array_length({column}) > 0 \
              AND NOT EXISTS (SELECT 1 FROM json_each({column}) WHERE json_each.value NOT IN ({placeholders}))"
        ),
        sorted,
    ))
}

/// Load a policy ON BEHALF OF a caller: existence, then ownership, both missing onto the SAME 404.
///
/// A 403 for the out-of-scope case would turn the handler into an existence oracle over the id
/// space — the refusal itself would confirm that `bkp_…` names a real policy belonging to some other
/// camera, which is exactly the inference the scope boundary exists to prevent.
async fn policy_for(
    pool: &sqlx::SqlitePool,
    principal: &Principal,
    id: &str,
) -> AppResult<BackupPolicy> {
    let policy = load_policy(pool, id).await?;
    if !owns_selection(principal, &policy.camera_ids.0) {
        return Err(AppError::NotFound(format!("backup policy {id} not found")));
    }
    Ok(policy)
}

/// [`policy_for`] for a job/archive-export row, with the same 404-collapsing rationale.
async fn job_for(pool: &sqlx::SqlitePool, principal: &Principal, id: &str) -> AppResult<BackupJob> {
    let job = load_job(pool, id).await?;
    if !owns_selection(principal, &job.camera_ids.0) {
        return Err(AppError::NotFound(format!("backup job {id} not found")));
    }
    Ok(job)
}

/// The `camera_ids` a caller may STORE on a policy or export: confined to its scope, and never empty
/// for a scoped caller.
///
/// The second half is the one that is not obvious. Confinement alone can still yield `[]` — for a
/// credential scoped to no cameras at all — and `[]` is written as "the whole fleet", so the row
/// would back up every camera on the box AND be invisible afterwards to the credential that made it.
/// Refusing keeps "empty means everything" from ever being reachable from a scoped principal.
fn stored_camera_ids(principal: &Principal, requested: &[String]) -> AppResult<Vec<String>> {
    let confined = confine_camera_ids(principal, requested)?;
    if principal.camera_scope().is_some() && confined.is_empty() {
        return Err(AppError::Forbidden(
            "credential is scoped to no cameras and cannot store a fleet-wide backup selection"
                .to_string(),
        ));
    }
    Ok(confined)
}

/// Merge a new config over the existing one: any secret value the client sent back as the `***`
/// placeholder is replaced with the stored secret (so editing other fields never wipes credentials).
fn merge_secret_config(old: &serde_json::Value, mut new: serde_json::Value) -> serde_json::Value {
    if let (Some(old_obj), Some(new_obj)) = (old.as_object(), new.as_object_mut()) {
        for key in BACKUP_SECRET_KEYS {
            if new_obj.get(*key).and_then(|v| v.as_str()) == Some("***") {
                match old_obj.get(*key) {
                    Some(prev) => {
                        new_obj.insert((*key).to_string(), prev.clone());
                    }
                    None => {
                        new_obj.remove(*key);
                    }
                }
            }
        }
    }
    new
}

// ---- Destinations ----

async fn list_destinations(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<Vec<BackupDestinationView>>> {
    principal.require_cap(Cap::SystemRead, "view backup destinations")?;
    // All four sibling WRITES refuse a scoped credential because a destination is an off-box egress
    // channel; the read discloses the same channel — host, path, username, port — and was left out of
    // that batch. Reading where footage is shipped is most of the value of being able to repoint it.
    crate::routes::cameras::require_fleet_scope(&principal, "view backup destinations")?;
    let rows = sqlx::query_as::<_, BackupDestination>(
        "SELECT * FROM backup_destinations ORDER BY created_at ASC",
    )
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(
        rows.into_iter().map(BackupDestinationView::from).collect(),
    ))
}

async fn create_destination(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<BackupDestinationCreate>,
) -> AppResult<(StatusCode, Json<BackupDestinationView>)> {
    principal.require(
        principal.can_manage_registry(),
        "create backup destinations",
    )?;
    // A backup destination is an OFF-BOX EGRESS CHANNEL for recorded footage, and it names no camera
    // to scope by. A camera-scoped credential that could create one — then attach a policy to it —
    // would exfiltrate footage past every per-route check in this repair, because the bytes leave via
    // rclone rather than over HTTP, so the /media guard never sees them. Refusing outright is the only
    // containment available. A no-op for unscoped credentials, which is every human role.
    crate::routes::cameras::require_fleet_scope(&principal, "create backup destinations")?;
    let name = body.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("`name` is required".into()));
    }
    if !valid_kind(&body.kind) {
        return Err(AppError::BadRequest(
            "`kind` must be local|sftp|ftp|s3".into(),
        ));
    }
    let mut config = body.config.unwrap_or_else(|| json!({}));
    if !config.is_object() {
        return Err(AppError::BadRequest(
            "`config` must be a JSON object".into(),
        ));
    }
    // Seal the credential keys before storing. They were masked on read but written to SQLite in the
    // clear, so a stolen database or a copied DB backup yielded SFTP/FTP passwords and S3 secret keys
    // outright.
    crate::services::secrets::seal_json_keys(&mut config, BACKUP_SECRET_KEYS)
        .map_err(|e| AppError::Other(anyhow::anyhow!("sealing destination credentials: {e}")))?;
    let enabled = body.enabled.unwrap_or(true);
    let id = format!("bkd_{}", Uuid::new_v4().simple());
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO backup_destinations (id, name, kind, config, enabled, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(name)
    .bind(&body.kind)
    .bind(SqlxJson(config))
    .bind(enabled)
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await?;
    auth::audit(
        &st.pool,
        &principal,
        "create_backup_destination",
        "backup_destination",
        &id,
        json!({ "kind": &body.kind, "name": name }),
    )
    .await;
    let dest = load_destination(&st.pool, &id).await?;
    Ok((StatusCode::CREATED, Json(BackupDestinationView::from(dest))))
}

async fn update_destination(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(body): Json<BackupDestinationUpdate>,
) -> AppResult<Json<BackupDestinationView>> {
    principal.require(
        principal.can_manage_registry(),
        "update backup destinations",
    )?;
    // A destination is an OFF-BOX EGRESS CHANNEL with no camera to scope by, so the whole surface is
    // refused to a camera-scoped credential — not just creation. Guarding create alone was the gap: an
    // existing local destination could be PATCHed to attacker-controlled SFTP/S3 and then triggered,
    // and the bytes leave via rclone so the media guard never sees them.
    crate::routes::cameras::require_fleet_scope(&principal, "manage backup destinations")?;
    let cur = load_destination(&st.pool, &id).await?;

    let name = body
        .name
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cur.name.clone());
    let kind = body.kind.unwrap_or_else(|| cur.kind.clone());
    if !valid_kind(&kind) {
        return Err(AppError::BadRequest(
            "`kind` must be local|sftp|ftp|s3".into(),
        ));
    }
    let config = match body.config {
        Some(new) => {
            if !new.is_object() {
                return Err(AppError::BadRequest(
                    "`config` must be a JSON object".into(),
                ));
            }
            merge_secret_config(&cur.config.0, new)
        }
        None => cur.config.0.clone(),
    };
    // Values carried over from the stored config are already sealed; `seal_json_keys` is idempotent,
    // so this only seals a freshly supplied plaintext secret.
    let mut config = config;
    crate::services::secrets::seal_json_keys(&mut config, BACKUP_SECRET_KEYS)
        .map_err(|e| AppError::Other(anyhow::anyhow!("sealing destination credentials: {e}")))?;
    let enabled = body.enabled.unwrap_or(cur.enabled);

    sqlx::query(
        "UPDATE backup_destinations SET name = ?, kind = ?, config = ?, enabled = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&name)
    .bind(&kind)
    .bind(SqlxJson(config))
    .bind(enabled)
    .bind(Utc::now())
    .bind(&id)
    .execute(&st.pool)
    .await?;
    auth::audit(
        &st.pool,
        &principal,
        "update_backup_destination",
        "backup_destination",
        &id,
        json!({ "kind": &kind, "enabled": enabled }),
    )
    .await;
    let dest = load_destination(&st.pool, &id).await?;
    Ok(Json(BackupDestinationView::from(dest)))
}

async fn delete_destination(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<StatusCode> {
    principal.require(
        principal.can_manage_registry(),
        "delete backup destinations",
    )?;
    // A destination is an OFF-BOX EGRESS CHANNEL with no camera to scope by, so the whole surface is
    // refused to a camera-scoped credential — not just creation. Guarding create alone was the gap: an
    // existing local destination could be PATCHed to attacker-controlled SFTP/S3 and then triggered,
    // and the bytes leave via rclone so the media guard never sees them.
    crate::routes::cameras::require_fleet_scope(&principal, "manage backup destinations")?;
    let res = sqlx::query("DELETE FROM backup_destinations WHERE id = ?")
        .bind(&id)
        .execute(&st.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "backup destination {id} not found"
        )));
    }
    auth::audit(
        &st.pool,
        &principal,
        "delete_backup_destination",
        "backup_destination",
        &id,
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn test_destination(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<BackupTestResult>> {
    principal.require(principal.can_manage_registry(), "test backup destinations")?;
    // A destination is an OFF-BOX EGRESS CHANNEL with no camera to scope by, so the whole surface is
    // refused to a camera-scoped credential — not just creation. Guarding create alone was the gap: an
    // existing local destination could be PATCHed to attacker-controlled SFTP/S3 and then triggered,
    // and the bytes leave via rclone so the media guard never sees them.
    crate::routes::cameras::require_fleet_scope(&principal, "manage backup destinations")?;
    let dest = load_destination(&st.pool, &id).await?;
    let result = backup::test_destination(&st, &dest).await;
    auth::audit(
        &st.pool,
        &principal,
        "test_backup_destination",
        "backup_destination",
        &id,
        json!({ "ok": result.ok }),
    )
    .await;
    Ok(Json(result))
}

// ---- Policies ----

async fn list_policies(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<Vec<BackupPolicy>>> {
    principal.require_cap(Cap::SystemRead, "view backup policies")?;
    // Unfiltered, this listing handed a camera-scoped credential the backup posture of the whole
    // fleet: which cameras are archived where, on what schedule, and which are not backed up at all.
    // Filtered in SQL like its two siblings — this one has no LIMIT so a post-filter would also be
    // correct, but one rule with one implementation is what stops the three drifting apart.
    let scope = owns_selection_sql(&principal, "camera_ids");
    let mut sql = "SELECT * FROM backup_policies WHERE 1=1".to_string();
    if let Some((pred, _)) = &scope {
        sql.push_str(pred);
    }
    sql.push_str(" ORDER BY created_at ASC");
    let mut query = sqlx::query_as::<_, BackupPolicy>(&sql);
    if let Some((_, binds)) = &scope {
        for b in binds {
            query = query.bind(b.clone());
        }
    }
    Ok(Json(query.fetch_all(&st.pool).await?))
}

async fn create_policy(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<BackupPolicyCreate>,
) -> AppResult<(StatusCode, Json<BackupPolicy>)> {
    principal.require(principal.can_manage_registry(), "create backup policies")?;
    let name = body.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("`name` is required".into()));
    }
    // The destination must exist (FK would also reject, but a clean 404 is friendlier).
    let _ = load_destination(&st.pool, &body.destination_id).await?;
    // Confine the selection to what this credential holds. An EMPTY list means "every camera on the
    // box" to `backup::resolve_segments`, so for a camera-scoped caller it must expand to its own
    // scope rather than the fleet — otherwise the emptiest possible request is the most privileged one.
    let requested = camera_ids_from_json(&body.camera_ids.unwrap_or_else(|| json!([])))?;
    let camera_ids = json!(stored_camera_ids(&principal, &requested)?);
    let incident_lock_only = body.incident_lock_only.unwrap_or(false);
    let schedule_interval_s = body.schedule_interval_s.unwrap_or(86_400).max(60);
    let lookback_hours = body.lookback_hours.unwrap_or(0).max(0);
    let enabled = body.enabled.unwrap_or(true);
    let id = format!("bkp_{}", Uuid::new_v4().simple());
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO backup_policies
           (id, name, destination_id, camera_ids, incident_lock_only, schedule_interval_s,
            lookback_hours, last_run_at, last_job_id, enabled, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, NULL, NULL, ?, ?, ?)",
    )
    .bind(&id)
    .bind(name)
    .bind(&body.destination_id)
    .bind(SqlxJson(camera_ids))
    .bind(incident_lock_only)
    .bind(schedule_interval_s)
    .bind(lookback_hours)
    .bind(enabled)
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await?;
    auth::audit(
        &st.pool,
        &principal,
        "create_backup_policy",
        "backup_policy",
        &id,
        json!({ "destination_id": &body.destination_id, "name": name }),
    )
    .await;
    let policy = load_policy(&st.pool, &id).await?;
    Ok((StatusCode::CREATED, Json(policy)))
}

async fn update_policy(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(body): Json<BackupPolicyUpdate>,
) -> AppResult<Json<BackupPolicy>> {
    principal.require(principal.can_manage_registry(), "update backup policies")?;
    // Ownership FIRST — the handler never established it, so a camera-scoped credential could PATCH
    // any policy on the box. That is also what made the `None` arm below dangerous: on a fleet-wide
    // policy it rewrote `camera_ids` from [] to the caller's own scope, silently dropping every
    // other camera out of the backup — a targeted evidence-retention outage from a request that
    // only meant to rename a policy.
    let cur = policy_for(&st.pool, &principal, &id).await?;

    let name = body
        .name
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cur.name.clone());
    let destination_id = body
        .destination_id
        .unwrap_or_else(|| cur.destination_id.clone());
    let _ = load_destination(&st.pool, &destination_id).await?;
    let camera_ids = match body.camera_ids {
        // Confined exactly as create_policy: a caller may re-point or narrow the selection, never
        // widen it past its own scope, and an EMPTY list means "every camera on the box" downstream
        // in `backup::resolve_segments`.
        Some(v) => json!(stored_camera_ids(&principal, &camera_ids_from_json(&v)?)?),
        // The caller did not ask about camera_ids, so leave the stored value byte-for-byte alone.
        // Re-confining it here was the silent rewrite described above; ownership is established, so
        // there is nothing left for a re-confinement to protect.
        None => cur.camera_ids.0.clone(),
    };
    let incident_lock_only = body.incident_lock_only.unwrap_or(cur.incident_lock_only);
    let schedule_interval_s = body
        .schedule_interval_s
        .map(|v| v.max(60))
        .unwrap_or(cur.schedule_interval_s);
    let lookback_hours = body
        .lookback_hours
        .map(|v| v.max(0))
        .unwrap_or(cur.lookback_hours);
    let enabled = body.enabled.unwrap_or(cur.enabled);

    sqlx::query(
        "UPDATE backup_policies SET name = ?, destination_id = ?, camera_ids = ?,
            incident_lock_only = ?, schedule_interval_s = ?, lookback_hours = ?, enabled = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(&name)
    .bind(&destination_id)
    .bind(SqlxJson(camera_ids))
    .bind(incident_lock_only)
    .bind(schedule_interval_s)
    .bind(lookback_hours)
    .bind(enabled)
    .bind(Utc::now())
    .bind(&id)
    .execute(&st.pool)
    .await?;
    auth::audit(
        &st.pool,
        &principal,
        "update_backup_policy",
        "backup_policy",
        &id,
        json!({ "enabled": enabled }),
    )
    .await;
    let policy = load_policy(&st.pool, &id).await?;
    Ok(Json(policy))
}

async fn delete_policy(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<StatusCode> {
    principal.require(principal.can_manage_registry(), "delete backup policies")?;
    // The DELETE was keyed on the id alone with no confinement at all, so any credential that could
    // manage the registry could remove any policy on the box — including the fleet-wide one that is
    // the only thing archiving the cameras it does not hold. Ownership first; an out-of-scope id
    // answers the same 404 as an unknown one.
    let _ = policy_for(&st.pool, &principal, &id).await?;
    let res = sqlx::query("DELETE FROM backup_policies WHERE id = ?")
        .bind(&id)
        .execute(&st.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("backup policy {id} not found")));
    }
    auth::audit(
        &st.pool,
        &principal,
        "delete_backup_policy",
        "backup_policy",
        &id,
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn trigger_policy(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<(StatusCode, Json<BackupJob>)> {
    principal.require(principal.can_manage_registry(), "trigger backup policies")?;
    let policy = policy_for(&st.pool, &principal, &id).await?;
    // Confine at the moment of ACTUATION, not only at edit time: the policy may have been written by
    // an unscoped admin, and triggering it is what moves the bytes. The confinement is now what the
    // job actually ships — it used to be computed into `let _` and thrown away while the STORED
    // policy went downstream, so an empty (= whole fleet) selection ran fleet-wide for a scoped
    // caller. `CameraSelection` is what keeps that from being expressible again: only an unscoped
    // principal can produce `All`.
    let selection = camera_selection(&principal, &camera_ids_from_json(&policy.camera_ids.0)?)?;
    let job_id = backup::trigger_policy(&st, &policy, &selection)
        .await
        .map_err(AppError::Other)?;
    auth::audit(
        &st.pool,
        &principal,
        "trigger_backup_policy",
        "backup_policy",
        &id,
        json!({ "job_id": &job_id }),
    )
    .await;
    let job = load_job(&st.pool, &job_id).await?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

// ---- Jobs ----

#[derive(Debug, Deserialize)]
struct JobQuery {
    policy_id: Option<String>,
    status: Option<String>,
    limit: Option<i64>,
}

async fn list_jobs(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<JobQuery>,
) -> AppResult<Json<Vec<BackupJob>>> {
    principal.require_cap(Cap::SystemRead, "view backup jobs")?;
    let limit = q.limit.unwrap_or(100).clamp(1, 2000);
    // Same subset rule as the policy listing, applied IN SQL so it runs before `LIMIT` — a job row is
    // more disclosive than a policy (`output_path` is an absolute filesystem path, and
    // `bytes_copied`/`from_time`/`to_time` describe another camera's footage volume and schedule),
    // and filtering after the limit made a scoped caller's own rows unreachable behind newer fleet
    // ones.
    let scope = owns_selection_sql(&principal, "camera_ids");
    let mut sql = "SELECT * FROM backup_jobs
         WHERE (? IS NULL OR policy_id = ?)
           AND (? IS NULL OR status = ?)"
        .to_string();
    if let Some((pred, _)) = &scope {
        sql.push_str(pred);
    }
    sql.push_str(" ORDER BY created_at DESC LIMIT ?");
    let mut query = sqlx::query_as::<_, BackupJob>(&sql)
        .bind(&q.policy_id)
        .bind(&q.policy_id)
        .bind(&q.status)
        .bind(&q.status);
    if let Some((_, binds)) = &scope {
        for b in binds {
            query = query.bind(b.clone());
        }
    }
    let rows = query.bind(limit).fetch_all(&st.pool).await?;
    Ok(Json(rows))
}

async fn get_job(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<BackupJob>> {
    principal.require_cap(Cap::SystemRead, "view backup jobs")?;
    // 404 for an out-of-scope job, identical to an unknown id — see `job_for`.
    let job = job_for(&st.pool, &principal, &id).await?;
    Ok(Json(job))
}

async fn delete_job(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<StatusCode> {
    principal.require(principal.can_manage_registry(), "delete backup jobs")?;
    // Ownership BEFORE the unlink below. `output_path` is a real file holding another camera's
    // exported evidence and `remove_file` does not come back — this handler destroyed it for any
    // credential that could name the job id. An out-of-scope id answers the same 404 as an unknown
    // one, so it cannot be used to sweep the id space for jobs worth destroying either.
    let job = job_for(&st.pool, &principal, &id).await?;
    // Remove the produced archive artifact, if any, before dropping the row.
    if let Some(path) = &job.output_path {
        let _ = tokio::fs::remove_file(path).await;
    }
    // ...and forget its attribution with it, so `media_artifacts` never describes bytes that are
    // gone. The key is derived from the SERVED url rather than rebuilt from the job id, so it is the
    // same key the guard looks up by construction. Retention's mtime prune of `archive_dir` is left
    // to the existence-based `media_scope::sweep_orphans`, which cannot act until the file is
    // already gone; an operator DELETE is explicit and immediate and should not wait a sweep cycle.
    if let Some(key) = job
        .output_url
        .as_deref()
        .and_then(crate::services::media_scope::artifact_key)
    {
        crate::services::media_scope::forget(&st.pool, &key).await;
    }
    sqlx::query("DELETE FROM backup_jobs WHERE id = ?")
        .bind(&id)
        .execute(&st.pool)
        .await?;
    auth::audit(
        &st.pool,
        &principal,
        "delete_backup_job",
        "backup_job",
        &id,
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

// ---- On-demand archive export ----

fn parse_opt_ts(s: &Option<String>, field: &str) -> AppResult<Option<DateTime<Utc>>> {
    match s {
        Some(v) => util::parse_rfc3339(v)
            .map(Some)
            .ok_or_else(|| AppError::BadRequest(format!("invalid `{field}` timestamp"))),
        None => Ok(None),
    }
}

async fn archive_export(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<ArchiveExportRequest>,
) -> AppResult<(StatusCode, Json<BackupJob>)> {
    principal.require(principal.can_manage_registry(), "export archives")?;
    let from = parse_opt_ts(&body.from, "from")?;
    let to = parse_opt_ts(&body.to, "to")?;
    if let (Some(f), Some(t)) = (from, to) {
        if f > t {
            return Err(AppError::BadRequest("`from` must be <= `to`".into()));
        }
    }
    // Same confinement as a policy: an empty list selects the whole box downstream. Note the sharp
    // edge this leaves — `backup::create_archive` still takes a plain `Vec<String>` where empty
    // means every camera, so the guarantee lives in this line rather than in the type. It holds
    // because `stored_camera_ids` refuses to produce an empty list for a scoped caller.
    let camera_ids = stored_camera_ids(&principal, &body.camera_ids)?;
    let incident_lock_only = body.incident_lock_only.unwrap_or(false);
    let trim = body.trim.unwrap_or(false);
    let job =
        backup::create_archive(&st, camera_ids.clone(), from, to, incident_lock_only, trim).await?;
    auth::audit(
        &st.pool,
        &principal,
        "create_archive_export",
        "backup_job",
        &job.id,
        json!({ "camera_ids": camera_ids, "incident_lock_only": incident_lock_only, "trim": trim }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(job)))
}

#[derive(Debug, Deserialize)]
struct LimitQuery {
    limit: Option<i64>,
}

async fn list_archive_exports(
    State(st): State<AppState>,
    principal: Principal,
    Query(q): Query<LimitQuery>,
) -> AppResult<Json<Vec<BackupJob>>> {
    principal.require_cap(Cap::SystemRead, "view archive exports")?;
    let limit = q.limit.unwrap_or(100).clamp(1, 2000);
    // A scoped credential legitimately CREATES exports here (`archive_export` confines the list it
    // stores), so this is a subset filter and not a refusal: it must keep seeing its own exports —
    // including the `/media/archives/…` URL it needs to fetch them — while the fleet's stay hidden.
    // In SQL, so its own export cannot be pushed off the end of the page by newer fleet exports.
    let scope = owns_selection_sql(&principal, "camera_ids");
    let mut sql = "SELECT * FROM backup_jobs WHERE kind = 'on_demand_archive'".to_string();
    if let Some((pred, _)) = &scope {
        sql.push_str(pred);
    }
    sql.push_str(" ORDER BY created_at DESC LIMIT ?");
    let mut query = sqlx::query_as::<_, BackupJob>(&sql);
    if let Some((_, binds)) = &scope {
        for b in binds {
            query = query.bind(b.clone());
        }
    }
    let rows = query.bind(limit).fetch_all(&st.pool).await?;
    Ok(Json(rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Scope;
    use std::collections::HashSet;
    use std::sync::Arc;

    async fn test_state() -> AppState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let cfg = Arc::new(crate::config::Config::from_env());
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

    fn scoped(cameras: &[&str]) -> Principal {
        let set: HashSet<String> = cameras.iter().map(|c| c.to_string()).collect();
        Principal {
            scope: Scope::Cameras(Arc::new(set)),
            ..Principal::system_admin()
        }
    }

    /// The fixture every attack below runs against: one destination, three policies (fleet-wide,
    /// cam_a's, cam_sentinel_b's) and the matching jobs. `cam_sentinel_b` is named so that any leak
    /// of it into a response body or an error message is unmistakable.
    async fn seed(pool: &sqlx::SqlitePool) {
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO backup_destinations (id, name, kind, config, enabled, created_at, updated_at)
             VALUES ('bkd_1', 'nas', 'local', '{}', 1, ?, ?)",
        )
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
        for (id, cams) in [
            ("bkp_fleet", "[]"),
            ("bkp_a", r#"["cam_a"]"#),
            ("bkp_b", r#"["cam_sentinel_b"]"#),
        ] {
            sqlx::query(
                "INSERT INTO backup_policies
                   (id, name, destination_id, camera_ids, incident_lock_only, schedule_interval_s,
                    lookback_hours, enabled, created_at, updated_at)
                 VALUES (?, ?, 'bkd_1', ?, 0, 86400, 0, 1, ?, ?)",
            )
            .bind(id)
            .bind(id)
            .bind(cams)
            .bind(now)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
        }
        for (id, policy, cams, kind) in [
            ("bkj_fleet", Some("bkp_fleet"), "[]", "policy"),
            ("bkj_a", Some("bkp_a"), r#"["cam_a"]"#, "policy"),
            ("bkj_b", Some("bkp_b"), r#"["cam_sentinel_b"]"#, "policy"),
            ("bkj_exp_a", None, r#"["cam_a"]"#, "on_demand_archive"),
            (
                "bkj_exp_b",
                None,
                r#"["cam_sentinel_b"]"#,
                "on_demand_archive",
            ),
            ("bkj_exp_fleet", None, "[]", "on_demand_archive"),
        ] {
            sqlx::query(
                "INSERT INTO backup_jobs (id, policy_id, destination_id, kind, camera_ids, status,
                                          output_path, created_at)
                 VALUES (?, ?, 'bkd_1', ?, ?, 'completed', ?, ?)",
            )
            .bind(id)
            .bind(policy)
            .bind(kind)
            .bind(cams)
            .bind(format!("/tmp/heldar-test-{id}.zip"))
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
        }
    }

    async fn policy_camera_ids(pool: &sqlx::SqlitePool, id: &str) -> String {
        sqlx::query_scalar::<_, String>("SELECT camera_ids FROM backup_policies WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// F2 (read): the four bare SELECTs served every policy, job and export on the box to a
    /// camera-scoped credential, including the absolute `output_path` and byte counts of footage it
    /// does not hold. A fleet-wide (`[]` = every camera) row belongs to nobody in particular, so it
    /// is not visible either.
    #[tokio::test]
    async fn a_scoped_key_sees_only_its_own_backup_rows() {
        let st = test_state().await;
        seed(&st.pool).await;
        let p = scoped(&["cam_a"]);

        let policies = list_policies(State(st.clone()), p.clone()).await.unwrap().0;
        let ids: Vec<&str> = policies.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, vec!["bkp_a"], "fleet + cam_sentinel_b policies leaked");

        let jobs = list_jobs(
            State(st.clone()),
            p.clone(),
            Query(JobQuery {
                policy_id: None,
                status: None,
                limit: None,
            }),
        )
        .await
        .unwrap()
        .0;
        // Sorted: the fixture's rows share a created_at, so the DESC tie-break is arbitrary.
        let mut ids: Vec<&str> = jobs.iter().map(|x| x.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["bkj_a", "bkj_exp_a"], "job ledger leaked");

        // A scoped credential legitimately CREATES exports, so it must still see its OWN — the rule
        // is a subset filter, not a blanket refusal.
        let exports = list_archive_exports(
            State(st.clone()),
            p.clone(),
            Query(LimitQuery { limit: None }),
        )
        .await
        .unwrap()
        .0;
        let ids: Vec<&str> = exports.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, vec!["bkj_exp_a"]);

        // Naming another policy in the filter does not reach its jobs either.
        let jobs = list_jobs(
            State(st.clone()),
            p,
            Query(JobQuery {
                policy_id: Some("bkp_b".into()),
                status: None,
                limit: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(jobs.is_empty(), "policy_id filter reached another camera");
    }

    /// F2 (single row): an out-of-scope job must answer exactly like a nonexistent one, or the
    /// handler is an existence oracle over the id space.
    #[tokio::test]
    async fn an_out_of_scope_job_is_indistinguishable_from_a_missing_one() {
        let st = test_state().await;
        seed(&st.pool).await;
        let p = scoped(&["cam_a"]);

        let nonexistent = get_job(State(st.clone()), Path("bkj_zzz".into()), p.clone())
            .await
            .unwrap_err();
        for id in ["bkj_b", "bkj_fleet"] {
            let refused = get_job(State(st.clone()), Path(id.into()), p.clone())
                .await
                .unwrap_err();
            assert!(
                matches!(refused, AppError::NotFound(_)),
                "{id}: {refused:?}"
            );
            assert_eq!(
                refused.to_string().replace(id, "bkj_zzz"),
                nonexistent.to_string(),
                "{id} answers differently from an unknown id"
            );
            assert!(!refused.to_string().contains("cam_sentinel_b"));
        }
        // No over-blocking on its own job.
        assert_eq!(
            get_job(State(st.clone()), Path("bkj_a".into()), p)
                .await
                .unwrap()
                .0
                .id,
            "bkj_a"
        );
    }

    /// F3: `delete_job` unlinked `output_path` before checking anything — a scoped credential could
    /// permanently destroy another camera's exported evidence by naming its job id.
    #[tokio::test]
    async fn a_scoped_key_cannot_destroy_another_cameras_exported_evidence() {
        let st = test_state().await;
        seed(&st.pool).await;
        let p = scoped(&["cam_a"]);

        let victim =
            std::env::temp_dir().join(format!("heldar-evidence-{}.zip", std::process::id()));
        std::fs::write(&victim, b"another camera's footage").unwrap();
        sqlx::query("UPDATE backup_jobs SET output_path = ? WHERE id = 'bkj_b'")
            .bind(victim.to_string_lossy().to_string())
            .execute(&st.pool)
            .await
            .unwrap();

        let refused = delete_job(State(st.clone()), Path("bkj_b".into()), p.clone())
            .await
            .unwrap_err();
        assert!(matches!(refused, AppError::NotFound(_)), "{refused:?}");
        assert!(
            victim.exists(),
            "the archive of a camera outside the scope was deleted"
        );
        let still: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM backup_jobs WHERE id IN ('bkj_b','bkj_fleet')",
        )
        .fetch_one(&st.pool)
        .await
        .unwrap();
        assert_eq!(still, 2);

        // The fleet-wide job is equally out of reach.
        assert!(matches!(
            delete_job(State(st.clone()), Path("bkj_fleet".into()), p.clone())
                .await
                .unwrap_err(),
            AppError::NotFound(_)
        ));
        // Its own job still deletes.
        assert_eq!(
            delete_job(State(st.clone()), Path("bkj_a".into()), p)
                .await
                .unwrap(),
            StatusCode::NO_CONTENT
        );
        std::fs::remove_file(&victim).ok();
    }

    /// F4: `DELETE ... WHERE id = ?` with no confinement — a scoped credential could delete the
    /// fleet-wide policy that is the only thing archiving every other camera.
    #[tokio::test]
    async fn a_scoped_key_cannot_delete_policies_it_does_not_own() {
        let st = test_state().await;
        seed(&st.pool).await;
        let p = scoped(&["cam_a"]);

        let nonexistent = delete_policy(State(st.clone()), Path("bkp_zzz".into()), p.clone())
            .await
            .unwrap_err();
        for id in ["bkp_fleet", "bkp_b"] {
            let refused = delete_policy(State(st.clone()), Path(id.into()), p.clone())
                .await
                .unwrap_err();
            assert!(
                matches!(refused, AppError::NotFound(_)),
                "{id}: {refused:?}"
            );
            assert_eq!(
                refused.to_string().replace(id, "bkp_zzz"),
                nonexistent.to_string()
            );
        }
        let left: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM backup_policies WHERE id IN ('bkp_fleet','bkp_b')",
        )
        .fetch_one(&st.pool)
        .await
        .unwrap();
        assert_eq!(left, 2, "a policy outside the scope was deleted");

        assert_eq!(
            delete_policy(State(st.clone()), Path("bkp_a".into()), p)
                .await
                .unwrap(),
            StatusCode::NO_CONTENT
        );
    }

    /// F5: `update_policy` never established ownership, and its `None` arm re-confined the STORED
    /// list — so PATCHing an unrelated field on the fleet-wide policy both succeeded AND rewrote its
    /// camera_ids from [] to the caller's scope, silently dropping every other camera out of the
    /// backup.
    #[tokio::test]
    async fn a_scoped_key_cannot_patch_a_policy_it_does_not_own() {
        let st = test_state().await;
        seed(&st.pool).await;
        let p = scoped(&["cam_a"]);

        let body: BackupPolicyUpdate = serde_json::from_value(json!({ "enabled": false })).unwrap();
        let refused = update_policy(
            State(st.clone()),
            Path("bkp_fleet".into()),
            p.clone(),
            Json(body),
        )
        .await
        .unwrap_err();
        assert!(matches!(refused, AppError::NotFound(_)), "{refused:?}");
        assert_eq!(
            policy_camera_ids(&st.pool, "bkp_fleet").await,
            "[]",
            "the fleet-wide selection was narrowed to the caller's scope"
        );
        let enabled: i64 =
            sqlx::query_scalar("SELECT enabled FROM backup_policies WHERE id = 'bkp_fleet'")
                .fetch_one(&st.pool)
                .await
                .unwrap();
        assert_eq!(enabled, 1, "the fleet-wide policy was disabled");

        let body: BackupPolicyUpdate = serde_json::from_value(json!({ "enabled": false })).unwrap();
        assert!(matches!(
            update_policy(
                State(st.clone()),
                Path("bkp_b".into()),
                p.clone(),
                Json(body)
            )
            .await
            .unwrap_err(),
            AppError::NotFound(_)
        ));

        // Widening its OWN policy to a camera it does not hold is refused as well.
        let body: BackupPolicyUpdate =
            serde_json::from_value(json!({ "camera_ids": ["cam_a", "cam_sentinel_b"] })).unwrap();
        assert!(matches!(
            update_policy(
                State(st.clone()),
                Path("bkp_a".into()),
                p.clone(),
                Json(body)
            )
            .await
            .unwrap_err(),
            AppError::Forbidden(_)
        ));
        assert_eq!(policy_camera_ids(&st.pool, "bkp_a").await, r#"["cam_a"]"#);

        // ...while an edit that leaves camera_ids alone works and does not touch the selection.
        let body: BackupPolicyUpdate =
            serde_json::from_value(json!({ "name": "renamed" })).unwrap();
        let updated = update_policy(State(st.clone()), Path("bkp_a".into()), p, Json(body))
            .await
            .unwrap()
            .0;
        assert_eq!(updated.name, "renamed");
        assert_eq!(policy_camera_ids(&st.pool, "bkp_a").await, r#"["cam_a"]"#);
    }

    /// F1: the confinement was computed into `let _` and discarded while the STORED policy went
    /// downstream, so triggering a fleet-wide policy ran a fleet-wide backup for a scoped caller.
    /// Ownership now refuses the trigger outright, and no job may be created by the attempt.
    #[tokio::test]
    async fn a_scoped_key_cannot_trigger_a_policy_it_does_not_own() {
        let st = test_state().await;
        seed(&st.pool).await;
        let p = scoped(&["cam_a"]);

        let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backup_jobs")
            .fetch_one(&st.pool)
            .await
            .unwrap();
        for id in ["bkp_fleet", "bkp_b"] {
            let refused = trigger_policy(State(st.clone()), Path(id.into()), p.clone())
                .await
                .unwrap_err();
            assert!(
                matches!(refused, AppError::NotFound(_)),
                "{id}: {refused:?}"
            );
        }
        let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backup_jobs")
            .fetch_one(&st.pool)
            .await
            .unwrap();
        assert_eq!(after, before, "a refused trigger still created a job");
    }

    /// The unscoped principal — every human role, and the auth-disabled default — must be untouched
    /// by all of the above, including on the fleet-wide rows.
    #[tokio::test]
    async fn an_unscoped_principal_is_unaffected() {
        let st = test_state().await;
        seed(&st.pool).await;
        let admin = Principal::system_admin();

        assert_eq!(
            list_policies(State(st.clone()), admin.clone())
                .await
                .unwrap()
                .0
                .len(),
            3
        );
        assert_eq!(
            list_jobs(
                State(st.clone()),
                admin.clone(),
                Query(JobQuery {
                    policy_id: None,
                    status: None,
                    limit: None,
                }),
            )
            .await
            .unwrap()
            .0
            .len(),
            6
        );
        assert_eq!(
            list_archive_exports(
                State(st.clone()),
                admin.clone(),
                Query(LimitQuery { limit: None })
            )
            .await
            .unwrap()
            .0
            .len(),
            3
        );
        assert_eq!(
            get_job(State(st.clone()), Path("bkj_fleet".into()), admin.clone())
                .await
                .unwrap()
                .0
                .id,
            "bkj_fleet"
        );
        // The pre-existing 404 for a genuinely unknown id is unchanged.
        match get_job(State(st.clone()), Path("bkj_zzz".into()), admin.clone())
            .await
            .unwrap_err()
        {
            AppError::NotFound(m) => assert_eq!(m, "backup job bkj_zzz not found"),
            other => panic!("expected the pre-existing 404, got {other:?}"),
        }
        // An admin may still edit a fleet-wide policy without its selection being rewritten.
        let body: BackupPolicyUpdate = serde_json::from_value(json!({ "name": "fleet" })).unwrap();
        let _ = update_policy(
            State(st.clone()),
            Path("bkp_fleet".into()),
            admin.clone(),
            Json(body),
        )
        .await
        .unwrap();
        assert_eq!(policy_camera_ids(&st.pool, "bkp_fleet").await, "[]");
        assert_eq!(
            delete_policy(State(st.clone()), Path("bkp_fleet".into()), admin)
                .await
                .unwrap(),
            StatusCode::NO_CONTENT
        );
    }

    #[tokio::test]
    async fn owns_selection_is_the_one_visibility_rule() {
        let p = scoped(&["cam_a", "cam_c"]);
        // Subset of the scope: owned.
        assert!(owns_selection(&p, &json!(["cam_a"])));
        assert!(owns_selection(&p, &json!(["cam_a", "cam_c"])));
        // Empty == the whole fleet, so it is never owned by a scoped credential.
        assert!(!owns_selection(&p, &json!([])));
        // Any camera outside the scope taints the whole row.
        assert!(!owns_selection(&p, &json!(["cam_a", "cam_sentinel_b"])));
        // Unparseable contents fail closed rather than degrading to "no cameras named".
        assert!(!owns_selection(&p, &json!([1, 2])));
        assert!(!owns_selection(&p, &json!("cam_a")));
        // The unscoped principal owns everything, fleet-wide rows included.
        let admin = Principal::system_admin();
        assert!(owns_selection(&admin, &json!([])));
        assert!(owns_selection(&admin, &json!(["cam_sentinel_b"])));
    }

    #[test]
    fn a_scoped_caller_can_never_store_the_empty_fleet_selection() {
        // A credential scoped to no cameras is the one case where confinement alone still yields the
        // empty list, which is written as "every camera on the box".
        let none = scoped(&[]);
        assert!(matches!(
            stored_camera_ids(&none, &[]).unwrap_err(),
            AppError::Forbidden(_)
        ));
        // A scoped caller that named nothing gets its own scope, never the fleet.
        assert_eq!(
            stored_camera_ids(&scoped(&["cam_a"]), &[]).unwrap(),
            vec!["cam_a".to_string()]
        );
        // The unscoped default is untouched: empty still means the whole fleet for an admin.
        assert!(stored_camera_ids(&Principal::system_admin(), &[])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn valid_kind_accepts_known_and_rejects_others() {
        assert!(valid_kind("local"));
        assert!(valid_kind("sftp"));
        assert!(valid_kind("ftp"));
        assert!(valid_kind("s3"));
        // Unknown, empty, and wrong-case are all rejected (case-sensitive match).
        assert!(!valid_kind("gcs"));
        assert!(!valid_kind(""));
        assert!(!valid_kind("Local"));
        assert!(!valid_kind("S3"));
    }

    #[test]
    fn valid_kinds_list_is_exact() {
        assert_eq!(VALID_KINDS, &["local", "sftp", "ftp", "s3"]);
    }

    #[test]
    fn merge_secret_config_restores_placeholder_from_old() {
        let old = json!({ "password": "hunter2", "host": "old.example" });
        let new = json!({ "password": "***", "host": "new.example" });
        let merged = merge_secret_config(&old, new);
        // The *** placeholder is replaced with the stored secret...
        assert_eq!(merged["password"], json!("hunter2"));
        // ...while non-secret fields take the newly submitted value.
        assert_eq!(merged["host"], json!("new.example"));
    }

    #[test]
    fn merge_secret_config_drops_placeholder_when_no_stored_secret() {
        let old = json!({ "host": "h" });
        let new = json!({ "secret": "***", "host": "h2" });
        let merged = merge_secret_config(&old, new);
        // No previously-stored `secret`, so the placeholder key is removed entirely.
        assert!(merged.as_object().unwrap().get("secret").is_none());
        assert_eq!(merged["host"], json!("h2"));
    }

    #[test]
    fn merge_secret_config_keeps_new_non_placeholder_secret() {
        let old = json!({ "password": "old" });
        let new = json!({ "password": "brandnew" });
        let merged = merge_secret_config(&old, new);
        // A real new value (not the placeholder) is preserved, overwriting the old secret.
        assert_eq!(merged["password"], json!("brandnew"));
    }

    #[test]
    fn merge_secret_config_handles_all_secret_keys() {
        let old = json!({ "pass": "a", "password": "b", "secret_key": "c", "secret": "d" });
        let new = json!({ "pass": "***", "password": "***", "secret_key": "***", "secret": "***" });
        let merged = merge_secret_config(&old, new);
        assert_eq!(merged["pass"], json!("a"));
        assert_eq!(merged["password"], json!("b"));
        assert_eq!(merged["secret_key"], json!("c"));
        assert_eq!(merged["secret"], json!("d"));
    }

    #[test]
    fn merge_secret_config_passthrough_for_non_objects() {
        // When either side is not a JSON object, the new value is returned unchanged.
        let old = json!("not-an-object");
        let new = json!([1, 2, 3]);
        let merged = merge_secret_config(&old, new.clone());
        assert_eq!(merged, new);
    }

    #[test]
    fn parse_opt_ts_none_valid_and_invalid() {
        // None input yields Ok(None).
        assert!(parse_opt_ts(&None, "from").unwrap().is_none());

        // A valid RFC3339 timestamp (trailing Z accepted) parses to Some.
        let ok = parse_opt_ts(&Some("2024-01-02T03:04:05Z".to_string()), "from").unwrap();
        assert!(ok.is_some());

        // An invalid timestamp surfaces a BadRequest mentioning the field name.
        let err = parse_opt_ts(&Some("not-a-timestamp".to_string()), "to").unwrap_err();
        match err {
            AppError::BadRequest(msg) => {
                assert!(msg.contains("to"));
                assert!(msg.contains("invalid"));
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }
}
