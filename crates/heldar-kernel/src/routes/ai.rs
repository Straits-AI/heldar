//! Stage 2 AI surface: AI task CRUD, the worker contract (discover tasks, pull the latest sampled
//! frame, post detections/events back), sampler status, and a detections query.

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::types::Json as SqlxJson;
use uuid::Uuid;

use crate::auth::{self, Principal};
use crate::error::{AppError, AppResult};
use crate::models::{AiIngest, AiTask, AiTaskCreate, AiTaskUpdate, Detection};
use crate::routes::cameras::load_camera;
use crate::services::sampler::SamplerInfo;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/cameras/{id}/ai-tasks",
            get(list_camera_tasks).post(create_task),
        )
        .route(
            "/api/v1/ai-tasks/{task_id}",
            axum::routing::patch(update_task).delete(delete_task),
        )
        .route("/api/v1/ai/tasks", get(list_all_tasks))
        .route("/api/v1/ai/samplers", get(sampler_status))
        // Bound the ingest body BEFORE deserialization so a hostile/buggy worker can't force a huge
        // allocation (the MAX_INGEST_DETECTIONS count check only runs after the body is fully parsed).
        // Generous headroom for MAX_INGEST_DETECTIONS rich detections; well under any real batch.
        .route(
            "/api/v1/ai/events",
            post(ingest).layer(DefaultBodyLimit::max(INGEST_BODY_LIMIT_BYTES)),
        )
        .route("/api/v1/cameras/{id}/frame", get(latest_frame))
        .route("/api/v1/cameras/{id}/detections", get(list_detections))
}

fn validate_profile(p: &str) -> AppResult<()> {
    if matches!(p, "sub" | "main") {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "`stream_profile` must be 'sub' or 'main'".into(),
        ))
    }
}

async fn list_camera_tasks(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<Vec<AiTask>>> {
    principal.require(principal.can_view(), "view AI tasks")?;
    let _ = load_camera(&st.pool, &id).await?;
    let tasks = sqlx::query_as::<_, AiTask>(
        "SELECT * FROM ai_tasks WHERE camera_id = ? ORDER BY created_at ASC",
    )
    .bind(&id)
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(tasks))
}

async fn create_task(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(body): Json<AiTaskCreate>,
) -> AppResult<(StatusCode, Json<AiTask>)> {
    principal.require(principal.can_manage_registry(), "create AI tasks")?;
    let _ = load_camera(&st.pool, &id).await?;
    if body.task_type.trim().is_empty() {
        return Err(AppError::BadRequest("`task_type` is required".into()));
    }
    let profile = body.stream_profile.unwrap_or_else(|| "sub".into());
    validate_profile(&profile)?;
    let fps = body.fps.unwrap_or(st.cfg.default_ai_fps).clamp(0.1, 30.0);
    let width = body
        .width
        .unwrap_or(st.cfg.default_ai_width)
        .clamp(160, 3840);
    let enabled = body.enabled.unwrap_or(true);
    let config = SqlxJson(body.config.unwrap_or_else(|| json!({})));

    // Idempotency: a camera has at most one task of a given type per stream profile. If one already
    // exists, return it instead of silently creating a duplicate — stacked-up identical detection
    // tasks (e.g. a provisioning script re-POSTing on every restart) waste inference. Change an
    // existing task via PATCH, not by re-creating it.
    if let Some(existing) = sqlx::query_as::<_, AiTask>(
        "SELECT * FROM ai_tasks WHERE camera_id = ? AND task_type = ? AND stream_profile = ?",
    )
    .bind(&id)
    .bind(&body.task_type)
    .bind(&profile)
    .fetch_optional(&st.pool)
    .await?
    {
        return Ok((StatusCode::OK, Json(existing)));
    }

    let now = Utc::now();
    let task_id = format!("ai_{}", Uuid::new_v4().simple());

    sqlx::query(
        "INSERT INTO ai_tasks
           (id, camera_id, task_type, enabled, stream_profile, fps, width, config, created_at, updated_at)
         VALUES (?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(&task_id)
    .bind(&id)
    .bind(&body.task_type)
    .bind(enabled)
    .bind(&profile)
    .bind(fps)
    .bind(width)
    .bind(config)
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await?;

    st.sampler.reconcile().await;
    let task = sqlx::query_as::<_, AiTask>("SELECT * FROM ai_tasks WHERE id = ?")
        .bind(&task_id)
        .fetch_one(&st.pool)
        .await?;
    auth::audit(
        &st.pool,
        &principal,
        "create_ai_task",
        "ai_task",
        &task_id,
        json!({ "camera_id": &id, "task_type": &task.task_type }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(task)))
}

async fn update_task(
    State(st): State<AppState>,
    Path(task_id): Path<String>,
    principal: Principal,
    Json(body): Json<AiTaskUpdate>,
) -> AppResult<Json<AiTask>> {
    principal.require(principal.can_manage_registry(), "update AI tasks")?;
    let cur = sqlx::query_as::<_, AiTask>("SELECT * FROM ai_tasks WHERE id = ?")
        .bind(&task_id)
        .fetch_optional(&st.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("ai task {task_id} not found")))?;

    let task_type = body.task_type.unwrap_or(cur.task_type);
    let profile = body.stream_profile.unwrap_or(cur.stream_profile);
    validate_profile(&profile)?;
    let fps = body.fps.map(|v| v.clamp(0.1, 30.0)).unwrap_or(cur.fps);
    let width = body.width.map(|v| v.clamp(160, 3840)).unwrap_or(cur.width);
    let enabled = body.enabled.unwrap_or(cur.enabled);
    let config = SqlxJson(body.config.unwrap_or(cur.config.0));

    sqlx::query(
        "UPDATE ai_tasks SET task_type=?, stream_profile=?, fps=?, width=?, enabled=?, config=?, updated_at=?
         WHERE id=?",
    )
    .bind(&task_type)
    .bind(&profile)
    .bind(fps)
    .bind(width)
    .bind(enabled)
    .bind(config)
    .bind(Utc::now())
    .bind(&task_id)
    .execute(&st.pool)
    .await?;

    st.sampler.reconcile().await;
    let task = sqlx::query_as::<_, AiTask>("SELECT * FROM ai_tasks WHERE id = ?")
        .bind(&task_id)
        .fetch_one(&st.pool)
        .await?;
    auth::audit(
        &st.pool,
        &principal,
        "update_ai_task",
        "ai_task",
        &task_id,
        json!({}),
    )
    .await;
    Ok(Json(task))
}

async fn delete_task(
    State(st): State<AppState>,
    Path(task_id): Path<String>,
    principal: Principal,
) -> AppResult<StatusCode> {
    principal.require(principal.can_manage_registry(), "delete AI tasks")?;
    let res = sqlx::query("DELETE FROM ai_tasks WHERE id = ?")
        .bind(&task_id)
        .execute(&st.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("ai task {task_id} not found")));
    }
    st.sampler.reconcile().await;
    auth::audit(
        &st.pool,
        &principal,
        "delete_ai_task",
        "ai_task",
        &task_id,
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
struct WorkerTask {
    id: String,
    camera_id: String,
    task_type: String,
    stream_profile: String,
    fps: f64,
    width: i64,
    config: Value,
    frame_url: String,
}

/// Worker discovery: every enabled AI task on an enabled camera, with the frame URL to pull.
async fn list_all_tasks(
    State(st): State<AppState>,
    principal: crate::auth::Principal,
) -> AppResult<Json<Vec<WorkerTask>>> {
    // Authentication floor: when auth is enabled this rejects anonymous callers (the worker sends an
    // integration API key). When auth is disabled the principal is the synthetic system admin.
    principal.require(principal.can_view(), "discover AI tasks")?;
    let tasks = sqlx::query_as::<_, AiTask>(
        "SELECT t.* FROM ai_tasks t JOIN cameras c ON c.id = t.camera_id
         WHERE t.enabled = 1 AND c.enabled = 1
         ORDER BY t.camera_id ASC",
    )
    .fetch_all(&st.pool)
    .await?;
    let out = tasks
        .into_iter()
        .map(|t| WorkerTask {
            frame_url: format!(
                "/api/v1/cameras/{}/frame?profile={}",
                t.camera_id, t.stream_profile
            ),
            id: t.id,
            camera_id: t.camera_id,
            task_type: t.task_type,
            stream_profile: t.stream_profile,
            fps: t.fps,
            width: t.width,
            config: t.config.0,
        })
        .collect();
    Ok(Json(out))
}

async fn sampler_status(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<Vec<SamplerInfo>>> {
    principal.require(principal.can_view(), "view sampler status")?;
    Ok(Json(st.sampler.statuses().await))
}

#[derive(Debug, Deserialize)]
struct FrameQuery {
    profile: Option<String>,
}

/// Serve the latest sampled frame for a camera + stream profile (the AI worker's input).
async fn latest_frame(
    State(st): State<AppState>,
    principal: crate::auth::Principal,
    Path(id): Path<String>,
    Query(q): Query<FrameQuery>,
) -> AppResult<Response> {
    // Authentication floor (a frame can contain faces/plates). Note: when auth is enabled the SPA's
    // <img> tags cannot send a bearer header — token-in-query / cookie for the media plane is handled
    // in the auth-split work; the worker authenticates via X-API-Key.
    principal.require(principal.can_view(), "read camera frames")?;
    // Defense in depth: the id becomes a path segment, so reject any separators/traversal.
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(AppError::BadRequest("invalid camera id".into()));
    }
    let profile = q.profile.unwrap_or_else(|| "sub".into());
    validate_profile(&profile)?;
    let path = st.sampler.frame_path(&id, &profile);
    let bytes = tokio::fs::read(&path).await.map_err(|_| {
        AppError::NotFound("no sampled frame yet (is an AI task enabled for this camera?)".into())
    })?;
    let captured = tokio::fs::metadata(&path)
        .await
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| chrono::DateTime::<Utc>::from_timestamp_millis(d.as_millis() as i64))
        })
        .flatten();
    let age_ms = captured
        .map(|c| (Utc::now() - c).num_milliseconds().max(0))
        .unwrap_or(0);

    Response::builder()
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-frame-age-ms", age_ms.to_string())
        .header(
            "x-frame-captured-at",
            captured.map(|c| c.to_rfc3339()).unwrap_or_default(),
        )
        .body(Body::from(bytes))
        .map_err(|e| AppError::Other(anyhow::anyhow!("building response: {e}")))
}

#[derive(Debug, Deserialize)]
struct DetectionQuery {
    from: Option<String>,
    to: Option<String>,
    label: Option<String>,
    limit: Option<i64>,
}

async fn list_detections(
    State(st): State<AppState>,
    principal: crate::auth::Principal,
    Path(id): Path<String>,
    Query(q): Query<DetectionQuery>,
) -> AppResult<Json<Vec<Detection>>> {
    principal.require(principal.can_view(), "read detections")?;
    let _ = load_camera(&st.pool, &id).await?;
    let limit = q.limit.unwrap_or(200).clamp(1, 5000);
    let from = parse_opt_ts(&q.from, "from")?;
    let to = parse_opt_ts(&q.to, "to")?;
    let rows = sqlx::query_as::<_, Detection>(
        "SELECT * FROM detections
         WHERE camera_id = ?
           AND (? IS NULL OR timestamp >= ?)
           AND (? IS NULL OR timestamp <= ?)
           AND (? IS NULL OR label = ?)
         ORDER BY timestamp DESC LIMIT ?",
    )
    .bind(&id)
    .bind(from)
    .bind(from)
    .bind(to)
    .bind(to)
    .bind(&q.label)
    .bind(&q.label)
    .bind(limit)
    .fetch_all(&st.pool)
    .await?;
    Ok(Json(rows))
}

/// Max detections accepted in a single ingest request (DoS / write-amplification bound).
const MAX_INGEST_DETECTIONS: usize = 1000;

/// Hard cap on the ingest request body, enforced by the framework BEFORE deserialization (defense
/// in depth vs the post-parse count guard). 8 MiB comfortably fits MAX_INGEST_DETECTIONS detections
/// with bounding boxes + attributes, while refusing a body crafted to exhaust memory.
const INGEST_BODY_LIMIT_BYTES: usize = 8 * 1024 * 1024;

/// Columns bound per detection row in the batched INSERT in [`ingest`].
const DETECTION_INSERT_COLS: usize = 11;
/// SQLite's compile-time bound-variable ceiling (SQLITE_MAX_VARIABLE_NUMBER). The batched insert is
/// chunked so a single statement never exceeds it, even at [`MAX_INGEST_DETECTIONS`].
const SQLITE_MAX_BIND_VARS: usize = 999;
/// Detection rows per INSERT statement (≈90), keeping bound variables under [`SQLITE_MAX_BIND_VARS`].
const DETECTION_INSERT_CHUNK: usize = SQLITE_MAX_BIND_VARS / DETECTION_INSERT_COLS;

/// Ingest detections (and an optional event) posted by an AI worker. Detections are written in a
/// single transaction so a batch is all-or-nothing.
async fn ingest(
    State(st): State<AppState>,
    principal: crate::auth::Principal,
    Json(body): Json<AiIngest>,
) -> AppResult<Json<Value>> {
    principal.require(principal.can_ingest(), "ingest perception events")?;
    let cam = load_camera(&st.pool, &body.camera_id).await?;
    if body.task_type.trim().is_empty() {
        return Err(AppError::BadRequest("`task_type` is required".into()));
    }
    if body.detections.len() > MAX_INGEST_DETECTIONS {
        return Err(AppError::BadRequest(format!(
            "too many detections in one request ({}); max {MAX_INGEST_DETECTIONS}",
            body.detections.len()
        )));
    }
    let ts = parse_opt_ts(&body.timestamp, "timestamp")?.unwrap_or_else(Utc::now);

    let mut inserted = 0u64;
    let mut tx = st.pool.begin().await?;
    // Idempotency + atomic capture: record the batch in the outbox FIRST, in the same transaction.
    // A duplicate (camera_id, frame_id) — i.e. an at-least-once redelivery — conflicts and inserts 0
    // rows; we then skip both the detection writes and the consumer fan-out, so a replayed batch can
    // never double-count ANPR votes or corrupt zone state. With no frame_id every batch is accepted.
    let outbox_res = sqlx::query(
        "INSERT INTO outbox (topic, camera_id, site_id, frame_id, task_type, detection_count, created_at)
         VALUES ('detections', ?, ?, ?, ?, ?, ?)
         ON CONFLICT DO NOTHING",
    )
    .bind(&body.camera_id)
    .bind(&cam.site_id)
    .bind(&body.frame_id)
    .bind(&body.task_type)
    .bind(body.detections.len() as i64)
    .bind(Utc::now())
    .execute(&mut *tx)
    .await?;
    if outbox_res.rows_affected() == 0 {
        // Duplicate frame already ingested — no-op (idempotent).
        tx.commit().await?;
        return Ok(Json(json!({ "detections_ingested": 0, "duplicate": true })));
    }
    // Batched multi-row insert: one INSERT per chunk instead of one statement per detection. Same
    // columns, values, and semantics as the prior per-row loop, still inside the transaction. We
    // chunk so a single statement's bound-variable count stays under SQLite's limit even at
    // MAX_INGEST_DETECTIONS.
    for chunk in body.detections.chunks(DETECTION_INSERT_CHUNK) {
        let tuples = vec!["(?,?,?,?,?,?,?,?,?,?,?)"; chunk.len()].join(",");
        let sql = format!(
            "INSERT INTO detections
               (id, camera_id, task_type, timestamp, label, confidence, bbox, track_id, attributes, frame_id, created_at)
             VALUES {tuples}"
        );
        let mut q = sqlx::query(&sql);
        for d in chunk {
            let bbox = d.bbox.clone().map(SqlxJson);
            let attrs = SqlxJson(d.attributes.clone().unwrap_or_else(|| json!({})));
            q = q
                .bind(format!("det_{}", Uuid::new_v4().simple()))
                .bind(&body.camera_id)
                .bind(&body.task_type)
                .bind(ts)
                .bind(&d.label)
                .bind(d.confidence)
                .bind(bbox)
                .bind(&d.track_id)
                .bind(attrs)
                .bind(&body.frame_id)
                .bind(Utc::now());
        }
        inserted += q.execute(&mut *tx).await?.rows_affected();
    }
    tx.commit().await?;

    // Fan the committed batch out to registered perception consumers (zones, ANPR/entry, future
    // apps). The kernel does not know or branch on which apps exist — each consumer self-selects by
    // task_type. Engines that need trustworthy timing use server time, not the worker timestamp.
    //
    // Durability: fan-out happens after commit, so a crash here would otherwise drop the consumer
    // notification. `fan_out` claims each (consumer, frame) at-most-once; on success we mark the
    // outbox batch fanned, and the `fanout` drainer replays any batch left un-fanned by a crash.
    let batch = crate::services::consumer::DetectionBatch {
        camera_id: &body.camera_id,
        site_id: cam.site_id.as_deref(),
        task_type: &body.task_type,
        detections: &body.detections,
        timestamp: ts,
    };
    let fanned = crate::services::consumer::fan_out(
        &st.pool,
        &st.consumers,
        &batch,
        body.frame_id.as_deref(),
    )
    .await;
    if fanned {
        if let Some(fid) = body.frame_id.as_deref() {
            let _ = sqlx::query(
                "UPDATE outbox SET fanned_out_at = ? \
                 WHERE topic = 'detections' AND camera_id = ? AND frame_id = ? AND fanned_out_at IS NULL",
            )
            .bind(Utc::now())
            .bind(&body.camera_id)
            .bind(fid)
            .execute(&st.pool)
            .await;
        }
    }

    if let Some(ev) = &body.event {
        let severity = ev.severity.clone().unwrap_or_else(|| "info".into());
        let payload = ev.payload.clone().unwrap_or_else(|| json!({}));
        crate::repo::log_event(
            &st.pool,
            Some(&body.camera_id),
            &ev.event_type,
            &severity,
            payload,
        )
        .await?;
    }

    Ok(Json(json!({ "detections_ingested": inserted })))
}

fn parse_opt_ts(s: &Option<String>, field: &str) -> AppResult<Option<chrono::DateTime<Utc>>> {
    match s {
        Some(v) => crate::util::parse_rfc3339(v)
            .map(Some)
            .ok_or_else(|| AppError::BadRequest(format!("invalid `{field}` timestamp"))),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_profile_accepts_sub_and_main() {
        assert!(validate_profile("sub").is_ok());
        assert!(validate_profile("main").is_ok());
    }

    #[test]
    fn validate_profile_rejects_other_values() {
        // Case-sensitive and whitespace-sensitive: only the exact lowercase tokens pass.
        for bad in ["", "Sub", "MAIN", " sub", "sub ", "substream", "foo"] {
            match validate_profile(bad) {
                Err(AppError::BadRequest(m)) => {
                    assert!(
                        m.contains("stream_profile"),
                        "unexpected message for {bad:?}: {m}"
                    );
                }
                other => panic!("expected BadRequest for {bad:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn parse_opt_ts_none_is_ok_none() {
        let out = parse_opt_ts(&None, "from").unwrap();
        assert!(out.is_none());
    }

    #[test]
    fn parse_opt_ts_valid_matches_util() {
        let raw = "2026-06-13T05:02:19Z".to_string();
        let parsed = parse_opt_ts(&Some(raw.clone()), "from").unwrap();
        // parse_opt_ts is a thin wrapper over crate::util::parse_rfc3339: anchor to it.
        assert_eq!(parsed, crate::util::parse_rfc3339(&raw));
        assert_eq!(parsed.unwrap().to_rfc3339(), "2026-06-13T05:02:19+00:00");
    }

    #[test]
    fn parse_opt_ts_invalid_reports_field() {
        match parse_opt_ts(&Some("not-a-timestamp".to_string()), "to") {
            Err(AppError::BadRequest(m)) => {
                assert!(m.contains("to"), "message should name the field: {m}");
                assert!(m.contains("timestamp"), "message: {m}");
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn max_ingest_detections_bound_is_stable() {
        assert_eq!(MAX_INGEST_DETECTIONS, 1000);
    }

    async fn test_state() -> AppState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let cfg = std::sync::Arc::new(crate::config::Config::from_env());
        AppState {
            recorder: crate::services::recorder::RecorderManager::new(pool.clone(), cfg.clone()),
            sampler: crate::services::sampler::SamplerManager::new(pool.clone(), cfg.clone()),
            mirror: None,
            consumers: std::sync::Arc::new(Vec::new()),
            modules: std::sync::Arc::new(Vec::new()),
            catalog: std::sync::Arc::new(crate::services::registry::CatalogService::new(&cfg)),
            http: reqwest::Client::new(),
            started_at: chrono::Utc::now(),
            pool,
            cfg,
        }
    }

    /// Re-creating the same task type on the same camera+profile returns the existing task (200), never
    /// a duplicate — but a different stream profile is a distinct task.
    #[tokio::test]
    async fn create_ai_task_is_idempotent_per_slot() {
        let st = test_state().await;
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO cameras (id, name, enabled, created_at, updated_at) VALUES (?,?,?,?,?)",
        )
        .bind("cam_x")
        .bind("cam_x")
        .bind(1)
        .bind(now)
        .bind(now)
        .execute(&st.pool)
        .await
        .unwrap();
        let body = || AiTaskCreate {
            task_type: "detection".into(),
            stream_profile: Some("sub".into()),
            fps: Some(2.0),
            width: Some(640),
            config: None,
            enabled: Some(true),
        };
        let mk = |b| {
            create_task(
                State(st.clone()),
                Path("cam_x".into()),
                Principal::system_admin(),
                Json(b),
            )
        };

        let (s1, Json(t1)) = mk(body()).await.unwrap();
        let (s2, Json(t2)) = mk(body()).await.unwrap();
        assert_eq!(s1, StatusCode::CREATED);
        assert_eq!(s2, StatusCode::OK, "re-create returns the existing task");
        assert_eq!(t1.id, t2.id, "no duplicate task created");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ai_tasks")
            .fetch_one(&st.pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "still exactly one task");

        // a different stream profile is a distinct slot → a new task
        let mut other = body();
        other.stream_profile = Some("main".into());
        let (s3, _) = mk(other).await.unwrap();
        assert_eq!(
            s3,
            StatusCode::CREATED,
            "a different profile is a separate task"
        );
    }
}
