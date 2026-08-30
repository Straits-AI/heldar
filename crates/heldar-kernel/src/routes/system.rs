use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::{Cap, Principal};
use crate::error::{AppError, AppResult};
use crate::services::remote_access::{self, OverlayStatus};
use crate::services::settings;
use crate::services::storage::{self, StorageReport};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/api/v1/system", get(system_info))
        .route(
            "/api/v1/system/retention",
            get(get_retention).put(put_retention),
        )
        .route("/api/v1/system/db", get(get_db_status).put(put_db_limit))
        .route("/api/v1/system/db/convert", post(post_db_convert))
        .route(
            "/api/v1/system/transcode",
            get(get_transcode).put(put_transcode),
        )
        .route(
            "/api/v1/system/timezone",
            get(get_timezone).put(put_timezone),
        )
}

/// The box-wide timezone, and — the part that matters — where the effective one comes from.
///
/// "UTC" and "nobody has configured a zone" look identical in a timestamp and mean very different
/// things, so the source is reported beside the value. `server_local_offset` is the box's own clock
/// offset, previously visible only in the boot log; next to the configured zone it is what lets an
/// operator see "the site says Asia/Kuala_Lumpur but this container is running on +00:00".
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TimezoneSettings {
    /// The IANA identifier configured box-wide, if any.
    configured: Option<String>,
    source: crate::services::tz::TzSource,
    /// The server's own local offset (`%:z`), for spotting a container whose `TZ` disagrees.
    server_local_offset: String,
    /// What an unconfigured box does, stated rather than left to be discovered: schedules follow
    /// the SERVER's local zone and search follows UTC. Setting a zone makes both follow it.
    unconfigured_behaviour: &'static str,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct TimezoneUpdate {
    /// An IANA identifier, e.g. `Asia/Kuala_Lumpur`. Empty clears it.
    timezone: String,
}

async fn timezone_settings(st: &AppState) -> TimezoneSettings {
    let (tz, source) = crate::services::tz::site_tz(&st.pool, None).await;
    TimezoneSettings {
        configured: tz.map(|t| t.to_string()),
        source,
        server_local_offset: chrono::Local::now().format("%:z").to_string(),
        unconfigured_behaviour:
            "with no zone configured, recording schedules follow the SERVER's local timezone and \
             search hour filters follow UTC — the historical behaviour of each. Setting a zone \
             makes both follow it.",
    }
}

#[utoipa::path(
    get, path = "/api/v1/system/timezone", tag = "system",
    responses((status = 200, description = "The effective timezone and where it comes from", body = TimezoneSettings)),
)]
pub async fn get_timezone(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<TimezoneSettings>> {
    principal.require_cap(Cap::SystemRead, "read the timezone")?;
    Ok(Json(timezone_settings(&st).await))
}

/// Set the box-wide timezone (admin only).
///
/// REFUSED FOR A CAMERA-SCOPED CREDENTIAL. A zone reinterprets every schedule and every relative
/// search on the box, so it is fleet-wide by nature — the same reasoning as the transcode engine.
///
/// The value is validated here rather than at read time. A stored zone that does not parse falls
/// back silently by design (a corrupted row must not take the recorder down), so if writes did not
/// refuse, `Asia/KL` would be accepted with a 200 and the box would quietly keep answering in the
/// old zone.
#[utoipa::path(
    put, path = "/api/v1/system/timezone", tag = "system",
    request_body = TimezoneUpdate,
    responses(
        (status = 200, description = "The new effective timezone", body = TimezoneSettings),
        (status = 400, description = "Not an IANA timezone identifier", body = crate::openapi::ErrorBody),
        (status = 403, description = "Not an admin, or a camera-scoped credential", body = crate::openapi::ErrorBody),
    ),
)]
pub async fn put_timezone(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<TimezoneUpdate>,
) -> AppResult<Json<TimezoneSettings>> {
    principal.require(principal.can_admin(), "change the timezone")?;
    crate::routes::cameras::require_fleet_scope(&principal, "change the box timezone")?;

    let raw = body.timezone.trim().to_string();
    if !raw.is_empty() && crate::services::tz::parse(&raw).is_none() {
        return Err(AppError::BadRequest(format!(
            "`timezone` must be an IANA identifier such as `Asia/Kuala_Lumpur` (got {raw:?}). \
             Abbreviations and fixed offsets are not accepted: `GMT+8` and `+08:00` cannot express \
             daylight saving, and a recorder that is an hour out twice a year returns the wrong \
             footage for a valid search."
        )));
    }
    settings::set_str(&st.pool, crate::services::tz::DEFAULT_TIMEZONE, &raw).await?;
    crate::auth::audit(
        &st.pool,
        &principal,
        "update_timezone",
        "settings",
        "timezone",
        json!({ "timezone": raw }),
    )
    .await;
    Ok(Json(timezone_settings(&st).await))
}

const BYTES_PER_GB: f64 = 1024.0 * 1024.0 * 1024.0;

/// The recording disk-limit policy enforced by the retention sweeper. Each value is the operator
/// override (settings table) when set, otherwise the env default — `overridden` flags which is which.
#[derive(Debug, Serialize)]
struct RetentionLimits {
    max_recordings_gb: f64,
    max_recordings_bytes: i64,
    max_overridden: bool,
    min_free_disk_gb: f64,
    min_free_disk_bytes: i64,
    min_free_overridden: bool,
}

async fn effective_limits(st: &AppState) -> RetentionLimits {
    let max_override = settings::get_i64(&st.pool, settings::RECORDING_MAX_BYTES)
        .await
        .filter(|&v| v > 0);
    let floor_override = settings::get_i64(&st.pool, settings::RECORDING_MIN_FREE_BYTES)
        .await
        .filter(|&v| v >= 0);
    let max = max_override.unwrap_or(st.cfg.max_recordings_bytes as i64);
    let floor = floor_override.unwrap_or(st.cfg.min_free_disk_bytes as i64);
    RetentionLimits {
        max_recordings_gb: max as f64 / BYTES_PER_GB,
        max_recordings_bytes: max,
        max_overridden: max_override.is_some(),
        min_free_disk_gb: floor as f64 / BYTES_PER_GB,
        min_free_disk_bytes: floor,
        min_free_overridden: floor_override.is_some(),
    }
}

/// Current recording disk limits (effective values). Any authenticated viewer may read.
async fn get_retention(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<RetentionLimits>> {
    principal.require_cap(Cap::SystemRead, "view recording limits")?;
    Ok(Json(effective_limits(&st).await))
}

#[derive(Debug, Deserialize)]
struct RetentionUpdate {
    /// New global recordings cap in GB (> 0). Omit to leave unchanged.
    max_recordings_gb: Option<f64>,
    /// New free-disk floor in GB (>= 0; 0 disables the floor). Omit to leave unchanged.
    min_free_disk_gb: Option<f64>,
}

/// Set the recording disk limits at runtime (admin only) — the retention sweeper picks them up on its
/// next pass, no restart. Stored in the settings table; clearing them reverts to the env defaults.
async fn put_retention(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<RetentionUpdate>,
) -> AppResult<Json<RetentionLimits>> {
    principal.require(principal.can_admin(), "change recording limits")?;
    // Box-level setting with no camera to scope by, so refusal is the only coherent answer.
    // This one is the sharpest of the four: the value lands in settings, and the retention sweeper
    // reads it LATER and evicts the oldest segments FLEET-WIDE (`services/retention.rs`, whose
    // eviction query carries no camera predicate) with no principal and no scope in scope. A scoped
    // credential that can shrink this cap deletes other cameras' footage without ever naming them —
    // the request is scope-clean and the damage happens after it returns.
    crate::routes::cameras::require_fleet_scope(&principal, "change recording limits")?;
    if let Some(gb) = body.max_recordings_gb {
        if !gb.is_finite() || gb <= 0.0 {
            return Err(AppError::BadRequest(
                "`max_recordings_gb` must be greater than 0".into(),
            ));
        }
        settings::set_i64(
            &st.pool,
            settings::RECORDING_MAX_BYTES,
            (gb * BYTES_PER_GB) as i64,
        )
        .await?;
    }
    if let Some(gb) = body.min_free_disk_gb {
        if !gb.is_finite() || gb < 0.0 {
            return Err(AppError::BadRequest(
                "`min_free_disk_gb` must be 0 or greater".into(),
            ));
        }
        settings::set_i64(
            &st.pool,
            settings::RECORDING_MIN_FREE_BYTES,
            (gb * BYTES_PER_GB) as i64,
        )
        .await?;
    }
    crate::auth::audit(
        &st.pool,
        &principal,
        "update_retention_limits",
        "settings",
        "recording",
        json!({ "max_recordings_gb": body.max_recordings_gb, "min_free_disk_gb": body.min_free_disk_gb }),
    )
    .await;
    Ok(Json(effective_limits(&st).await))
}

/// The live-preview transcode engine: effective value + which hardware encoders LOOK available on
/// this box (device-node presence — a hint for the picker, not a guarantee the driver works).
#[derive(Debug, Serialize)]
struct TranscodeSettings {
    /// The engine new live publishers use: `software` | `vaapi` | `nvenc`.
    engine: String,
    /// True when the engine is an operator override (settings table) vs the env default.
    overridden: bool,
    /// The `HELDAR_LIVE_TRANSCODE_ENGINE` env default this falls back to.
    env_default: String,
    /// `/dev/dri/renderD*` present (Intel/AMD VAAPI render node).
    vaapi_available: bool,
    /// `/dev/nvidia*` present (NVIDIA NVENC).
    nvenc_available: bool,
}

async fn transcode_settings(st: &AppState) -> TranscodeSettings {
    let override_ = settings::get_str(&st.pool, settings::LIVE_TRANSCODE_ENGINE)
        .await
        .filter(|e| crate::services::mediamtx::VALID_ENGINES.contains(&e.as_str()));
    TranscodeSettings {
        // The canonical effective engine (an invalid env default reads as the software fallback it
        // actually runs as), so the UI's picker always shows a real, selectable value.
        engine: crate::services::mediamtx::effective_engine(&st.pool, &st.cfg).await,
        overridden: override_.is_some(),
        env_default: st.cfg.live_transcode_engine.clone(),
        vaapi_available: std::fs::read_dir("/dev/dri")
            .map(|d| {
                d.flatten()
                    .any(|e| e.file_name().to_string_lossy().starts_with("renderD"))
            })
            .unwrap_or(false),
        nvenc_available: std::path::Path::new("/dev/nvidiactl").exists()
            || std::path::Path::new("/dev/nvidia0").exists(),
    }
}

/// Current live-transcode engine (effective value + detected hardware). Any viewer may read.
async fn get_transcode(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<TranscodeSettings>> {
    principal.require_cap(Cap::SystemRead, "view transcode settings")?;
    Ok(Json(transcode_settings(&st).await))
}

#[derive(Debug, Deserialize)]
struct TranscodeUpdate {
    /// New engine (`software` | `vaapi` | `nvenc`).
    engine: String,
}

/// Set the live-transcode engine at runtime (admin only). New live sessions pick it up immediately;
/// already-running publishers (warm AND watched on-demand) are restarted onto it within seconds
/// (the write pokes a reconcile pass) — attached viewers see a brief reconnect. Stored in the
/// settings table; the env default remains the fallback.
async fn put_transcode(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<TranscodeUpdate>,
) -> AppResult<Json<TranscodeSettings>> {
    principal.require(principal.can_admin(), "change transcode engine")?;
    // Restarts every publisher on the box — fleet-wide by nature, not scopable.
    crate::routes::cameras::require_fleet_scope(&principal, "change the transcode engine")?;
    let engine = body.engine.trim().to_lowercase();
    if !crate::services::mediamtx::VALID_ENGINES.contains(&engine.as_str()) {
        return Err(AppError::BadRequest(format!(
            "`engine` must be one of: {}",
            crate::services::mediamtx::VALID_ENGINES.join(", ")
        )));
    }
    settings::set_str(&st.pool, settings::LIVE_TRANSCODE_ENGINE, &engine).await?;
    // Apply to running publishers now (a spawned reconcile pass) instead of the next 30s tick.
    st.live.poke();
    crate::auth::audit(
        &st.pool,
        &principal,
        "update_live_transcode_engine",
        "settings",
        "live_transcode",
        json!({ "engine": engine }),
    )
    .await;
    Ok(Json(transcode_settings(&st).await))
}

/// Metadata-DB (`heldar.db`) status + size cap. `incremental` = the DB is in `auto_vacuum=INCREMENTAL`
/// mode, in which the size cap can reclaim freed space back to the OS. `max_overridden` flags an
/// operator override (settings table) vs the env default.
#[derive(Debug, Serialize)]
struct DbStatus {
    db_bytes: i64,
    max_db_gb: f64,
    max_db_bytes: i64,
    max_overridden: bool,
    incremental: bool,
}

async fn db_status(st: &AppState) -> AppResult<DbStatus> {
    let max_override = settings::get_i64(&st.pool, settings::DB_MAX_BYTES)
        .await
        .filter(|&v| v > 0);
    let max = max_override.unwrap_or(st.cfg.max_db_bytes as i64);
    let db_bytes = crate::services::db_maintenance::db_size_bytes(&st.pool).await? as i64;
    let mode = crate::services::db_maintenance::auto_vacuum_mode(&st.pool).await?;
    Ok(DbStatus {
        db_bytes,
        max_db_gb: max as f64 / BYTES_PER_GB,
        max_db_bytes: max,
        max_overridden: max_override.is_some(),
        incremental: mode == 2,
    })
}

/// Current metadata-DB size + cap + conversion status. Any authenticated viewer may read.
async fn get_db_status(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<DbStatus>> {
    principal.require_cap(Cap::SystemRead, "view database status")?;
    // `db_bytes` grows with the fleet's cameras, segments and events — the same fleet-shape signal
    // that was just removed from `GET /api/v1/system`, one path segment away. Scoping a single
    // aggregate is meaningless, so refuse.
    crate::routes::cameras::require_fleet_scope(&principal, "view database status")?;
    Ok(Json(db_status(&st).await?))
}

#[derive(Debug, Deserialize)]
struct DbLimitUpdate {
    /// New metadata-DB size cap in GB (> 0). Omit to leave unchanged.
    max_db_gb: Option<f64>,
}

/// Set the metadata-DB size cap at runtime (admin only) — the retention sweeper picks it up on its
/// next pass, no restart. Stored in the settings table; clearing it reverts to the env default.
async fn put_db_limit(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<DbLimitUpdate>,
) -> AppResult<Json<DbStatus>> {
    principal.require(principal.can_admin(), "change database size cap")?;
    // Same deferred-execution shape as the recording cap: written now, enforced later by a sweep
    // that has no principal.
    crate::routes::cameras::require_fleet_scope(&principal, "change the database size cap")?;
    if let Some(gb) = body.max_db_gb {
        if !gb.is_finite() || gb <= 0.0 {
            return Err(AppError::BadRequest(
                "`max_db_gb` must be greater than 0".into(),
            ));
        }
        settings::set_i64(&st.pool, settings::DB_MAX_BYTES, (gb * BYTES_PER_GB) as i64).await?;
    }
    crate::auth::audit(
        &st.pool,
        &principal,
        "update_db_limit",
        "settings",
        "database",
        json!({ "max_db_gb": body.max_db_gb }),
    )
    .await;
    Ok(Json(db_status(&st).await?))
}

#[derive(Debug, Serialize)]
struct DbConvertResult {
    /// "already-incremental" (no-op) or "started" (a background conversion was kicked off).
    status: &'static str,
}

/// Trigger the one-time `auto_vacuum=INCREMENTAL` conversion online (admin only). A no-op if the DB is
/// already incremental; otherwise spawns the conversion in the BACKGROUND (it holds a write lock for
/// its duration) and returns immediately — the UI polls `GET /api/v1/system/db` until `incremental`
/// flips true. Best-effort + disk-gated + convergence-checked (see `ensure_incremental_autovacuum`).
async fn post_db_convert(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<DbConvertResult>> {
    principal.require(principal.can_admin(), "convert database auto_vacuum")?;
    // Rewrites the whole database file; there is no per-camera version of this.
    crate::routes::cameras::require_fleet_scope(&principal, "convert the database")?;
    if crate::services::db_maintenance::auto_vacuum_mode(&st.pool).await? == 2 {
        return Ok(Json(DbConvertResult {
            status: "already-incremental",
        }));
    }
    let (pool, cfg) = (st.pool.clone(), st.cfg.clone());
    tokio::spawn(async move {
        match crate::services::db_maintenance::ensure_incremental_autovacuum(&pool, &cfg).await {
            Ok(true) => tracing::info!("db: UI-triggered auto_vacuum conversion complete"),
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(error = %e, "db: UI-triggered auto_vacuum conversion failed")
            }
        }
    });
    crate::auth::audit(
        &st.pool,
        &principal,
        "convert_db_autovacuum",
        "settings",
        "database",
        json!({}),
    )
    .await;
    Ok(Json(DbConvertResult { status: "started" }))
}

/// Liveness: the process is up.
async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

/// Readiness: the database is reachable (returns 503 otherwise). When
/// `HELDAR_READYZ_MIN_RECORDING_PERCENT > 0` this also acts as an HA recorder-quorum probe (see
/// docs/HA.md): a node whose recording coverage drops below the threshold reports 503 so a
/// keepalived `health_script` can fail it over to a hot spare. Default 0 keeps DB-only behaviour.
async fn readyz(State(st): State<AppState>) -> Response {
    if let Err(e) = sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(&st.pool)
        .await
    {
        tracing::error!(error = %e, "readyz: database not reachable");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "ready": false, "reason": "database" })),
        )
            .into_response();
    }

    let required = st.cfg.readyz_min_recording_percent;
    if required > 0.0 {
        let counts = async {
            let enabled: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cameras WHERE enabled = 1")
                .fetch_one(&st.pool)
                .await?;
            let recording: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM camera_status cs JOIN cameras c ON c.id = cs.camera_id WHERE cs.state = 'recording' AND c.enabled = 1")
                    .fetch_one(&st.pool)
                    .await?;
            Ok::<_, sqlx::Error>((enabled, recording))
        }
        .await;
        let (enabled, recording) = match counts {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, "readyz: recorder-quorum query failed");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({ "ready": false, "reason": "database" })),
                )
                    .into_response();
            }
        };
        // No enabled cameras => nothing to record => the node is ready by definition.
        let pct = if enabled > 0 {
            (recording as f64) * 100.0 / (enabled as f64)
        } else {
            100.0
        };
        let pct = (pct * 10.0).round() / 10.0;
        if pct < required {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "ready": false,
                    "reason": "insufficient_recorders",
                    "recording_pct": pct,
                    "required_pct": required,
                })),
            )
                .into_response();
        }
    }

    (StatusCode::OK, Json(json!({ "ready": true }))).into_response()
}

#[derive(Debug, Serialize)]
struct SystemInfo {
    name: &'static str,
    version: &'static str,
    started_at: DateTime<Utc>,
    uptime_seconds: i64,
    recorder_enabled: bool,
    cameras_total: i64,
    cameras_recording: i64,
    active_recorders: usize,
    segments_total: i64,
    recordings_bytes: i64,
    recordings_gb: f64,
    max_recordings_gb: f64,
    storage: StorageReport,
    remote_access: OverlayStatus,
    /// WebRTC remote-dashboard relay health (the dial-out that carries remote login/API). `configured`
    /// false = remote access not set up (UI hides the row); `healthy` false while configured = the box
    /// is up but the remote path is dead (the 2026-07-15 failure mode) — surfaced instead of hidden.
    relay: RelayStatus,
    /// No recent disk_smart_warning/raid_degraded events (see services::health disk-health pass).
    disk_health_ok: bool,
    /// Timestamp of the most recent disk-health alert (any time), or null if none ever fired.
    last_disk_alert_at: Option<DateTime<Utc>>,
    /// Active live-preview transcode engine (software | vaapi | nvenc).
    live_transcode_engine: String,
    /// Resolved enforcement posture for the two staged security tiers. Reported rather than inferred:
    /// `ingest_provenance` is deliberately NOT promoted by `HELDAR_DEPLOYMENT_MODE` (see
    /// `Config::from_env`), so an operator who hardened the deployment mode and assumed both tiers
    /// moved would otherwise have no way to see that ticketless AI ingest is still accepted.
    enforcement: EnforcementPosture,
}

/// The effective value of each staged enforcement tier, plus whether the deployment mode had any say.
#[derive(Debug, Serialize)]
struct EnforcementPosture {
    /// `off` | `warn` | `enforce` — capability enforcement for credentials with no explicit grant.
    machine_auth: &'static str,
    /// `off` | `warn` | `enforce` — frame-ticket requirement on the AI ingest path.
    ingest_provenance: &'static str,
    /// True when `ingest_provenance` is `enforce`. Named positively so a dashboard does not have to
    /// string-compare a tier to decide whether to show the "ticketless ingest is accepted" notice.
    frame_tickets_required: bool,
    /// `HELDAR_DEPLOYMENT_MODE` as resolved (empty = unset).
    deployment_mode: String,
    /// Whether the deployment mode promotes a tier at all. Only `machine_auth` is ever promoted;
    /// `ingest_provenance` must be set explicitly because it is a client-protocol requirement.
    machine_auth_promoted_by_mode: bool,
}

fn enforcement_posture(st: &AppState) -> EnforcementPosture {
    EnforcementPosture {
        machine_auth: st.cfg.machine_auth.as_str(),
        ingest_provenance: st.cfg.ingest_provenance.as_str(),
        frame_tickets_required: st.cfg.ingest_provenance == crate::config::EnforcementTier::Enforce,
        deployment_mode: st.cfg.deployment_mode.clone(),
        machine_auth_promoted_by_mode: st.cfg.deployment_mode_is_production(),
    }
}

/// Health of the WebRTC remote-dashboard relay dial-out (see `services::webrtc_rendezvous`).
#[derive(Debug, Serialize)]
struct RelayStatus {
    configured: bool,
    healthy: bool,
    last_ok_at: Option<DateTime<Utc>>,
}

fn relay_status(st: &AppState) -> RelayStatus {
    // The relay runs only when remote access is configured AND kernel auth is on (its own guard).
    let configured =
        st.cfg.rendezvous_url.is_some() && st.cfg.site_id.is_some() && st.cfg.auth_enabled;
    let (healthy, last_ok) = crate::services::webrtc_rendezvous::relay_health();
    RelayStatus {
        configured,
        // Unconfigured = trivially healthy (nothing to be unhealthy about; the UI hides the row).
        healthy: !configured || healthy,
        last_ok_at: last_ok.and_then(|s| DateTime::<Utc>::from_timestamp(s, 0)),
    }
}

async fn system_info(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<SystemInfo>> {
    principal.require_cap(Cap::SystemRead, "view system info")?;
    // These four aggregates are the fleet's shape. Unscoped they answered "how many cameras exist
    // outside your scope, and how much footage do they hold" — precisely the bit `list_cameras`
    // filters away, handed back as a count. `cameras_total = 14` next to a one-camera
    // `GET /api/v1/cameras` is a complete inventory disclosure, and differencing it over time reports
    // fleet changes. Scope every one of them to the caller's cameras.
    let cam_scope = crate::state::camera_scope_filter(&principal, "id");
    let mut cameras_total_sql = "SELECT COUNT(*) FROM cameras WHERE 1=1".to_string();
    if let Some((pred, _)) = &cam_scope {
        cameras_total_sql.push_str(pred);
    }
    let mut q = sqlx::query_scalar::<_, i64>(&cameras_total_sql);
    if let Some((_, binds)) = &cam_scope {
        for b in binds {
            q = q.bind(b.clone());
        }
    }
    let cameras_total: i64 = q.fetch_one(&st.pool).await?;

    let rec_scope = crate::state::camera_scope_filter(&principal, "cs.camera_id");
    let mut recording_sql = "SELECT COUNT(*) FROM camera_status cs JOIN cameras c ON c.id = cs.camera_id WHERE cs.state = 'recording' AND c.enabled = 1".to_string();
    if let Some((pred, _)) = &rec_scope {
        recording_sql.push_str(pred);
    }
    let mut q = sqlx::query_scalar::<_, i64>(&recording_sql);
    if let Some((_, binds)) = &rec_scope {
        for b in binds {
            q = q.bind(b.clone());
        }
    }
    let cameras_recording: i64 = q.fetch_one(&st.pool).await?;

    let seg_scope = crate::state::camera_scope_filter(&principal, "camera_id");
    let mut segments_sql = "SELECT COUNT(*) FROM segments WHERE 1=1".to_string();
    if let Some((pred, _)) = &seg_scope {
        segments_sql.push_str(pred);
    }
    let mut q = sqlx::query_scalar::<_, i64>(&segments_sql);
    if let Some((_, binds)) = &seg_scope {
        for b in binds {
            q = q.bind(b.clone());
        }
    }
    let segments_total: i64 = q.fetch_one(&st.pool).await?;

    let mut bytes_sql = "SELECT COALESCE(SUM(size_bytes), 0) FROM segments WHERE 1=1".to_string();
    if let Some((pred, _)) = &seg_scope {
        bytes_sql.push_str(pred);
    }
    let mut q = sqlx::query_scalar::<_, i64>(&bytes_sql);
    if let Some((_, binds)) = &seg_scope {
        for b in binds {
            q = q.bind(b.clone());
        }
    }
    let recordings_bytes: i64 = q.fetch_one(&st.pool).await?;
    // Recorder ids are camera ids, so an unfiltered count is another fleet-size oracle.
    let active_recorders = match principal.camera_scope() {
        Some(scope) => st
            .recorder
            .active_ids()
            .await
            .iter()
            .filter(|id| scope.contains(*id))
            .count(),
        None => st.recorder.active_ids().await.len(),
    };
    // `storage_report` is fleet-wide by construction, so narrow it here. (An earlier version of this
    // comment justified that by "other callers need it that way" — there are none; this is its only
    // call site. The reason is that its fleet-wide numbers are correct for the unscoped caller, not
    // that anything else depends on them.)
    // `disk` stays: free/total bytes on the volume are a box-level operator fact and disclose nothing
    // per-camera. The footage-derived fields do disclose — the retention horizon
    // (MIN(start_time)/MAX(end_time)) reveals when cameras outside the scope started and last
    // recorded — so a scoped caller gets its OWN footprint and no fleet horizon or projection.
    let mut storage = storage::storage_report(&st.pool, &st.cfg).await?;
    if principal.camera_scope().is_some() {
        storage.recordings_bytes = recordings_bytes;
        storage.segment_count = segments_total;
        storage.oldest_segment = None;
        storage.newest_segment = None;
        storage.write_rate_bytes_per_day = 0;
        storage.projected_days_remaining = None;
    }
    let limits = effective_limits(&st).await;

    // Disk health: the latest disk-health alert (any time) and whether one fired recently (within a
    // few SMART-check cycles). With checks disabled no such events exist, so health reads as OK.
    let last_disk_alert_raw: Option<String> = sqlx::query_scalar(
        "SELECT MAX(timestamp) FROM events WHERE event_type IN ('disk_smart_warning', 'raid_degraded')",
    )
    .fetch_one(&st.pool)
    .await?;
    let last_disk_alert_at = last_disk_alert_raw
        .as_deref()
        .and_then(crate::util::parse_rfc3339);
    let recent_window_s = (st.cfg.smart_check_interval_s.saturating_mul(3)).max(900) as i64;
    let cutoff = Utc::now() - chrono::Duration::seconds(recent_window_s);
    let recent_disk_alerts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events
          WHERE event_type IN ('disk_smart_warning', 'raid_degraded') AND timestamp >= ?",
    )
    .bind(cutoff)
    .fetch_one(&st.pool)
    .await?;

    Ok(Json(SystemInfo {
        name: "Heldar Core",
        version: env!("CARGO_PKG_VERSION"),
        started_at: st.started_at,
        uptime_seconds: (Utc::now() - st.started_at).num_seconds(),
        recorder_enabled: st.cfg.recorder_enabled,
        cameras_total,
        cameras_recording,
        active_recorders,
        segments_total,
        recordings_bytes,
        recordings_gb: recordings_bytes as f64 / 1024.0 / 1024.0 / 1024.0,
        max_recordings_gb: limits.max_recordings_gb,
        storage,
        remote_access: remote_access::status(&st.cfg),
        relay: relay_status(&st),
        disk_health_ok: recent_disk_alerts == 0,
        last_disk_alert_at,
        live_transcode_engine: crate::services::mediamtx::effective_engine(&st.pool, &st.cfg).await,
        enforcement: enforcement_posture(&st),
    }))
}
