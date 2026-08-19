//! Evidence-lock + incident tagging on recorded segments.
//!
//! A durable evidence hold (`segments.evidence_locked`) pins footage so the retention sweeper never
//! prunes it (distinct from the transient `locked` read-lock used by clip/snapshot export, which is
//! wiped at startup). Segments can be tagged with a free-form `incident_id` so evidence can be
//! grouped into a case and reviewed together. Locking/tagging is a manager+ mutation and is written
//! to the immutable audit log; reading the incident roll-up is open to any authenticated principal.

use axum::extract::{Path, State};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::{self, Cap, Principal};
use crate::error::{AppError, AppResult};
use crate::models::Segment;
use crate::repo;
use crate::routes::recordings::SegmentView;
use crate::state::{camera_scope_filter, AppState, CameraOwned};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/segments/{id}/evidence-lock",
            post(lock_evidence).delete(unlock_evidence),
        )
        .route("/api/v1/segments/{id}/incident", patch(tag_incident))
        .route("/api/v1/incidents", get(list_incidents))
        .route(
            "/api/v1/incidents/{incident_id}/segments",
            get(incident_segments),
        )
}

/// Lock body: an optional incident tag to attach when pinning the segment.
#[derive(Debug, Deserialize)]
struct EvidenceLockBody {
    incident_id: Option<String>,
}

/// Tag body: the incident to set, or JSON `null` to clear the tag.
#[derive(Debug, Deserialize)]
struct IncidentTagBody {
    #[serde(default)]
    incident_id: Option<String>,
}

/// Roll-up of one incident: how many segments are tagged to it, their footprint, and span.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct IncidentSummary {
    pub incident_id: String,
    pub segment_count: i64,
    pub total_bytes: i64,
    pub oldest_start: DateTime<Utc>,
    pub newest_end: DateTime<Utc>,
}

/// Trim an optional incident id, treating blank/whitespace as absent (no tag).
fn norm_incident(raw: Option<String>) -> Option<String> {
    raw.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Assert the caller holds the camera owning a segment, WITHOUT disclosing the segment row.
///
/// Every route below is addressed by a bare segment id, so it cannot use `camera_for`. Loading the
/// segment first and then calling `require_camera(&seg.camera_id, …)` would be the naive fix and is
/// wrong twice over: it answers 404 for a missing segment and 403 for another camera's (an id-space
/// oracle), and its message embeds the owning camera id (the fleet roster, one probe at a time).
async fn segment_scope(
    st: &AppState,
    principal: &Principal,
    segment_id: &str,
    action: &str,
) -> AppResult<()> {
    st.resource_camera(principal, CameraOwned::Segment, segment_id, action)
        .await
        .map(|_| ())
}

/// Load a segment by id or 404.
async fn load_segment(pool: &sqlx::SqlitePool, id: &str) -> AppResult<Segment> {
    sqlx::query_as::<_, Segment>("SELECT * FROM segments WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("segment {id} not found")))
}

async fn lock_evidence(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(body): Json<EvidenceLockBody>,
) -> AppResult<Json<SegmentView>> {
    principal.require(principal.can_manage_registry(), "evidence-lock segments")?;
    segment_scope(&st, &principal, &id, "evidence-lock segments").await?;
    let incident_id = norm_incident(body.incident_id);
    repo::set_evidence_locked(&st.pool, &id, true, incident_id.as_deref()).await?;
    auth::audit(
        &st.pool,
        &principal,
        "evidence_lock_segment",
        "segment",
        &id,
        json!({ "incident_id": incident_id }),
    )
    .await;
    let seg = load_segment(&st.pool, &id).await?;
    Ok(Json(SegmentView::new(seg)))
}

async fn unlock_evidence(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
) -> AppResult<Json<SegmentView>> {
    principal.require(principal.can_manage_registry(), "evidence-unlock segments")?;
    // The sharpest of the three: unlocking hands pinned footage back to the retention sweeper for
    // deletion, so it must never be reachable for a camera this credential does not hold.
    segment_scope(&st, &principal, &id, "evidence-unlock segments").await?;
    // incident_id is preserved (COALESCE) so the case tag survives unlocking.
    repo::set_evidence_locked(&st.pool, &id, false, None).await?;
    auth::audit(
        &st.pool,
        &principal,
        "evidence_unlock_segment",
        "segment",
        &id,
        json!({}),
    )
    .await;
    let seg = load_segment(&st.pool, &id).await?;
    Ok(Json(SegmentView::new(seg)))
}

async fn tag_incident(
    State(st): State<AppState>,
    Path(id): Path<String>,
    principal: Principal,
    Json(body): Json<IncidentTagBody>,
) -> AppResult<Json<SegmentView>> {
    principal.require(principal.can_manage_registry(), "tag segment incidents")?;
    segment_scope(&st, &principal, &id, "tag segment incidents").await?;
    let incident_id = norm_incident(body.incident_id);
    // Direct set/clear (not COALESCE): a null/blank tag clears the association.
    sqlx::query("UPDATE segments SET incident_id = ? WHERE id = ?")
        .bind(&incident_id)
        .bind(&id)
        .execute(&st.pool)
        .await?;
    auth::audit(
        &st.pool,
        &principal,
        "tag_segment_incident",
        "segment",
        &id,
        json!({ "incident_id": incident_id }),
    )
    .await;
    let seg = load_segment(&st.pool, &id).await?;
    Ok(Json(SegmentView::new(seg)))
}

async fn list_incidents(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<Vec<IncidentSummary>>> {
    principal.require_cap(Cap::EventsRead, "list incidents")?;
    // `EventsRead` is nominally unscopable (UNSCOPABLE_CAPS refuses it at MINT time), but that rule is
    // not applied at RUNTIME by `resolve_key_scope`, so a pre-existing key row carrying both a camera
    // scope and this cap is live. Filter regardless: `camera_scope_filter` is `None` — and this query
    // therefore unchanged — for every unscoped credential.
    let scope = camera_scope_filter(&principal, "camera_id");
    let mut sql = String::from(
        "SELECT incident_id,
                COUNT(*) AS segment_count,
                COALESCE(SUM(size_bytes), 0) AS total_bytes,
                MIN(start_time) AS oldest_start,
                MAX(end_time) AS newest_end
         FROM segments
         WHERE incident_id IS NOT NULL",
    );
    if let Some((pred, _)) = &scope {
        sql.push_str(pred);
    }
    sql.push_str(
        " GROUP BY incident_id
         ORDER BY newest_end DESC
         LIMIT 1000",
    );
    let mut query = sqlx::query_as::<_, IncidentSummary>(&sql);
    if let Some((_, binds)) = &scope {
        for b in binds {
            query = query.bind(b);
        }
    }
    let rows = query.fetch_all(&st.pool).await?;
    Ok(Json(rows))
}

/// Defensive upper bound on segments returned for one incident roll-up. Generous (an incident is a
/// handful of pinned clips in practice); a hit is logged so truncation is never silent.
const INCIDENT_SEGMENTS_CAP: i64 = 5000;

async fn incident_segments(
    State(st): State<AppState>,
    Path(incident_id): Path<String>,
    principal: Principal,
) -> AppResult<Json<Vec<SegmentView>>> {
    principal.require_cap(Cap::VideoPlayback, "read incident segments")?;
    // An incident id is guessable and is echoed by the segment routes above; without this filter it
    // yields another camera's segment rows, file paths and timestamps. Bind order matters: the
    // predicate is spliced between `incident_id = ?` and the `LIMIT ?`.
    let scope = camera_scope_filter(&principal, "camera_id");
    let mut sql = String::from("SELECT * FROM segments WHERE incident_id = ?");
    if let Some((pred, _)) = &scope {
        sql.push_str(pred);
    }
    sql.push_str(" ORDER BY start_time ASC LIMIT ?");
    let mut query = sqlx::query_as::<_, Segment>(&sql).bind(&incident_id);
    if let Some((_, binds)) = &scope {
        for b in binds {
            query = query.bind(b);
        }
    }
    let segments = query
        .bind(INCIDENT_SEGMENTS_CAP)
        .fetch_all(&st.pool)
        .await?;
    if segments.len() as i64 >= INCIDENT_SEGMENTS_CAP {
        tracing::warn!(
            incident_id = %incident_id,
            cap = INCIDENT_SEGMENTS_CAP,
            "incident segment query hit the row cap; results may be truncated"
        );
    }
    let views = segments.into_iter().map(SegmentView::new).collect();
    Ok(Json(views))
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

    /// One camera plus one evidence-locked segment tagged to `incident`.
    async fn seed(pool: &sqlx::SqlitePool, camera_id: &str, segment_id: &str, incident: &str) {
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
            "INSERT INTO segments
               (id, camera_id, path, start_time, end_time, duration_s, size_bytes,
                evidence_locked, incident_id, created_at)
             VALUES (?,?,?,?,?,60.0,1024,1,?,?)",
        )
        .bind(segment_id)
        .bind(camera_id)
        .bind(format!("/recordings/{camera_id}/{segment_id}.mp4"))
        .bind(now)
        .bind(now)
        .bind(incident)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
    }

    /// Unlocking another camera's evidence hands its pinned footage back to the retention sweeper.
    /// It must be refused — and refused identically to a segment id that never existed.
    #[tokio::test]
    async fn evidence_unlock_is_refused_and_is_not_an_existence_oracle() {
        let st = test_state().await;
        seed(&st.pool, "cam_a", "seg_a", "inc_1").await;
        seed(&st.pool, "cam_sentinel_b", "seg_b", "inc_1").await;
        let p = scoped(&["cam_a"]);

        let out_of_scope = unlock_evidence(State(st.clone()), Path("seg_b".into()), p.clone())
            .await
            .unwrap_err();
        let nonexistent = unlock_evidence(State(st.clone()), Path("seg_zzz".into()), p.clone())
            .await
            .unwrap_err();
        assert!(matches!(out_of_scope, AppError::Forbidden(_)));
        assert_eq!(out_of_scope.to_string(), nonexistent.to_string());
        assert!(!out_of_scope.to_string().contains("cam_sentinel_b"));
        assert!(!out_of_scope.to_string().contains("seg_b"));

        // The hold really survived.
        let locked: i64 =
            sqlx::query_scalar("SELECT evidence_locked FROM segments WHERE id = 'seg_b'")
                .fetch_one(&st.pool)
                .await
                .unwrap();
        assert_eq!(locked, 1, "another camera's evidence hold must survive");

        // Its own camera is unaffected.
        assert!(unlock_evidence(State(st.clone()), Path("seg_a".into()), p)
            .await
            .is_ok());
    }

    /// Retagging is the same shape: it can clear another operator's case association.
    #[tokio::test]
    async fn incident_tagging_is_scoped() {
        let st = test_state().await;
        seed(&st.pool, "cam_sentinel_b", "seg_b", "inc_1").await;
        let body = || IncidentTagBody { incident_id: None };
        let err = tag_incident(
            State(st.clone()),
            Path("seg_b".into()),
            scoped(&["cam_a"]),
            Json(body()),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::Forbidden(_)), "got {err:?}");
        // An unscoped principal still clears the tag exactly as before.
        assert!(tag_incident(
            State(st.clone()),
            Path("seg_b".into()),
            Principal::system_admin(),
            Json(body()),
        )
        .await
        .is_ok());
    }

    /// An incident id is guessable, so the roll-up and its segment list must both be camera-filtered.
    #[tokio::test]
    async fn incident_rollups_are_camera_filtered() {
        let st = test_state().await;
        seed(&st.pool, "cam_a", "seg_a", "inc_1").await;
        seed(&st.pool, "cam_sentinel_b", "seg_b", "inc_1").await;
        seed(&st.pool, "cam_sentinel_c", "seg_c", "inc_2").await;
        let p = scoped(&["cam_a"]);

        let Json(segments) = incident_segments(State(st.clone()), Path("inc_1".into()), p.clone())
            .await
            .unwrap();
        assert_eq!(segments.len(), 1, "only the in-scope segment");
        let body = serde_json::to_string(&segments).unwrap();
        assert!(!body.contains("cam_sentinel_b"), "{body}");

        let Json(rollup) = list_incidents(State(st.clone()), p).await.unwrap();
        assert_eq!(rollup.len(), 1, "inc_2 belongs entirely to another camera");
        assert_eq!(rollup[0].incident_id, "inc_1");
        assert_eq!(rollup[0].segment_count, 1, "cam_b's segment is not counted");

        // Unscoped: the full fleet-wide view, unchanged.
        let Json(all) = list_incidents(State(st.clone()), Principal::system_admin())
            .await
            .unwrap();
        assert_eq!(all.len(), 2);
        let Json(all_segs) = incident_segments(
            State(st.clone()),
            Path("inc_1".into()),
            Principal::system_admin(),
        )
        .await
        .unwrap();
        assert_eq!(all_segs.len(), 2);
    }
}
