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

use crate::auth::{self, Principal};
use crate::error::{AppError, AppResult};
use crate::models::{
    ArchiveExportRequest, BackupDestination, BackupDestinationCreate, BackupDestinationUpdate,
    BackupDestinationView, BackupJob, BackupPolicy, BackupPolicyCreate, BackupPolicyUpdate,
    BackupTestResult, BACKUP_SECRET_KEYS,
};
use crate::services::backup;
use crate::state::AppState;
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
    principal.require(principal.can_view(), "view backup destinations")?;
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
    let name = body.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("`name` is required".into()));
    }
    if !valid_kind(&body.kind) {
        return Err(AppError::BadRequest(
            "`kind` must be local|sftp|ftp|s3".into(),
        ));
    }
    let config = body.config.unwrap_or_else(|| json!({}));
    if !config.is_object() {
        return Err(AppError::BadRequest(
            "`config` must be a JSON object".into(),
        ));
    }
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
    principal.require(principal.can_view(), "view backup policies")?;
    let rows =
        sqlx::query_as::<_, BackupPolicy>("SELECT * FROM backup_policies ORDER BY created_at ASC")
            .fetch_all(&st.pool)
            .await?;
    Ok(Json(rows))
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
    let camera_ids = body.camera_ids.unwrap_or_else(|| json!([]));
    if !camera_ids.is_array() {
        return Err(AppError::BadRequest(
            "`camera_ids` must be a JSON array of camera ids".into(),
        ));
    }
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
    let cur = load_policy(&st.pool, &id).await?;

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
        Some(v) => {
            if !v.is_array() {
                return Err(AppError::BadRequest(
                    "`camera_ids` must be a JSON array of camera ids".into(),
                ));
            }
            v
        }
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
    let policy = load_policy(&st.pool, &id).await?;
    let job_id = backup::trigger_policy(&st, &policy)
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
    principal.require(principal.can_view(), "view backup jobs")?;
    let limit = q.limit.unwrap_or(100).clamp(1, 2000);
    let rows = sqlx::query_as::<_, BackupJob>(
        "SELECT * FROM backup_jobs
         WHERE (? IS NULL OR policy_id = ?)
           AND (? IS NULL OR status = ?)
         ORDER BY created_at DESC LIMIT ?",
    )
    .bind(&q.policy_id)
    .bind(&q.policy_id)
    .bind(&q.status)
    .bind(&q.status)
    .bind(limit)
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(rows))
}

async fn get_job(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<BackupJob>> {
    principal.require(principal.can_view(), "view backup jobs")?;
    let job = load_job(&st.pool, &id).await?;
    Ok(Json(job))
}

async fn delete_job(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<StatusCode> {
    principal.require(principal.can_manage_registry(), "delete backup jobs")?;
    let job = load_job(&st.pool, &id).await?;
    // Remove the produced archive artifact, if any, before dropping the row.
    if let Some(path) = &job.output_path {
        let _ = tokio::fs::remove_file(path).await;
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
    let camera_ids = body.camera_ids;
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
    principal.require(principal.can_view(), "view archive exports")?;
    let limit = q.limit.unwrap_or(100).clamp(1, 2000);
    let rows = sqlx::query_as::<_, BackupJob>(
        "SELECT * FROM backup_jobs WHERE kind = 'on_demand_archive' ORDER BY created_at DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(rows))
}
