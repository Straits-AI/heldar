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

use crate::auth::{self, Cap, Principal, PrincipalKind};
use crate::config::EnforcementTier;
use crate::error::{AppError, AppResult};
use crate::models::{AiIngest, AiTask, AiTaskCreate, AiTaskUpdate, Detection, Provenance};
use crate::services::sampler::SamplerInfo;
use crate::services::{ai_leases, frame_ticket};
use crate::state::{camera_scope_filter, AppState, CameraOwned};

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
        // Task LEASES (acquire/renew are one call). `GET /ai/tasks` above is deliberately unchanged,
        // so an old worker and every existing script keep working exactly as before.
        .route("/api/v1/ai/leases", post(acquire_lease))
        .route(
            "/api/v1/ai/leases/{lease_id}",
            axum::routing::delete(release_lease),
        )
        .route("/api/v1/ai/samplers", get(sampler_status))
        // Bound the ingest body BEFORE deserialization so a hostile/buggy worker can't force a huge
        // allocation (the MAX_INGEST_DETECTIONS count check only runs after the body is fully parsed).
        // Generous headroom for MAX_INGEST_DETECTIONS rich detections; well under any real batch.
        .route(
            "/api/v1/ai/events",
            post(ingest).layer(DefaultBodyLimit::max(INGEST_BODY_LIMIT_BYTES)),
        )
        // Semantic-retrieval worker surface (issue #38): batched crop-embedding ingest, plus the
        // pull-only query-embedding queue (fast claim poll + result post). Embedding batches carry
        // f32 vectors and optional JPEG thumbs, so this route gets its own, larger body cap.
        .route(
            "/api/v1/ai/embeddings",
            post(ingest_embeddings).layer(DefaultBodyLimit::max(EMBED_INGEST_BODY_LIMIT_BYTES)),
        )
        .route("/api/v1/ai/embed-queries", get(claim_embed_queries))
        .route(
            "/api/v1/ai/embed-queries/{id}/result",
            post(embed_query_result).layer(DefaultBodyLimit::max(EMBED_RESULT_BODY_LIMIT_BYTES)),
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
    principal.require_cap(Cap::AiTasks, "view AI tasks")?;
    // `camera_for`, not the raw loader: scope BEFORE existence, so a camera this credential does not
    // hold answers 403 whether or not it is on the box.
    let _ = st.camera_for(&principal, &id).await?;
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
    // Scope first: creating a task here spends decode budget on the camera and enrolls it with the
    // sampler, so an out-of-scope camera must be refused before anything is written.
    let _ = st.camera_for(&principal, &id).await?;
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
    // Resolve the OWNING camera before the row is disclosed. A task addressed by its own id cannot use
    // `camera_for` (there is no camera id in the path yet), and `require_camera(&cur.camera_id, …)`
    // would answer 404 for a missing task and 403 for another camera's — an id-space oracle.
    let _ = st
        .resource_camera(&principal, CameraOwned::AiTask, &task_id, "update AI tasks")
        .await?;
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
    // Before the DELETE, so the 204-vs-404 shape below can no longer be probed for another camera's
    // task ids (deleting one is a targeted perception denial, and it also reconciles the sampler).
    let _ = st
        .resource_camera(&principal, CameraOwned::AiTask, &task_id, "delete AI tasks")
        .await?;
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
/// A worker whose `last_seen` is older than this is treated as gone and its tasks reassigned. Workers
/// must poll `/ai/tasks` more often than this (the default poll interval is 10s), so a live worker is
/// never dropped between polls; a crashed one is reclaimed within the TTL.
///
/// Re-exported from the shard module rather than redeclared: pruning here and the liveness filter used
/// when leasing must use the SAME cutoff, or a worker can be live for one and gone for the other.
use crate::services::worker_shard::WORKER_LIVENESS_TTL_SECS;

#[derive(Debug, Deserialize)]
struct TasksQuery {
    /// Stable identity of the polling worker process. When present, the kernel shards the task set so
    /// multiple workers on one node split the load; when absent (a single/legacy worker) it returns all.
    worker_id: Option<String>,
}

/// Which of `total` stably-ordered tasks belong to `me`, given the stably-ordered `live` worker set.
/// Task sharding lives in [`crate::services::worker_shard`] so this path and lease acquisition cannot
/// drift apart — when they did, discovery sharded while leasing was greedy, and a fleet collapsed to
/// one active worker.
use crate::services::worker_shard::assign as worker_shard;

async fn list_all_tasks(
    State(st): State<AppState>,
    Query(q): Query<TasksQuery>,
    principal: crate::auth::Principal,
) -> AppResult<Json<Vec<WorkerTask>>> {
    // Authentication floor: when auth is enabled this rejects anonymous callers (the worker sends an
    // integration API key). When auth is disabled the principal is the synthetic system admin.
    principal.require_cap(Cap::AiTasks, "discover AI tasks")?;
    // Stable order (by id) so every worker sees the same task sequence and the modulo shard agrees.
    //
    // The scope filter is `None` for every unscoped credential (every human role, every key minted
    // without a camera list), so this is byte-identical to the previous query for them. A camera-scoped
    // credential sees only its own cameras' tasks — otherwise this route hands out the whole roster,
    // which is the input every camera-keyed route needs.
    let scope = camera_scope_filter(&principal, "t.camera_id");
    let mut sql = String::from(
        "SELECT t.* FROM ai_tasks t JOIN cameras c ON c.id = t.camera_id
         WHERE t.enabled = 1 AND c.enabled = 1",
    );
    if let Some((pred, _)) = &scope {
        sql.push_str(pred);
    }
    sql.push_str(" ORDER BY t.id ASC");
    let mut query = sqlx::query_as::<_, AiTask>(&sql);
    if let Some((_, binds)) = &scope {
        // Bind from the RETURNED vector, never from `camera_scope()`: the empty-allowlist arm is
        // `" AND 0"` with zero binds, and iterating the scope instead would desync the parameters.
        for b in binds {
            query = query.bind(b);
        }
    }
    let tasks = query.fetch_all(&st.pool).await?;

    // Multi-worker sharding: an identified worker heartbeats itself, stale workers are pruned, and this
    // worker gets only its slice of the tasks. No worker_id → return everything (backward-compatible in
    // both directions: an old worker gets all tasks; a new worker against an old kernel is unaffected).
    let keep: Vec<usize> = match q
        .worker_id
        .as_deref()
        .map(str::trim)
        .filter(|w| !w.is_empty())
    {
        None => (0..tasks.len()).collect(),
        Some(worker_id) => {
            let now = Utc::now();
            // Bind the worker id to the CREDENTIAL that registered it (graft G4). Before this, any
            // principal could heartbeat any `worker_id`: registering the real worker's id from a
            // second process collapsed its shard, and modulo sharding then handed that worker only
            // half the tasks — a perception denial available to anyone holding a read credential.
            //
            // `IS` is SQLite's null-safe equality, so a row written before 0012 (api_key_id NULL) is
            // adoptable exactly once, by whoever heartbeats it next.
            let claimed = sqlx::query(
                "INSERT INTO ai_workers (worker_id, last_seen, api_key_id) VALUES (?, ?, ?)
                 ON CONFLICT(worker_id) DO UPDATE SET
                     last_seen = excluded.last_seen,
                     api_key_id = excluded.api_key_id
                 WHERE ai_workers.api_key_id IS excluded.api_key_id
                    OR ai_workers.api_key_id IS NULL",
            )
            .bind(worker_id)
            .bind(now)
            .bind(credential_id(&principal))
            .execute(&st.pool)
            .await
            .map(|r| r.rows_affected() > 0)
            .unwrap_or(false);
            if !claimed {
                if st.cfg.machine_auth == EnforcementTier::Enforce {
                    return Err(AppError::Conflict(format!(
                        "worker_id `{worker_id}` is registered to another credential"
                    )));
                }
                tracing::warn!(
                    target: "heldar::security",
                    worker_id, credential = %credential_id(&principal),
                    "ai: worker_id is registered to another credential; allowing under \
                     HELDAR_MACHINE_AUTH={} (it would be a 409 under `enforce`)",
                    st.cfg.machine_auth.as_str()
                );
            }
            let _ = sqlx::query("DELETE FROM ai_workers WHERE last_seen < ?")
                .bind(now - chrono::Duration::seconds(WORKER_LIVENESS_TTL_SECS))
                .execute(&st.pool)
                .await;
            // Partitioned by CREDENTIAL, matching `ai_leases::acquire`. `tasks` here is already
            // camera-filtered, so dividing it by every worker on the box counts workers that are
            // dividing a different list — discovery and leasing then disagree about who owns what,
            // and the comment in `ai_leases` claiming they agree was simply wrong. Same partition on
            // both sides is what makes that comment true.
            let live: Vec<String> = sqlx::query_scalar(
                "SELECT worker_id FROM ai_workers WHERE api_key_id IS ? ORDER BY worker_id ASC",
            )
            .bind(credential_id(&principal))
            .fetch_all(&st.pool)
            .await
            .unwrap_or_default();
            let live = if live.iter().any(|w| w == worker_id) {
                live
            } else {
                let mut v = live;
                v.push(worker_id.to_string());
                v.sort();
                v
            };
            worker_shard(tasks.len(), &live, worker_id)
        }
    };

    let keep: std::collections::HashSet<usize> = keep.into_iter().collect();
    let out = tasks
        .into_iter()
        .enumerate()
        .filter(|(i, _)| keep.contains(i))
        .map(|(_, t)| worker_task(t))
        .collect();
    Ok(Json(out))
}

/// Render one task for the worker contract.
///
/// `frame_url` now carries `&task=<id>`: that is what makes the frame endpoint mint an
/// `x-frame-ticket` for a lease-holding caller. It is harmless for a worker that has no lease (no
/// ticket is emitted) and for the dashboard (which never passes `task`), so `GET /ai/tasks` stays
/// backward-compatible in both directions.
fn worker_task(t: AiTask) -> WorkerTask {
    WorkerTask {
        frame_url: format!(
            "/api/v1/cameras/{}/frame?profile={}&task={}",
            t.camera_id, t.stream_profile, t.id
        ),
        id: t.id,
        camera_id: t.camera_id,
        task_type: t.task_type,
        stream_profile: t.stream_profile,
        fps: t.fps,
        width: t.width,
        config: t.config.0,
    }
}

/// The credential a machine-surface row is attributed to.
///
/// `'system'` for the synthetic principal used when auth is disabled, so the LAN-appliance default
/// behaves uniformly (leases, worker bindings and tickets all resolve against one stable id) instead of
/// being a special case at every call site.
fn credential_id(principal: &Principal) -> String {
    match principal.kind {
        PrincipalKind::System => "system".to_string(),
        _ => principal.id.clone(),
    }
}

#[derive(Debug, Deserialize, Default)]
struct LeaseRequest {
    worker_id: String,
    /// Restrict the lease to these task types (default: any).
    task_types: Option<Vec<String>>,
    /// Cap on how many tasks to take in one call (default: all eligible).
    max_tasks: Option<i64>,
    /// Requested lease lifetime; clamped to 15..=300 s.
    ttl_secs: Option<i64>,
}

#[derive(Debug, Serialize)]
struct LeaseResponse {
    lease_id: String,
    worker_id: String,
    expires_at: String,
    tasks: Vec<WorkerTask>,
}

/// Acquire — or renew, they are the SAME call — a lease over eligible AI tasks.
///
/// A lease is what makes a subsequent frame pull ticketable, and a ticket is what makes an ingest
/// attributable. A worker's poll loop therefore needs no state machine: call this every tick, analyze
/// whatever comes back.
async fn acquire_lease(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<LeaseRequest>,
) -> AppResult<Json<LeaseResponse>> {
    principal.require_cap(Cap::AiTasks, "lease AI tasks")?;
    let worker_id = body.worker_id.trim();
    if worker_id.is_empty() {
        return Err(AppError::BadRequest("`worker_id` is required".into()));
    }
    let ttl = ai_leases::clamp_ttl(body.ttl_secs);
    let acquired = ai_leases::acquire(
        &st.pool,
        &credential_id(&principal),
        worker_id,
        body.task_types.as_deref(),
        body.max_tasks,
        ttl,
        principal.camera_scope(),
    )
    .await?;
    Ok(Json(LeaseResponse {
        lease_id: acquired.lease_id,
        worker_id: worker_id.to_string(),
        expires_at: acquired.expires_at.to_rfc3339(),
        tasks: acquired.tasks.into_iter().map(worker_task).collect(),
    }))
}

/// Release a lease early (graceful worker shutdown), freeing its tasks immediately instead of after
/// the TTL. Scoped to the holding credential, so a lease id is not a capability on its own.
async fn release_lease(
    State(st): State<AppState>,
    principal: Principal,
    Path(lease_id): Path<String>,
) -> AppResult<Json<Value>> {
    principal.require_cap(Cap::AiTasks, "release AI task leases")?;
    let released = ai_leases::release(&st.pool, &lease_id, &credential_id(&principal)).await?;
    Ok(Json(json!({ "released": released })))
}

async fn sampler_status(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<Vec<SamplerInfo>>> {
    principal.require_cap(Cap::AiTasks, "view sampler status")?;
    // `SamplerInfo` leads with `camera_id`, so this collection is a camera roster in disguise. Not a
    // SQL list — the statuses come from the in-process sampler — so it is post-filtered. `camera_allowed`
    // is `true` for every unscoped credential, making this a no-op retain for them.
    let mut infos = st.sampler.statuses().await;
    infos.retain(|s| principal.camera_allowed(&s.camera_id));
    Ok(Json(infos))
}

#[derive(Debug, Deserialize)]
struct FrameQuery {
    profile: Option<String>,
    /// AI task this pull is for. When present AND the caller holds a live lease on that task, the
    /// response carries an `x-frame-ticket`. Absent (the dashboard) → no header, nothing changes.
    task: Option<String>,
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
    principal.require_cap(Cap::AiFrames, "read camera frames")?;
    principal.require_camera(&id, "read frames from this camera")?;
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

    // Ticket issuance. Every failure mode here — no `task`, no lease, a lease on another camera, no
    // capture time, a lease-table error — degrades to "no ticket emitted", never to a failed frame
    // pull. That is what keeps a lease problem from becoming a perception outage under the default
    // `warn` tier (and keeps it entirely off the recorder's path).
    let mut ticket: Option<String> = None;
    if let (Some(task_id), Some(captured_at)) = (q.task.as_deref().map(str::trim), captured) {
        if !task_id.is_empty() {
            let key_id = credential_id(&principal);
            match ai_leases::is_live(&st.pool, task_id, &key_id).await {
                Ok(Some(lease)) if lease.camera_id == id => {
                    ticket = frame_ticket::mint(
                        &key_id,
                        &id,
                        task_id,
                        captured_at.timestamp_millis(),
                        Utc::now().timestamp(),
                        st.cfg.frame_ticket_ttl_secs,
                    );
                }
                Ok(_) => {
                    tracing::debug!(
                        camera_id = %id, task_id,
                        "ai: no live lease for this task/credential; serving the frame without a ticket"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e, camera_id = %id, task_id,
                        "ai: lease lookup failed; serving the frame without a ticket"
                    );
                }
            }
        }
    }

    let mut resp = Response::builder()
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-frame-age-ms", age_ms.to_string())
        .header(
            "x-frame-captured-at",
            captured.map(|c| c.to_rfc3339()).unwrap_or_default(),
        );
    if let Some(t) = ticket {
        resp = resp.header("x-frame-ticket", t);
    }
    resp.body(Body::from(bytes))
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
    principal.require_cap(Cap::EventsRead, "read detections")?;
    let _ = st.camera_for(&principal, &id).await?;
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

/// Hard cap on the ingest request body, enforced by the framework BEFORE deserialization (defense
/// in depth vs the post-parse count guard). 8 MiB comfortably fits MAX_INGEST_DETECTIONS detections
/// with bounding boxes + attributes, while refusing a body crafted to exhaust memory.
const INGEST_BODY_LIMIT_BYTES: usize = 8 * 1024 * 1024;

/// Ingest detections (and an optional event) posted by an AI worker. The persist + fan-out
/// contract (outbox idempotency, all-or-nothing transaction, durable consumer fan-out) lives in
/// [`crate::services::perception_ingest`], shared with kernel-internal producers such as the
/// camera-native ANPR poller; this handler owns only the HTTP/RBAC surface.
async fn ingest(
    State(st): State<AppState>,
    principal: crate::auth::Principal,
    Json(body): Json<AiIngest>,
) -> AppResult<Json<Value>> {
    principal.require_cap(Cap::AiIngest, "ingest perception events")?;
    let bound = resolve_binding(
        &st,
        &principal,
        body.frame_ticket.as_deref(),
        Some(&body.camera_id),
        Some(&body.task_type),
        body.frame_id.as_deref(),
    )
    .await?;

    // Under a ticket, camera_id / task_type / frame_id are SERVER-DERIVED. That is what kills the
    // suppression primitive: `idx_outbox_dedup(camera_id, frame_id)` is first-writer-wins, and a
    // client can no longer name a frame it never held a ticket for.
    let derived = AiIngest {
        camera_id: bound.camera_id.clone(),
        task_type: bound.task_type.clone(),
        timestamp: body.timestamp.clone(),
        frame_id: bound.frame_id.clone(),
        detections: body.detections,
        event: body.event,
        frame_ticket: None,
    };
    let outcome =
        crate::services::perception_ingest::ingest_batch(&st, &derived, &bound.provenance).await?;
    // `ticketed` is echoed so a worker can tell — without parsing the ticket or knowing the box's
    // tier — whether the batch it just posted was bound to a server-issued frame. Under `warn` a
    // worker that expected to be ticketed but reads `false` knows its lease lapsed.
    if outcome.duplicate {
        return Ok(Json(json!({
            "detections_ingested": 0,
            "duplicate": true,
            "ticketed": bound.ticketed,
        })));
    }
    Ok(Json(json!({
        "detections_ingested": outcome.inserted,
        "ticketed": bound.ticketed,
    })))
}

/// What a verified (or absent) frame ticket resolves to: the values the kernel will actually use.
#[derive(Debug)]
struct IngestBinding {
    camera_id: String,
    task_type: String,
    frame_id: Option<String>,
    provenance: Provenance,
    /// True when a ticket was verified — the caller is speaking about a frame it was handed.
    ticketed: bool,
}

/// Resolve the ingest binding for one request: verify the frame ticket (if any) and decide what the
/// kernel trusts.
///
/// Five steps, in this order, under `HELDAR_INGEST_PROVENANCE=enforce`:
///   1. a ticket must be present (401 `frame_ticket_required`);
///   2. it must parse, and its task must hold a LIVE lease for the CALLING credential (401);
///   3. its signature must verify over that credential and the lease's camera (401) — so a leaked
///      ticket is inert for any other key;
///   4. it must not be expired (401);
///   5. the task and its camera must still be enabled (403).
///
/// Then `camera_id` / `task_type` / `frame_id` are taken from the ticket, and any value the body still
/// carries must AGREE (409) rather than being trusted.
///
/// Under `warn` / `off` a ticketless batch is accepted exactly as today, with a rate-limited log and an
/// `ingest_unleased` event so an operator can see who would break before flipping the switch.
///
/// "Exactly as today" INCLUDES the client's own `frame_id`. That is not cosmetic: `frame_id` is the
/// idempotency key behind `idx_outbox_dedup(camera_id, frame_id)`, and dropping it would make an
/// at-least-once redelivery insert a second time — re-firing consumer side effects and, on the entry
/// path, adding a second ANPR vote toward `min_votes` for a frame that was only ever seen once.
/// A ticketless batch therefore keeps its `frame_id`; only a TICKETED batch has it server-derived.
async fn resolve_binding(
    st: &AppState,
    principal: &Principal,
    ticket: Option<&str>,
    body_camera: Option<&str>,
    body_task_type: Option<&str>,
    body_frame_id: Option<&str>,
) -> AppResult<IngestBinding> {
    let key_id = credential_id(principal);
    let enforce = st.cfg.ingest_provenance == EnforcementTier::Enforce;
    let raw = ticket.map(str::trim).filter(|t| !t.is_empty());

    let Some(raw) = raw else {
        if enforce {
            return Err(AppError::Unauthorized(
                "frame_ticket_required: post the `x-frame-ticket` returned by \
                 GET /api/v1/cameras/{id}/frame?task=<ai_task_id> (acquire a lease first via \
                 POST /api/v1/ai/leases)"
                    .into(),
            ));
        }
        let camera_id = body_camera
            .map(str::to_string)
            .ok_or_else(|| AppError::BadRequest("`camera_id` is required".into()))?;
        principal.require_camera(&camera_id, "ingest for this camera")?;
        note_unleased_ingest(st, &key_id, &camera_id).await;
        return Ok(IngestBinding {
            camera_id,
            task_type: body_task_type.unwrap_or_default().to_string(),
            // Preserved, not derived — see the note above on idempotency.
            frame_id: body_frame_id.filter(|f| !f.is_empty()).map(str::to_string),
            provenance: Provenance::Worker {
                api_key_id: key_id,
                task_id: None,
                worker_id: None,
            },
            ticketed: false,
        });
    };

    // Step 2: which task does the ticket claim, and do we hold a live lease on it? The lease is what
    // supplies the camera the MAC must be recomputed over — the ticket never carries it.
    let claimed = frame_ticket::peek(raw)
        .ok_or_else(|| AppError::Unauthorized("frame_ticket is malformed".into()))?;
    let lease = ai_leases::is_live(&st.pool, &claimed.task_id, &key_id)
        .await
        .unwrap_or(None)
        .ok_or_else(|| {
            AppError::Unauthorized(
                "frame_ticket names a task this credential holds no live lease on".into(),
            )
        })?;
    // Steps 3 + 4.
    let verified = frame_ticket::verify(raw, &key_id, &lease.camera_id, Utc::now().timestamp())
        .ok_or_else(|| AppError::Unauthorized("frame_ticket is invalid or expired".into()))?;

    // Step 5: the task and camera must still be live. A disabled task must stop being ingestable
    // immediately, not at the end of its lease.
    let still_enabled: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM ai_tasks t JOIN cameras c ON c.id = t.camera_id
          WHERE t.id = ? AND t.enabled = 1 AND c.enabled = 1",
    )
    .bind(&verified.task_id)
    .fetch_optional(&st.pool)
    .await?;
    if still_enabled.is_none() {
        return Err(AppError::Forbidden(
            "the leased AI task or its camera is disabled".into(),
        ));
    }
    principal.require_camera(&lease.camera_id, "ingest for this camera")?;

    // Cross-check whatever the body still claims. The reference worker keeps sending these; they are
    // now a consistency check rather than an input.
    if let Some(c) = body_camera.filter(|c| !c.is_empty()) {
        if c != lease.camera_id {
            return Err(AppError::Conflict(format!(
                "body `camera_id` = `{c}` but the frame ticket is for camera `{}`",
                lease.camera_id
            )));
        }
    }
    if let Some(t) = body_task_type.filter(|t| !t.is_empty()) {
        if t != lease.task_type {
            return Err(AppError::Conflict(format!(
                "body `task_type` = `{t}` but the frame ticket is for task type `{}`",
                lease.task_type
            )));
        }
    }

    Ok(IngestBinding {
        camera_id: lease.camera_id,
        task_type: lease.task_type,
        frame_id: Some(verified.frame_id()),
        provenance: Provenance::Worker {
            api_key_id: key_id,
            task_id: Some(verified.task_id),
            worker_id: Some(lease.worker_id),
        },
        ticketed: true,
    })
}

/// Record a ticketless ingest under the `warn` tier: one log line and one `ingest_unleased` event per
/// credential per hour, so an operator gets the list of clients that would break under `enforce`
/// WITHOUT the ingest path writing an event per frame.
async fn note_unleased_ingest(st: &AppState, key_id: &str, camera_id: &str) {
    if st.cfg.ingest_provenance != EnforcementTier::Warn {
        return;
    }
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
    let now = Utc::now().timestamp();
    let fire = {
        let Ok(mut map) = SEEN.get_or_init(Default::default).lock() else {
            return;
        };
        match map.get(key_id) {
            Some(last) if now - *last < 3600 => false,
            _ => {
                if map.len() > 1024 {
                    map.retain(|_, last| now - *last < 3600);
                }
                map.insert(key_id.to_string(), now);
                true
            }
        }
    };
    if !fire {
        return;
    }
    tracing::warn!(
        target: "heldar::security",
        credential = %key_id, camera_id,
        "ingest: batch posted with no frame ticket; accepted under \
         HELDAR_INGEST_PROVENANCE=warn. Under `enforce` this would be 401 \
         frame_ticket_required — the client must acquire a lease and pull frames with `?task=`."
    );
    let _ = crate::repo::log_event(
        &st.pool,
        Some(camera_id),
        "ingest_unleased",
        "warning",
        json!({ "credential": key_id, "tier": "warn" }),
    )
    .await;
}

/// Body cap for `POST /api/v1/ai/embeddings`, enforced before deserialization. Sized for
/// [`crate::services::embeddings::MAX_INGEST_EMBEDDINGS`] items each carrying a 512-d f32 vector
/// as JSON floats (~6 KB) plus an optional crop thumbnail (≤ ~128 KB base64).
const EMBED_INGEST_BODY_LIMIT_BYTES: usize = 24 * 1024 * 1024;

/// Body cap for a single query-embedding result (one vector + model id).
const EMBED_RESULT_BODY_LIMIT_BYTES: usize = 1024 * 1024;

/// Ingest a batch of crop embeddings posted by an AI worker's `embedding` task. Validation,
/// idempotency, and thumbnail persistence live in [`crate::services::embeddings::ingest_batch`];
/// this handler owns only the HTTP/RBAC surface (mirroring `ingest` above).
async fn ingest_embeddings(
    State(st): State<AppState>,
    principal: crate::auth::Principal,
    Json(body): Json<crate::services::embeddings::EmbeddingIngest>,
) -> AppResult<Json<Value>> {
    principal.require_cap(Cap::AiIngest, "ingest embeddings")?;
    // Same binding as detection ingest — an embedding batch carries crop thumbnails of people and
    // vehicles, so "which camera did this really come from?" matters just as much here.
    let bound = resolve_binding(
        &st,
        &principal,
        body.frame_ticket.as_deref(),
        Some(&body.camera_id),
        None,
        body.frame_id.as_deref(),
    )
    .await?;
    // `bound.frame_id` is already the right answer in both tiers: server-derived when ticketed, the
    // client's own when not.
    let inserted = crate::services::embeddings::ingest_batch(
        &st,
        &body,
        &bound.camera_id,
        bound.frame_id.as_deref(),
    )
    .await?;
    Ok(Json(json!({ "embeddings_ingested": inserted })))
}

#[derive(Deserialize)]
struct EmbedQueriesQuery {
    worker_id: Option<String>,
}

/// Hand pending query embeddings to a worker (claiming them). Polled fast (~1 s) by workers with
/// a CLIP backend, so the underlying claim is read-only when the queue is empty — see
/// [`crate::services::embeddings::claim_queries`].
///
/// Requires `ai:embedwork`, NOT `ai:ingest`: the payloads are the operator's own search text and
/// images. Splitting the two is what stops a key minted purely to POST detections from reading what
/// the operator is looking for.
async fn claim_embed_queries(
    State(st): State<AppState>,
    principal: crate::auth::Principal,
    Query(q): Query<EmbedQueriesQuery>,
) -> AppResult<Json<Value>> {
    principal.require_cap(Cap::AiEmbedWork, "claim embedding queries")?;
    // An absent worker id used to default to the literal "unknown", so every anonymous claimant shared
    // one identity in `claimed_by` and the audit trail was worthless.
    let worker = q
        .worker_id
        .as_deref()
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .ok_or_else(|| AppError::BadRequest("`worker_id` is required".into()))?;
    let queries =
        crate::services::embeddings::claim_queries(&st.pool, worker, &credential_id(&principal))
            .await?;
    Ok(Json(json!({ "queries": queries })))
}

/// Record a worker's answer (vector or error) for a claimed query. First result wins; late
/// duplicates return `updated: false`.
///
/// The answer is accepted only from the credential that CLAIMED the query — otherwise any principal
/// reaching this route could overwrite an in-flight vector and poison the operator's search result.
async fn embed_query_result(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: crate::auth::Principal,
    Json(body): Json<crate::services::embeddings::QueryResult>,
) -> AppResult<Json<Value>> {
    principal.require_cap(Cap::AiEmbedWork, "answer embedding queries")?;
    let updated = crate::services::embeddings::submit_query_result(
        &st.pool,
        &id,
        &body,
        &credential_id(&principal),
    )
    .await?;
    Ok(Json(json!({ "updated": updated })))
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
        assert_eq!(
            crate::services::perception_ingest::MAX_INGEST_DETECTIONS,
            1000
        );
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
            live: crate::services::live_publisher::LivePublisherManager::new(
                pool.clone(),
                cfg.clone(),
                reqwest::Client::new(),
            ),
            mirror: None,
            consumers: std::sync::Arc::new(Vec::new()),
            modules: std::sync::Arc::new(Vec::new()),
            catalog: std::sync::Arc::new(crate::services::registry::CatalogService::new(&cfg)),
            http: reqwest::Client::new(),
            media_jobs: crate::services::media_jobs::MediaJobGovernor::new(2),
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

    // ---- worker sharding (multi-worker task split) ----------------------------------------------

    #[test]
    fn worker_shard_partitions_tasks_disjointly_and_balanced() {
        let live: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        let total = 10;
        let mut union: Vec<usize> = Vec::new();
        let mut sizes = Vec::new();
        for w in &live {
            let mine = worker_shard(total, &live, w);
            // disjoint from what's already claimed
            for i in &mine {
                assert!(!union.contains(i), "task {i} claimed by two workers");
            }
            sizes.push(mine.len());
            union.extend(mine);
        }
        union.sort_unstable();
        assert_eq!(
            union,
            (0..total).collect::<Vec<_>>(),
            "every task is covered exactly once"
        );
        // Balanced: 10 tasks over 3 workers -> sizes {4,3,3}, differ by at most 1.
        assert_eq!(
            *sizes.iter().max().unwrap() - *sizes.iter().min().unwrap(),
            1
        );
    }

    #[test]
    fn worker_shard_single_worker_gets_all() {
        let live: Vec<String> = vec!["solo".into()];
        assert_eq!(worker_shard(5, &live, "solo"), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn worker_shard_absent_worker_defensively_gets_all() {
        // A worker that raced a prune out of the live set gets ALL tasks (redo, deduped) rather than
        // silently nothing — and an empty live set likewise.
        let live: Vec<String> = vec!["a".into(), "b".into()];
        assert_eq!(worker_shard(3, &live, "ghost"), vec![0, 1, 2]);
        assert_eq!(worker_shard(3, &[], "a"), vec![0, 1, 2]);
    }

    // ---- leases, frame tickets, and the ingest binding -------------------------------------------

    /// A state with a camera + one enabled anpr task, and the ingest tier set explicitly (the process
    /// env would otherwise decide, which makes these tests order-dependent).
    async fn ai_state(tier: EnforcementTier) -> AppState {
        let st = test_state().await;
        let mut cfg = crate::config::Config::from_env();
        cfg.ingest_provenance = tier;
        cfg.machine_auth = tier;
        let st = AppState {
            cfg: std::sync::Arc::new(cfg),
            ..st
        };
        crate::services::ai_leases::clear_cache_for_tests();
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO cameras (id, name, enabled, created_at, updated_at) VALUES (?,?,1,?,?)",
        )
        .bind("cam1")
        .bind("cam1")
        .bind(now)
        .bind(now)
        .execute(&st.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO ai_tasks (id, camera_id, task_type, enabled, stream_profile, fps, width,
                                   config, created_at, updated_at)
             VALUES ('ai_1','cam1','anpr',1,'sub',5.0,640,'{}',?,?)",
        )
        .bind(now)
        .bind(now)
        .execute(&st.pool)
        .await
        .unwrap();
        st
    }

    fn api_key(id: &str) -> Principal {
        Principal {
            id: id.into(),
            name: id.into(),
            role: crate::auth::Role::Integration,
            kind: PrincipalKind::ApiKey,
            caps: crate::auth::CapSet::ALL,
            scope: crate::auth::Scope::All,
        }
    }

    /// A credential id that is unique across the whole test binary.
    ///
    /// Two pieces of state on this path are process-global BY DESIGN — the lease cache in
    /// `services::ai_leases` (keyed by `(task_id, api_key_id)`) and the once-per-key-per-hour rate
    /// limiter behind `note_unleased_ingest`. Both are correct in production, where there is one
    /// process and one database. In tests they are shared across concurrently running cases that each
    /// have their OWN in-memory database, so a hard-coded `"key_a"` lets one test's lease satisfy
    /// another test's lookup, or consume another test's hourly log budget. Every test id is unique, so
    /// the shared maps are partitioned instead of contended.
    fn unique_key(label: &str) -> String {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        format!(
            "key_{label}_{}",
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )
    }

    async fn lease_for(st: &AppState, key: &str) {
        let _ = acquire_lease(
            State(st.clone()),
            api_key(key),
            Json(LeaseRequest {
                worker_id: "w1".into(),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    }

    const CAPTURED_MS: i64 = 1_700_000_000_123;

    fn mint_for(key: &str, camera: &str, task: &str) -> String {
        frame_ticket::mint(key, camera, task, CAPTURED_MS, Utc::now().timestamp(), 120).unwrap()
    }

    /// THE HEADLINE ENFORCE CONTROL: without a ticket there is no ingest.
    #[tokio::test]
    async fn enforce_refuses_a_ticketless_batch_with_frame_ticket_required() {
        let st = ai_state(EnforcementTier::Enforce).await;
        let ka = unique_key("a");
        let err = resolve_binding(&st, &api_key(&ka), None, Some("cam1"), Some("anpr"), None)
            .await
            .unwrap_err();
        match err {
            AppError::Unauthorized(m) => assert!(
                m.contains("frame_ticket_required"),
                "the worker keys on this token: {m}"
            ),
            other => panic!("expected 401, got {other:?}"),
        }
    }

    /// ...and WITH one, camera / task type / frame id are all server-derived.
    #[tokio::test]
    async fn a_valid_ticket_derives_camera_task_type_and_frame_id() {
        let st = ai_state(EnforcementTier::Enforce).await;
        let ka = unique_key("a");
        lease_for(&st, &ka).await;
        let ticket = mint_for(&ka, "cam1", "ai_1");

        let bound = resolve_binding(
            &st,
            &api_key(&ka),
            Some(&ticket),
            Some("cam1"),
            Some("anpr"),
            None,
        )
        .await
        .unwrap();
        assert!(bound.ticketed);
        assert_eq!(bound.camera_id, "cam1");
        assert_eq!(bound.task_type, "anpr");
        assert_eq!(
            bound.frame_id.as_deref(),
            Some(format!("ai_1:{CAPTURED_MS}").as_str()),
            "the frame id is derived from the ticket, never taken from the body"
        );
        match bound.provenance {
            Provenance::Worker {
                api_key_id,
                task_id,
                worker_id,
            } => {
                assert_eq!(api_key_id, ka);
                assert_eq!(task_id.as_deref(), Some("ai_1"));
                assert_eq!(worker_id.as_deref(), Some("w1"));
            }
            other => panic!("an API batch must never be kernel-provenance: {other:?}"),
        }
    }

    /// A ticket minted for key A is inert for key B — the test that fails if `api_key_id` is dropped
    /// from the HMAC preimage.
    #[tokio::test]
    async fn a_ticket_cannot_be_replayed_by_another_credential() {
        let st = ai_state(EnforcementTier::Enforce).await;
        let (ka, kb) = (unique_key("a"), unique_key("b"));
        lease_for(&st, &ka).await;
        lease_for(&st, &kb).await; // kb holds no lease on ai_1 (ka does)
        let ticket = mint_for(&ka, "cam1", "ai_1");

        let err = resolve_binding(
            &st,
            &api_key(&kb),
            Some(&ticket),
            Some("cam1"),
            Some("anpr"),
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)), "got {err:?}");
    }

    /// A ticket without a live lease is worthless, even if it verifies — which is what stops a
    /// credential ingesting for a task the lease does not cover.
    #[tokio::test]
    async fn a_ticket_for_an_unleased_task_is_refused() {
        let st = ai_state(EnforcementTier::Enforce).await;
        let ka = unique_key("a");
        // No lease acquired at all.
        let ticket = mint_for(&ka, "cam1", "ai_1");
        let err = resolve_binding(
            &st,
            &api_key(&ka),
            Some(&ticket),
            Some("cam1"),
            Some("anpr"),
            None,
        )
        .await
        .unwrap_err();
        match err {
            AppError::Unauthorized(m) => assert!(m.contains("lease"), "{m}"),
            other => panic!("expected 401, got {other:?}"),
        }

        // Releasing a held lease has the same effect immediately.
        lease_for(&st, &ka).await;
        assert!(resolve_binding(
            &st,
            &api_key(&ka),
            Some(&mint_for(&ka, "cam1", "ai_1")),
            Some("cam1"),
            Some("anpr"),
            None,
        )
        .await
        .is_ok());
        sqlx::query("DELETE FROM ai_task_leases")
            .execute(&st.pool)
            .await
            .unwrap();
        crate::services::ai_leases::clear_cache_for_tests();
        assert!(resolve_binding(
            &st,
            &api_key(&ka),
            Some(&mint_for(&ka, "cam1", "ai_1")),
            Some("cam1"),
            Some("anpr"),
            None,
        )
        .await
        .is_err());
    }

    /// An expired ticket is refused even under a live lease.
    #[tokio::test]
    async fn an_expired_ticket_is_refused() {
        let st = ai_state(EnforcementTier::Enforce).await;
        let ka = unique_key("a");
        lease_for(&st, &ka).await;
        let stale = frame_ticket::mint(
            &ka,
            "cam1",
            "ai_1",
            CAPTURED_MS,
            Utc::now().timestamp() - 600,
            60,
        )
        .unwrap();
        assert!(resolve_binding(
            &st,
            &api_key(&ka),
            Some(&stale),
            Some("cam1"),
            Some("anpr"),
            None
        )
        .await
        .is_err());
    }

    /// A body that disagrees with its ticket is a 409, not a silent reinterpretation.
    #[tokio::test]
    async fn a_body_that_contradicts_its_ticket_is_a_conflict() {
        let st = ai_state(EnforcementTier::Enforce).await;
        let ka = unique_key("a");
        lease_for(&st, &ka).await;
        let ticket = mint_for(&ka, "cam1", "ai_1");

        let err = resolve_binding(
            &st,
            &api_key(&ka),
            Some(&ticket),
            Some("some-other-camera"),
            Some("anpr"),
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::Conflict(_)), "got {err:?}");

        let err = resolve_binding(
            &st,
            &api_key(&ka),
            Some(&ticket),
            Some("cam1"),
            Some("object_detection"),
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::Conflict(_)), "got {err:?}");
    }

    /// A disabled task stops being ingestable immediately, not at the end of its lease.
    #[tokio::test]
    async fn disabling_a_task_revokes_its_tickets_at_once() {
        let st = ai_state(EnforcementTier::Enforce).await;
        let ka = unique_key("a");
        lease_for(&st, &ka).await;
        sqlx::query("UPDATE ai_tasks SET enabled = 0 WHERE id = 'ai_1'")
            .execute(&st.pool)
            .await
            .unwrap();
        let err = resolve_binding(
            &st,
            &api_key(&ka),
            Some(&mint_for(&ka, "cam1", "ai_1")),
            Some("cam1"),
            Some("anpr"),
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::Forbidden(_)), "got {err:?}");
    }

    /// CONSTRAINT 5. Under the default `warn`, a ticketless batch is accepted exactly as today —
    /// this is what keeps `validate_ai.sh` and friends green with zero edits.
    #[tokio::test]
    async fn warn_accepts_a_ticketless_batch_exactly_as_today() {
        let st = ai_state(EnforcementTier::Warn).await;
        let ka = unique_key("a");
        let bound = resolve_binding(&st, &api_key(&ka), None, Some("cam1"), Some("anpr"), None)
            .await
            .unwrap();
        assert!(!bound.ticketed);
        assert_eq!(bound.camera_id, "cam1");
        assert_eq!(bound.task_type, "anpr");
        assert!(bound.frame_id.is_none(), "no frame_id in, none out");
        // Even ticketless, provenance is still WORKER — never camera_native.
        assert_eq!(bound.provenance.source(), "worker");

        // REGRESSION: a ticketless batch keeps the CLIENT'S frame_id. Deriving it (or dropping it,
        // which an earlier revision did) silently disables `idx_outbox_dedup`, so an at-least-once
        // redelivery inserts twice — re-firing consumer side effects and adding a second ANPR vote
        // toward `min_votes` for a frame that was only ever seen once.
        // Same credential as above on purpose: it also pins the once-per-key-per-hour rate limit on
        // the `ingest_unleased` notice, so a chatty ticketless client cannot flood the event log.
        let kept = resolve_binding(
            &st,
            &api_key(&ka),
            None,
            Some("cam1"),
            Some("anpr"),
            Some("client-chosen-frame"),
        )
        .await
        .unwrap();
        assert_eq!(
            kept.frame_id.as_deref(),
            Some("client-chosen-frame"),
            "under warn the client's idempotency key must survive untouched"
        );
        // The operator gets a visible record of who would break under `enforce`.
        let noted: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE event_type = 'ingest_unleased'")
                .fetch_one(&st.pool)
                .await
                .unwrap();
        assert_eq!(
            noted, 1,
            "one notice per credential per hour, not one per batch"
        );
    }

    /// CONSTRAINT 1. Auth off: the synthetic principal leases, gets tickets, and ingests — the whole
    /// chain works with no credential at all, including under `enforce`.
    #[tokio::test]
    async fn the_auth_disabled_system_principal_walks_the_whole_chain() {
        let st = ai_state(EnforcementTier::Enforce).await;
        let _ = acquire_lease(
            State(st.clone()),
            Principal::system_admin(),
            Json(LeaseRequest {
                worker_id: "w1".into(),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        let ticket = mint_for("system", "cam1", "ai_1");
        let bound = resolve_binding(
            &st,
            &Principal::system_admin(),
            Some(&ticket),
            Some("cam1"),
            Some("anpr"),
            None,
        )
        .await
        .unwrap();
        assert!(bound.ticketed);
        assert_eq!(bound.provenance.source(), "worker");
    }

    /// Camera scope is checked BEFORE existence, so the boundary is not an existence oracle.
    #[tokio::test]
    async fn an_out_of_scope_camera_is_forbidden_not_not_found() {
        let st = ai_state(EnforcementTier::Warn).await;
        let scoped = Principal {
            scope: crate::auth::Scope::Cameras(std::sync::Arc::new(
                ["cam1".to_string()].into_iter().collect(),
            )),
            ..api_key(&unique_key("a"))
        };
        // In scope → fine.
        assert!(
            resolve_binding(&st, &scoped, None, Some("cam1"), Some("anpr"), None)
                .await
                .is_ok()
        );
        // Out of scope → 403 whether or not the camera exists.
        for other in ["cam2", "does-not-exist"] {
            let err = resolve_binding(&st, &scoped, None, Some(other), Some("anpr"), None)
                .await
                .unwrap_err();
            assert!(
                matches!(err, AppError::Forbidden(_)),
                "{other}: got {err:?}"
            );
        }
    }

    /// Leases are exclusive, and `GET /ai/tasks` is unchanged for a worker that never leases.
    #[tokio::test]
    async fn leases_are_exclusive_and_the_legacy_task_endpoint_is_unchanged() {
        let st = ai_state(EnforcementTier::Warn).await;
        lease_for(&st, &unique_key("a")).await;

        let second = acquire_lease(
            State(st.clone()),
            api_key(&unique_key("b")),
            Json(LeaseRequest {
                worker_id: "w2".into(),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        assert!(second.0.tasks.is_empty(), "a live lease is exclusive");

        // A worker that never calls /ai/leases still sees the task, exactly as before.
        let Json(tasks) = list_all_tasks(
            State(st.clone()),
            Query(TasksQuery { worker_id: None }),
            api_key(&unique_key("c")),
        )
        .await
        .unwrap();
        assert_eq!(tasks.len(), 1);
        assert!(
            tasks[0].frame_url.contains("task=ai_1"),
            "frame_url must carry the task id so a lease-holder gets a ticket: {}",
            tasks[0].frame_url
        );
    }

    /// GRAFT G4: a worker id belongs to the credential that registered it. Reverting the conditional
    /// upsert restores the shard-collapse denial available to any read credential.
    #[tokio::test]
    async fn a_worker_id_is_bound_to_the_credential_that_registered_it() {
        let st = ai_state(EnforcementTier::Enforce).await;
        let (ka, kb) = (unique_key("a"), unique_key("b"));
        let q = || {
            Query(TasksQuery {
                worker_id: Some("w1".into()),
            })
        };
        assert!(list_all_tasks(State(st.clone()), q(), api_key(&ka))
            .await
            .is_ok());

        let err = list_all_tasks(State(st.clone()), q(), api_key(&kb))
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Conflict(_)), "got {err:?}");

        let owner: Option<String> =
            sqlx::query_scalar("SELECT api_key_id FROM ai_workers WHERE worker_id = 'w1'")
                .fetch_one(&st.pool)
                .await
                .unwrap();
        assert_eq!(
            owner.as_deref(),
            Some(ka.as_str()),
            "the binding is unchanged"
        );

        // Under warn the same heartbeat is allowed through with a warning (constraint 5).
        let mut warn_cfg = crate::config::Config::from_env();
        warn_cfg.machine_auth = EnforcementTier::Warn;
        let warn_st = AppState {
            cfg: std::sync::Arc::new(warn_cfg),
            ..st
        };
        assert!(list_all_tasks(State(warn_st), q(), api_key(&kb))
            .await
            .is_ok());
    }

    /// CONSTRAINT 4: a broken lease table must degrade to "no ticket", never to a failed ingest.
    #[tokio::test]
    async fn a_missing_lease_table_degrades_to_no_ticket_rather_than_an_outage() {
        let st = ai_state(EnforcementTier::Warn).await;
        let ka = unique_key("a");
        sqlx::query("DROP TABLE ai_task_leases")
            .execute(&st.pool)
            .await
            .unwrap();
        // Ticketless ingest still resolves under warn...
        assert!(
            resolve_binding(&st, &api_key(&ka), None, Some("cam1"), Some("anpr"), None)
                .await
                .is_ok()
        );
        // ...and presenting a ticket simply fails the lease lookup rather than 500ing.
        let err = resolve_binding(
            &st,
            &api_key(&ka),
            Some(&mint_for(&ka, "cam1", "ai_1")),
            Some("cam1"),
            Some("anpr"),
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)), "got {err:?}");
    }

    // ---- camera scope ---------------------------------------------------------------------------

    fn scoped(cameras: &[&str]) -> Principal {
        let set: std::collections::HashSet<String> =
            cameras.iter().map(|c| c.to_string()).collect();
        Principal {
            scope: crate::auth::Scope::Cameras(std::sync::Arc::new(set)),
            ..Principal::system_admin()
        }
    }

    async fn seed_task(pool: &sqlx::SqlitePool, camera_id: &str, task_id: &str) {
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO cameras (id, name, enabled, created_at, updated_at) VALUES (?,?,1,?,?)",
        )
        .bind(camera_id)
        .bind(camera_id)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO ai_tasks (id, camera_id, task_type, enabled, stream_profile, fps, width,
                                   config, created_at, updated_at)
             VALUES (?,?,'detection',1,'sub',2.0,640,'{}',?,?)",
        )
        .bind(task_id)
        .bind(camera_id)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
    }

    /// `GET|POST /api/v1/cameras/{id}/ai-tasks` used the RAW `load_camera`, so a scoped key could read
    /// and create tasks on any camera. They now go through `camera_for` like every other camera path.
    #[tokio::test]
    async fn camera_keyed_ai_task_routes_are_scoped() {
        let st = test_state().await;
        seed_task(&st.pool, "cam_a", "ai_a").await;
        seed_task(&st.pool, "cam_sentinel_b", "ai_b").await;
        let p = scoped(&["cam_a"]);

        assert!(matches!(
            list_camera_tasks(State(st.clone()), Path("cam_sentinel_b".into()), p.clone())
                .await
                .unwrap_err(),
            AppError::Forbidden(_)
        ));
        assert!(matches!(
            list_detections(
                State(st.clone()),
                p.clone(),
                Path("cam_sentinel_b".into()),
                Query(DetectionQuery {
                    from: None,
                    to: None,
                    label: None,
                    limit: None
                }),
            )
            .await
            .unwrap_err(),
            AppError::Forbidden(_)
        ));
        let create = AiTaskCreate {
            task_type: "detection".into(),
            stream_profile: Some("main".into()),
            fps: None,
            width: None,
            config: None,
            enabled: Some(true),
        };
        assert!(matches!(
            create_task(
                State(st.clone()),
                Path("cam_sentinel_b".into()),
                p.clone(),
                Json(create),
            )
            .await
            .unwrap_err(),
            AppError::Forbidden(_)
        ));
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ai_tasks WHERE camera_id = ?")
            .bind("cam_sentinel_b")
            .fetch_one(&st.pool)
            .await
            .unwrap();
        assert_eq!(n, 1, "no task was created on the out-of-scope camera");

        // No over-blocking on its own camera.
        assert!(
            list_camera_tasks(State(st.clone()), Path("cam_a".into()), p)
                .await
                .is_ok()
        );
    }

    /// PATCH/DELETE `/api/v1/ai-tasks/{task_id}` are addressed by task id: refuse before the row is
    /// disclosed, and refuse a missing id identically so the task id space cannot be enumerated.
    #[tokio::test]
    async fn task_id_mutations_are_scoped_and_are_not_an_existence_oracle() {
        let st = test_state().await;
        seed_task(&st.pool, "cam_a", "ai_a").await;
        seed_task(&st.pool, "cam_sentinel_b", "ai_b").await;
        let p = scoped(&["cam_a"]);

        let out_of_scope = delete_task(State(st.clone()), Path("ai_b".into()), p.clone())
            .await
            .unwrap_err();
        let nonexistent = delete_task(State(st.clone()), Path("ai_zzz".into()), p.clone())
            .await
            .unwrap_err();
        assert!(matches!(out_of_scope, AppError::Forbidden(_)));
        assert_eq!(out_of_scope.to_string(), nonexistent.to_string());
        assert!(!out_of_scope.to_string().contains("cam_sentinel_b"));

        let disable: AiTaskUpdate = serde_json::from_value(json!({ "enabled": false })).unwrap();
        assert!(matches!(
            update_task(State(st.clone()), Path("ai_b".into()), p, Json(disable))
                .await
                .unwrap_err(),
            AppError::Forbidden(_)
        ));
        let enabled: i64 = sqlx::query_scalar("SELECT enabled FROM ai_tasks WHERE id = 'ai_b'")
            .fetch_one(&st.pool)
            .await
            .unwrap();
        assert_eq!(enabled, 1, "another camera's perception is still running");

        // Unscoped keeps the pre-existing 404 text.
        match delete_task(
            State(st.clone()),
            Path("ai_zzz".into()),
            Principal::system_admin(),
        )
        .await
        .unwrap_err()
        {
            AppError::NotFound(m) => assert_eq!(m, "ai task ai_zzz not found"),
            other => panic!("expected the pre-existing 404, got {other:?}"),
        }
    }

    /// `GET /api/v1/ai/tasks` is the worker-discovery roster: fleet-wide for an unscoped credential,
    /// confined for a scoped one. This is the route that otherwise hands out every camera id on the box.
    #[tokio::test]
    async fn worker_discovery_is_confined_to_the_credentials_cameras() {
        let st = test_state().await;
        seed_task(&st.pool, "cam_a", "ai_a").await;
        seed_task(&st.pool, "cam_sentinel_b", "ai_b").await;
        let q = || Query(TasksQuery { worker_id: None });

        let Json(mine) = list_all_tasks(State(st.clone()), q(), scoped(&["cam_a"]))
            .await
            .unwrap();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].camera_id, "cam_a");
        let body = serde_json::to_string(&mine).unwrap();
        assert!(!body.contains("cam_sentinel_b"), "{body}");

        // An empty scope selects nothing rather than everything.
        let Json(none) = list_all_tasks(State(st.clone()), q(), scoped(&[]))
            .await
            .unwrap();
        assert!(none.is_empty());

        // Unscoped: unchanged, the whole fleet.
        let Json(all) = list_all_tasks(State(st.clone()), q(), Principal::system_admin())
            .await
            .unwrap();
        assert_eq!(all.len(), 2);
    }
}
