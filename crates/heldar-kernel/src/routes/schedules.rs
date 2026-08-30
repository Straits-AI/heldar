//! Per-camera recording-schedule CRUD (time-of-day windows).
//!
//! A `camera_schedules` row defines a recurring daily recording window; it takes effect only when
//! the camera's `record_mode` is `scheduled` or `scheduled_event`. `days` is a JSON array of weekday
//! ints (0=Mon..6=Sun); `time_start`/`time_end` are "HH:MM" 24h read in the CAMERA'S SITE timezone
//! (#125), falling back to the SERVER's local zone when no zone is configured anywhere
//! (start > end means an overnight window). Schedules are managed by manager+; any authenticated
//! principal can list them. The schedule watcher opens/closes windows in the background.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use serde_json::{json, Value};
use sqlx::types::Json as SqlxJson;
use uuid::Uuid;

use crate::auth::{self, Cap, Principal};
use crate::error::{AppError, AppResult};
use crate::models::{RecordSchedule, RecordScheduleCreate, RecordScheduleUpdate};
use crate::state::{AppState, CameraOwned};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/cameras/{id}/schedules",
            get(list_schedules).post(create_schedule),
        )
        .route(
            "/api/v1/schedules/{schedule_id}",
            axum::routing::patch(update_schedule).delete(delete_schedule),
        )
}

/// Validate a JSON `days` array: weekday ints 0..6 (0=Mon..6=Sun).
fn validate_days(v: &Value) -> AppResult<()> {
    let arr = v.as_array().ok_or_else(|| {
        AppError::BadRequest("`days` must be an array of weekday ints (0=Mon..6=Sun)".into())
    })?;
    // An empty `days` array is a degenerate schedule that silently never records; reject it rather than
    // store a window that looks active but does nothing.
    if arr.is_empty() {
        return Err(AppError::BadRequest(
            "`days` must include at least one weekday (0=Mon..6=Sun); an empty set never records"
                .into(),
        ));
    }
    for d in arr {
        match d.as_i64() {
            Some(n) if (0..7).contains(&n) => {}
            _ => {
                return Err(AppError::BadRequest(
                    "`days` entries must be integers 0..6 (0=Mon..6=Sun)".into(),
                ))
            }
        }
    }
    Ok(())
}

/// Reject a zero-length recording window (`time_start == time_end`), which the recorder treats as never
/// active — an operator who wants all-day should use `00:00`–`23:59`.
fn validate_window(time_start: &str, time_end: &str) -> AppResult<()> {
    if time_start == time_end {
        return Err(AppError::BadRequest(
            "`time_start` and `time_end` must differ (a zero-length window never records; use 00:00–23:59 for all day)".into(),
        ));
    }
    Ok(())
}

/// Resolve the camera owning a recording schedule, keeping this route's own 404 wording.
///
/// [`AppState::resource_camera`] names the resource with [`CameraOwned::noun`] ("schedule"), but this
/// route has always said "recording schedule". Only the NotFound arm is rewritten — and that arm is
/// reachable only for an UNSCOPED principal, so the rewrite is exactly what keeps constraint 2 (no
/// behaviour change for unscoped credentials) literally true down to the response body. The Forbidden
/// arm is passed through untouched so it stays byte-identical to every other resource-id refusal.
async fn schedule_camera(
    st: &AppState,
    principal: &Principal,
    schedule_id: &str,
    action: &str,
) -> AppResult<String> {
    st.resource_camera(principal, CameraOwned::Schedule, schedule_id, action)
        .await
        .map_err(|e| match e {
            AppError::NotFound(_) => {
                AppError::NotFound(format!("recording schedule {schedule_id} not found"))
            }
            other => other,
        })
}

/// Validate "HH:MM" 24h time and return its canonical zero-padded form.
fn normalize_hhmm(s: &str, field: &str) -> AppResult<String> {
    let (h, m) = s
        .split_once(':')
        .and_then(|(h, m)| Some((h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().ok()?)))
        .filter(|(h, m)| *h < 24 && *m < 60)
        .ok_or_else(|| AppError::BadRequest(format!("`{field}` must be HH:MM 24h time")))?;
    Ok(format!("{h:02}:{m:02}"))
}

async fn list_schedules(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<Vec<RecordSchedule>>> {
    principal.require_cap(Cap::CameraRead, "list recording schedules")?;
    let _ = st.camera_for(&principal, &id).await?;
    let rows = sqlx::query_as::<_, RecordSchedule>(
        "SELECT * FROM camera_schedules WHERE camera_id = ? ORDER BY created_at ASC",
    )
    .bind(&id)
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(rows))
}

async fn create_schedule(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(body): Json<RecordScheduleCreate>,
) -> AppResult<(StatusCode, Json<RecordSchedule>)> {
    principal.require(
        principal.can_manage_registry(),
        "create recording schedules",
    )?;
    let _ = st.camera_for(&principal, &id).await?;
    validate_days(&body.days)?;
    let time_start = normalize_hhmm(&body.time_start, "time_start")?;
    let time_end = normalize_hhmm(&body.time_end, "time_end")?;
    validate_window(&time_start, &time_end)?;
    let enabled = body.enabled.unwrap_or(true);
    let now = Utc::now();
    let schedule_id = format!("recsch_{}", Uuid::new_v4().simple());

    sqlx::query(
        "INSERT INTO camera_schedules
           (id, camera_id, days, time_start, time_end, enabled, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&schedule_id)
    .bind(&id)
    .bind(SqlxJson(body.days))
    .bind(&time_start)
    .bind(&time_end)
    .bind(enabled)
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await?;

    let schedule =
        sqlx::query_as::<_, RecordSchedule>("SELECT * FROM camera_schedules WHERE id = ?")
            .bind(&schedule_id)
            .fetch_one(&st.pool)
            .await?;
    // Apply immediately (e.g. a window that is active right now should start the recorder).
    st.recorder.reconcile(&id).await;
    auth::audit(
        &st.pool,
        &principal,
        "create_record_schedule",
        "camera_schedule",
        &schedule_id,
        json!({ "camera_id": &id, "time_start": &time_start, "time_end": &time_end, "enabled": enabled }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(schedule)))
}

async fn update_schedule(
    State(st): State<AppState>,
    Path(schedule_id): Path<String>,
    principal: Principal,
    Json(body): Json<RecordScheduleUpdate>,
) -> AppResult<Json<RecordSchedule>> {
    principal.require(
        principal.can_manage_registry(),
        "update recording schedules",
    )?;
    // Disabling a schedule stops a camera recording, so resolve the owning camera before the row is
    // disclosed — the reconcile at the end of this handler drives that camera's recorder.
    let _ = schedule_camera(&st, &principal, &schedule_id, "update recording schedules").await?;
    let cur = sqlx::query_as::<_, RecordSchedule>("SELECT * FROM camera_schedules WHERE id = ?")
        .bind(&schedule_id)
        .fetch_optional(&st.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("recording schedule {schedule_id} not found")))?;

    let days = match body.days {
        Some(d) => {
            validate_days(&d)?;
            SqlxJson(d)
        }
        None => SqlxJson(cur.days.0.clone()),
    };
    let time_start = match body.time_start {
        Some(s) => normalize_hhmm(&s, "time_start")?,
        None => cur.time_start.clone(),
    };
    let time_end = match body.time_end {
        Some(s) => normalize_hhmm(&s, "time_end")?,
        None => cur.time_end.clone(),
    };
    validate_window(&time_start, &time_end)?;
    let enabled = body.enabled.unwrap_or(cur.enabled);

    sqlx::query(
        "UPDATE camera_schedules SET days = ?, time_start = ?, time_end = ?, enabled = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(days)
    .bind(&time_start)
    .bind(&time_end)
    .bind(enabled)
    .bind(Utc::now())
    .bind(&schedule_id)
    .execute(&st.pool)
    .await?;

    let schedule =
        sqlx::query_as::<_, RecordSchedule>("SELECT * FROM camera_schedules WHERE id = ?")
            .bind(&schedule_id)
            .fetch_one(&st.pool)
            .await?;
    st.recorder.reconcile(&cur.camera_id).await;
    auth::audit(
        &st.pool,
        &principal,
        "update_record_schedule",
        "camera_schedule",
        &schedule_id,
        json!({ "time_start": &time_start, "time_end": &time_end, "enabled": enabled }),
    )
    .await;
    Ok(Json(schedule))
}

async fn delete_schedule(
    State(st): State<AppState>,
    Path(schedule_id): Path<String>,
    principal: Principal,
) -> AppResult<StatusCode> {
    principal.require(
        principal.can_manage_registry(),
        "delete recording schedules",
    )?;
    // This replaces the probe that used to live here: it runs the same `SELECT camera_id … WHERE id = ?`
    // but refuses an out-of-scope schedule with the SAME error as a missing one, so the 404 can no
    // longer be used to enumerate the box's schedule ids.
    let camera_id =
        schedule_camera(&st, &principal, &schedule_id, "delete recording schedules").await?;
    sqlx::query("DELETE FROM camera_schedules WHERE id = ?")
        .bind(&schedule_id)
        .execute(&st.pool)
        .await?;
    st.recorder.reconcile(&camera_id).await;
    auth::audit(
        &st.pool,
        &principal,
        "delete_record_schedule",
        "camera_schedule",
        &schedule_id,
        json!({ "camera_id": &camera_id }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
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

    async fn seed(pool: &sqlx::SqlitePool, camera_id: &str, schedule_id: &str) {
        let now = Utc::now();
        sqlx::query("INSERT INTO cameras (id, name, created_at, updated_at) VALUES (?,?,?,?)")
            .bind(camera_id)
            .bind(camera_id)
            .bind(now)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO camera_schedules
               (id, camera_id, days, time_start, time_end, enabled, created_at, updated_at)
             VALUES (?,?,'[0,1,2,3,4]','08:00','18:00',1,?,?)",
        )
        .bind(schedule_id)
        .bind(camera_id)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
    }

    fn disable() -> RecordScheduleUpdate {
        serde_json::from_value(json!({ "enabled": false })).unwrap()
    }

    /// Disabling another camera's recording window is a targeted recording denial. Refuse it, and make
    /// the refusal indistinguishable from a schedule id that was never on the box.
    #[tokio::test]
    async fn a_scoped_key_cannot_disable_or_delete_another_cameras_schedule() {
        let st = test_state().await;
        seed(&st.pool, "cam_a", "recsch_a").await;
        seed(&st.pool, "cam_sentinel_b", "recsch_b").await;
        let p = scoped(&["cam_a"]);

        let out_of_scope = update_schedule(
            State(st.clone()),
            Path("recsch_b".into()),
            p.clone(),
            Json(disable()),
        )
        .await
        .unwrap_err();
        let nonexistent = update_schedule(
            State(st.clone()),
            Path("recsch_zzz".into()),
            p.clone(),
            Json(disable()),
        )
        .await
        .unwrap_err();
        assert!(matches!(out_of_scope, AppError::Forbidden(_)));
        assert_eq!(out_of_scope.to_string(), nonexistent.to_string());
        assert!(!out_of_scope.to_string().contains("cam_sentinel_b"));

        // Same property on DELETE. The two refusals are compared WITHIN the route: the `action` clause
        // differs between update and delete by design (it says what was attempted, which the caller
        // already knows), while the part that would leak — owner, resource id, existence — does not.
        let del_out_of_scope =
            delete_schedule(State(st.clone()), Path("recsch_b".into()), p.clone())
                .await
                .unwrap_err();
        let del_nonexistent =
            delete_schedule(State(st.clone()), Path("recsch_zzz".into()), p.clone())
                .await
                .unwrap_err();
        assert!(matches!(del_out_of_scope, AppError::Forbidden(_)));
        assert_eq!(del_out_of_scope.to_string(), del_nonexistent.to_string());
        assert!(!del_out_of_scope.to_string().contains("cam_sentinel_b"));

        let still: i64 =
            sqlx::query_scalar("SELECT enabled FROM camera_schedules WHERE id = 'recsch_b'")
                .fetch_one(&st.pool)
                .await
                .unwrap();
        assert_eq!(still, 1, "the window is still recording");

        // Its own camera is untouched by the new check.
        assert!(update_schedule(
            State(st.clone()),
            Path("recsch_a".into()),
            p,
            Json(disable()),
        )
        .await
        .is_ok());
    }

    /// Constraint 2, to the byte: an unscoped principal still gets THIS route's own 404 wording, not
    /// the generic `CameraOwned::noun()` one the shared loader would otherwise produce.
    #[tokio::test]
    async fn an_unscoped_principal_keeps_the_original_404_wording() {
        let st = test_state().await;
        match delete_schedule(
            State(st.clone()),
            Path("recsch_zzz".into()),
            Principal::system_admin(),
        )
        .await
        .unwrap_err()
        {
            AppError::NotFound(m) => assert_eq!(m, "recording schedule recsch_zzz not found"),
            other => panic!("expected the pre-existing 404, got {other:?}"),
        }
    }
}
