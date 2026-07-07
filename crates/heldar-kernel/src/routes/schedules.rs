//! Per-camera recording-schedule CRUD (time-of-day windows).
//!
//! A `camera_schedules` row defines a recurring daily recording window; it takes effect only when
//! the camera's `record_mode` is `scheduled` or `scheduled_event`. `days` is a JSON array of weekday
//! ints (0=Mon..6=Sun); `time_start`/`time_end` are "HH:MM" 24h in the SERVER's LOCAL timezone
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

use crate::auth::{self, Principal};
use crate::error::{AppError, AppResult};
use crate::models::{RecordSchedule, RecordScheduleCreate, RecordScheduleUpdate};
use crate::routes::cameras::load_camera;
use crate::state::AppState;

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
    principal.require(principal.can_view(), "list recording schedules")?;
    let _ = load_camera(&st.pool, &id).await?;
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
    let _ = load_camera(&st.pool, &id).await?;
    validate_days(&body.days)?;
    let time_start = normalize_hhmm(&body.time_start, "time_start")?;
    let time_end = normalize_hhmm(&body.time_end, "time_end")?;
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
    let camera_id: Option<String> =
        sqlx::query_scalar("SELECT camera_id FROM camera_schedules WHERE id = ?")
            .bind(&schedule_id)
            .fetch_optional(&st.pool)
            .await?;
    let Some(camera_id) = camera_id else {
        return Err(AppError::NotFound(format!(
            "recording schedule {schedule_id} not found"
        )));
    };
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
