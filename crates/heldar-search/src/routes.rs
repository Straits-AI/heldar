//! Semantic-search HTTP surface: structured search, natural-language search (plan → execute → proof),
//! and a plan dry-run. Reads need can_view; every search is logged (search_log) and identity-bearing
//! queries are audited (kernel audit_log). Answers are the executed query's rows — never model output.

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Extension, State};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use heldar_kernel::auth::{self, Cap, Principal};
use heldar_kernel::error::{AppError, AppResult};
use heldar_kernel::state::{camera_scope_filter, confine_camera_ids, AppState, CameraOwned};

use crate::config::SearchConfig;
use crate::query::{self, QueryPlan};

pub fn router(cfg: Arc<SearchConfig>) -> Router<AppState> {
    Router::new()
        .route("/api/v1/search/events", post(search_events))
        .route("/api/v1/search/nl", post(search_nl))
        .route("/api/v1/search/plan", post(plan_only))
        // Similarity retrieval over stored crop embeddings (issue #38). The body can carry a query
        // image as base64, so this route gets its own pre-deserialization cap.
        //
        // It is registered through `search_semantic_scoped`, NOT through `semantic::search_semantic`
        // directly, so that this route joins the same camera-confinement path as its three siblings
        // instead of taking `body.cameras` verbatim. See that wrapper for why the confinement lives
        // here rather than inside the handler.
        .route(
            "/api/v1/search/semantic",
            post(search_semantic_scoped).layer(DefaultBodyLimit::max(
                crate::semantic::SEMANTIC_BODY_LIMIT_BYTES,
            )),
        )
        .route("/api/v1/modules/search/ui/index.js", get(serve_ui))
        .layer(Extension(cfg))
}

// ---- Camera scope ---------------------------------------------------------
//
// Search reads across every fact table on the box, so its `cameras` field is the ONLY thing standing
// between a camera-scoped credential and the whole fleet's history. Today that field is pure caller
// convenience — it reads like a scope and is not one. The two helpers below make it one.
//
// Note the trap both of them exist to close: every executor downstream treats an EMPTY camera list as
// "no filter", i.e. all cameras (`query.rs` `camset.is_empty()`, `semantic.rs`
// `(!cameras.is_empty()).then(...)`). So for a scoped caller an empty list must never survive to the
// executor — including the degenerate empty-allowlist credential, which must find nothing rather than
// everything. Both helpers are the identity for `Scope::All`, so every human role, every key minted
// without a camera list, and the auth-disabled LAN default are unaffected by construction.

/// Confine a CALLER-SUPPLIED camera list (the structured plan's `cameras`, the semantic body's).
///
/// Delegates to the kernel's `confine_camera_ids` — verbatim for an unscoped caller, expanded from
/// empty to the credential's own scope for a scoped one, and refused whole rather than silently
/// narrowed when the caller names a camera it does not hold.
fn confine_requested_cameras(
    principal: &Principal,
    requested: &[String],
) -> AppResult<Vec<String>> {
    let confined = confine_camera_ids(principal, requested)?;
    if principal.camera_scope().is_some() && confined.is_empty() {
        // Only reachable for a credential scoped to the empty set. Refuse rather than hand the
        // executor an empty list, which it would read as "every camera".
        return Err(AppError::Forbidden(
            "credential is not scoped to any camera (cannot search)".to_string(),
        ));
    }
    Ok(confined)
}

/// Confine a PLANNER-PRODUCED camera list (the rule parser's or the LLM's).
///
/// Unlike a caller-supplied list this is not a request, so an out-of-scope entry is the planner's
/// guess rather than the caller's demand and is dropped rather than refused — an LLM naming a camera
/// that does not exist must not turn a question into a 403. When nothing survives (or the planner
/// named no camera at all) a scoped caller falls back to its OWN scope, never to "all".
fn confine_planned_cameras(principal: &Principal, planned: &[String]) -> AppResult<Vec<String>> {
    let Some(scope) = principal.camera_scope() else {
        return Ok(planned.to_vec());
    };
    if scope.is_empty() {
        // The degenerate credential again: there is no non-empty list to fall back to, and an empty
        // one would read as "every camera" downstream. Same refusal as the caller-supplied path.
        return Err(AppError::Forbidden(
            "credential is not scoped to any camera (cannot search)".to_string(),
        ));
    }
    let kept: Vec<String> = planned
        .iter()
        .filter(|c| scope.contains(*c))
        .cloned()
        .collect();
    if !kept.is_empty() {
        return Ok(kept);
    }
    let mut all: Vec<String> = scope.iter().cloned().collect();
    all.sort();
    Ok(all)
}

/// The built search module UI bundle, embedded at compile time (regenerate with `make module-bundles`
/// after editing `apps/web/src/modules/search`). It imports React + the shell SDK (`@heldar/shell`) as
/// bare specifiers the dashboard's import map resolves — so this crate ships only the module's own code.
const SEARCH_UI_BUNDLE: &str = include_str!("../ui/search.js");

/// Serve the runtime-loaded search module UI (the dashboard imports it via `ModuleHost`). Any
/// authenticated viewer may load it — it is inert frontend code; the data it fetches is separately
/// gated by the kernel's RBAC.
async fn serve_ui(principal: Principal) -> AppResult<axum::response::Response> {
    principal.require_cap(Cap::EventsRead, "load the search module UI")?;
    Ok((
        [
            (
                axum::http::header::CONTENT_TYPE,
                "text/javascript; charset=utf-8",
            ),
            // Stable URL; the bundle changes only on redeploy — revalidate so a kernel rebuild never
            // serves a stale module UI from the browser's heuristic cache.
            (axum::http::header::CACHE_CONTROL, "no-cache"),
        ],
        SEARCH_UI_BUNDLE,
    )
        .into_response())
}

/// The roster handed to the planner so it can turn "the loading dock camera" into a camera id.
///
/// It is a full inventory of names AND ids, so for a scoped credential it is filtered — otherwise the
/// planner's echoed plan is a roster leak wearing a helper's clothes, and it supplies exactly the ids
/// every camera-keyed route elsewhere takes as input. `camera_scope_filter` returns `None` for an
/// unscoped caller, so the query below is byte-identical to today's for every human role.
async fn cameras(pool: &sqlx::SqlitePool, principal: &Principal) -> Vec<(String, String)> {
    let scope = camera_scope_filter(principal, "id");
    let mut sql = "SELECT id, name FROM cameras WHERE 1=1".to_string();
    if let Some((pred, _)) = &scope {
        sql.push_str(pred);
    }
    let mut q = sqlx::query_as::<_, (String, String)>(&sql);
    // Bind from the RETURNED vector: the empty-allowlist arm is `" AND 0"` with ZERO binds.
    for id in scope.iter().flat_map(|(_, ids)| ids) {
        q = q.bind(id);
    }
    q.fetch_all(pool).await.unwrap_or_default()
}

/// The plate an identity-bearing query targets, if any — covers both the `plate` field AND a `text`
/// filter that resolves to a plausible plate (the text channel matches `hit.plate`, so it is an
/// identity lookup too and must be audited).
fn identity_plate(plan: &QueryPlan) -> Option<String> {
    if let Some(p) = &plan.plate {
        return Some(p.clone());
    }
    if let Some(t) = &plan.text {
        let n = crate::planner::norm_plate(t);
        if crate::planner::plausible_plate(&n) {
            return Some(n);
        }
    }
    None
}

pub(crate) async fn log_search(
    st: &AppState,
    principal: &Principal,
    mode: &str,
    query_text: Option<&str>,
    plan: &QueryPlan,
    planner: &str,
    count: usize,
) {
    let _ = sqlx::query(
        "INSERT INTO search_log (id, actor, mode, query_text, plan, planner, result_count, created_at)
         VALUES (?,?,?,?,?,?,?,?)",
    )
    .bind(format!("sl_{}", Uuid::new_v4().simple()))
    .bind(&principal.id)
    .bind(mode)
    .bind(query_text)
    .bind(sqlx::types::Json(plan))
    .bind(planner)
    .bind(count as i64)
    .bind(Utc::now())
    .execute(&st.pool)
    .await;
    // Identity accountability: a plate-targeted query (via `plate` OR a plate-like `text`) is audited.
    if let Some(plate) = identity_plate(plan) {
        auth::audit(
            &st.pool,
            principal,
            "search_identity_query",
            "plate",
            &plate,
            json!({ "mode": mode, "query": query_text }),
        )
        .await;
    }
}

/// The zone a plan's hour filter and relative dates are read in, and where it came from (#125).
///
/// Order: an explicit `plan.tz`, then the single site the plan's cameras belong to, then the
/// box-wide default, then UTC — search's historical behaviour, so an unconfigured box is unchanged.
///
/// A PLAN SPANNING SITES IN DIFFERENT ZONES IS REFUSED rather than resolved. Picking one site's
/// zone for another's cameras is the failure this whole issue is about, and it would be invisible:
/// the results look plausible, just shifted. The caller is told to say which zone it meant.
async fn resolve_plan_tz(
    st: &AppState,
    plan: &QueryPlan,
) -> AppResult<(chrono_tz::Tz, &'static str)> {
    if let Some(raw) = plan.tz.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let tz = heldar_kernel::services::tz::parse(raw).ok_or_else(|| {
            AppError::BadRequest(format!(
                "`tz` must be an IANA timezone identifier such as `Asia/Kuala_Lumpur` (got {raw:?})"
            ))
        })?;
        return Ok((tz, "explicit"));
    }

    // Every distinct zone among the plan's cameras. An empty camera list means the whole fleet, in
    // which case there is no single site to ask and the box default applies.
    let mut zones: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for cam in &plan.cameras {
        let (tz, _) = heldar_kernel::services::tz::site_tz(&st.pool, Some(cam)).await;
        if let Some(tz) = tz {
            zones.insert(tz.to_string());
        }
    }
    if zones.len() > 1 {
        return Err(AppError::BadRequest(format!(
            "this query spans cameras in different timezones ({}). A relative time means something \
             different at each, so pass an explicit `tz`, or search one site at a time — resolving \
             it silently would return plausible results shifted by hours.",
            zones.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    if let Some(one) = zones.into_iter().next() {
        if let Some(tz) = heldar_kernel::services::tz::parse(&one) {
            return Ok((tz, "site"));
        }
    }
    let (tz, _) = heldar_kernel::services::tz::site_tz(&st.pool, None).await;
    match tz {
        Some(tz) => Ok((tz, "default")),
        // Search has always read hours in UTC. Keeping that as the unconfigured fallback is what
        // stops this change from silently moving every saved query on every box.
        None => Ok((chrono_tz::Tz::UTC, "utc_fallback")),
    }
}

async fn search_events(
    State(st): State<AppState>,
    principal: Principal,
    Extension(cfg): Extension<Arc<SearchConfig>>,
    Json(plan): Json<QueryPlan>,
) -> AppResult<Json<Value>> {
    principal.require_cap(Cap::EventsRead, "search events")?;
    // Sanitize the caller-supplied plan (clamp out-of-range hours, etc.) before executing — the same
    // guard applied to LLM-produced plans — so a hand-crafted QueryPlan can't smuggle invalid filters.
    let mut plan = crate::planner::sanitize(plan);
    // ...and confine its `cameras` field, which until now was pure caller convenience: the executor
    // reads an empty list as "every camera", so an unconfined plan is the whole fleet's history.
    // The plan is echoed back in the response, so this also keeps the echo honest.
    plan.cameras = confine_requested_cameras(&principal, &plan.cameras)?;
    let (tz, tz_source) = resolve_plan_tz(&st, &plan).await?;
    // Echo the RESOLVED zone in the plan itself, so the logged plan and the response agree about
    // which clock the answer was computed on. A plan logged without it cannot be re-run.
    plan.tz = Some(tz.to_string());
    let outcome = query::execute_in(&st.pool, &plan, cfg.max_results, tz).await?;
    log_search(
        &st,
        &principal,
        "structured",
        None,
        &plan,
        "structured",
        outcome.hits.len(),
    )
    .await;
    Ok(Json(response(
        None,
        "structured",
        &plan,
        outcome,
        tz,
        tz_source,
    )))
}

#[derive(Debug, Deserialize)]
struct NlBody {
    query: String,
}

async fn search_nl(
    State(st): State<AppState>,
    principal: Principal,
    Extension(cfg): Extension<Arc<SearchConfig>>,
    Json(body): Json<NlBody>,
) -> AppResult<Json<Value>> {
    principal.require_cap(Cap::EventsRead, "natural-language search")?;
    let q = body.query.trim();
    if q.is_empty() {
        return Err(heldar_kernel::error::AppError::BadRequest(
            "`query` is required".into(),
        ));
    }
    let cams = cameras(&st.pool, &principal).await;
    // LLM planner if configured, else the transparent rule parser. The LLM only PLANS.
    let (mut plan, planner) = match crate::planner::plan_llm(&st.http, &cfg, q, &cams).await {
        Some(p) => (crate::planner::sanitize(p), "llm"),
        None => {
            // The rule parser needs the zone BEFORE it can turn "yesterday" into a window, so
            // resolve from an empty plan first: explicit tz cannot apply here (there is no plan
            // yet), so this is the site-or-default-or-UTC answer.
            let (pre_tz, _) = resolve_plan_tz(&st, &QueryPlan::default()).await?;
            (crate::planner::parse_rules_in(q, &cams, pre_tz), "rules")
        }
    };
    // The roster above already confined what the planner could name, but the LLM is free to invent an
    // id, so the plan is confined again before it executes — and, because it is echoed, before it is
    // shown. Planner output is narrowed rather than refused; see `confine_planned_cameras`.
    plan.cameras = confine_planned_cameras(&principal, &plan.cameras)?;
    // A MODEL'S TYPO MUST NOT FAIL AN OPERATOR'S QUESTION. On the structured route an unparseable
    // `tz` is a 400, because a caller sent it deliberately. Here the planner may have invented one,
    // so it is cleared and resolution falls through to the site — the answer is still computed in a
    // real zone, and the response says which.
    if plan
        .tz
        .as_deref()
        .is_some_and(|t| heldar_kernel::services::tz::parse(t).is_none())
    {
        plan.tz = None;
    }
    let (tz, tz_source) = resolve_plan_tz(&st, &plan).await?;
    plan.tz = Some(tz.to_string());
    let outcome = query::execute_in(&st.pool, &plan, cfg.max_results, tz).await?;
    log_search(
        &st,
        &principal,
        "nl",
        Some(q),
        &plan,
        planner,
        outcome.hits.len(),
    )
    .await;
    Ok(Json(response(
        Some(q),
        planner,
        &plan,
        outcome,
        tz,
        tz_source,
    )))
}

async fn plan_only(
    State(st): State<AppState>,
    principal: Principal,
    Extension(cfg): Extension<Arc<SearchConfig>>,
    Json(body): Json<NlBody>,
) -> AppResult<Json<Value>> {
    // Dry-run: show how a question is interpreted (no execution, no data) — useful for trust/debug.
    principal.require_cap(Cap::EventsRead, "plan a search")?;
    let q = body.query.trim();
    if q.is_empty() {
        return Err(heldar_kernel::error::AppError::BadRequest(
            "`query` is required".into(),
        ));
    }
    let cams = cameras(&st.pool, &principal).await;
    let (mut plan, planner) = match crate::planner::plan_llm(&st.http, &cfg, q, &cams).await {
        Some(p) => (crate::planner::sanitize(p), "llm"),
        None => (
            crate::planner::parse_rules(body.query.trim(), &cams),
            "rules",
        ),
    };
    // Confined identically to `search_nl`, so the dry-run shows the plan that WOULD run rather than
    // one the executor will then narrow — a plan you cannot trust is worse than no dry-run.
    plan.cameras = confine_planned_cameras(&principal, &plan.cameras)?;
    Ok(Json(
        json!({ "query": body.query, "planner": planner, "plan": plan }),
    ))
}

/// `POST /api/v1/search/semantic`, brought onto the same camera-confinement path as its three
/// siblings.
///
/// The confinement lives in this wrapper rather than inside `semantic::search_semantic` because the
/// route table is the thing an auditor reads: a handler registered here without a visible
/// confinement step is exactly how this route came to be the one search surface that took its
/// `cameras` field verbatim. Two things are closed:
///
/// 1. `body.cameras` is confined like any other caller-supplied list. It matters more here than on
///    the structured routes: `SimilarFilters.cameras` is `None` when the list is empty, i.e. the
///    whole box's stored crop embeddings.
/// 2. `body.zone` is resolved through the kernel's `resource_camera` loader FIRST. Downstream,
///    `embeddings::resolve_zone_scope` is a bare `SELECT … FROM zones WHERE id = ?` that then pivots
///    the search onto the zone's owning camera — a zone id is therefore a zone→camera oracle, and a
///    way to search a camera you do not hold without ever naming it. `resource_camera` refuses an
///    out-of-scope zone and a nonexistent zone with the SAME error value, so the zone id space cannot
///    be enumerated either. It runs only for a scoped principal, so an unscoped caller keeps today's
///    exact behaviour, 404 wording included.
async fn search_semantic_scoped(
    State(st): State<AppState>,
    principal: Principal,
    Extension(cfg): Extension<Arc<SearchConfig>>,
    Json(mut body): Json<crate::semantic::SemanticBody>,
) -> AppResult<Json<Value>> {
    if principal.camera_scope().is_some() {
        if let Some(zid) = body
            .zone
            .as_deref()
            .map(str::trim)
            .filter(|z| !z.is_empty())
        {
            st.resource_camera(&principal, CameraOwned::Zone, zid, "run a semantic search")
                .await?;
        }
    }
    body.cameras = confine_requested_cameras(&principal, &body.cameras)?;
    crate::semantic::search_semantic(State(st), principal, Extension(cfg), Json(body)).await
}

fn response(
    query: Option<&str>,
    planner: &str,
    plan: &QueryPlan,
    outcome: query::ExecOutcome,
    tz: chrono_tz::Tz,
    tz_source: &str,
) -> Value {
    let proof = crate::proof::build(query, planner, plan, &outcome.hits, outcome.truncated);
    json!({
        "query": query,
        "planner": planner,
        "plan": plan,
        // WHICH CLOCK THIS ANSWER WAS COMPUTED ON (#125). An operator cannot tell a correct result
        // from one shifted by eight hours by looking at it, so the interpretation is stated rather
        // than left to be assumed.
        "interpretation": {
            "timezone": tz.to_string(),
            "timezone_source": tz_source,
            "hour_filter_read_in": tz.to_string(),
            "note": "hour_min/hour_max and relative dates are read in this zone; stored timestamps \
                     and every timestamp below are UTC.",
        },
        "count": outcome.hits.len(),
        // Honest signal: the field-filtered result set may omit older in-window matches because a
        // source hit its fetch cap. Clients (and the proof layer) must not treat the count as complete.
        "truncated": outcome.truncated,
        "hits": outcome.hits,
        "proof": proof,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use heldar_kernel::auth::Scope;
    use std::collections::HashSet;

    /// A camera-scoped credential holding every capability. Only `scope` differs from the
    /// auth-disabled system admin, so any behaviour difference below is attributable to camera scope.
    fn scoped(cameras: &[&str]) -> Principal {
        let set: HashSet<String> = cameras.iter().map(|c| c.to_string()).collect();
        Principal {
            scope: Scope::Cameras(Arc::new(set)),
            ..Principal::system_admin()
        }
    }

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn confinement_is_the_identity_for_an_unscoped_credential() {
        // CONSTRAINT 1 + 2: the auth-disabled default and every human role search exactly as today,
        // INCLUDING the empty list, which for them still means "every camera".
        let admin = Principal::system_admin();
        assert_eq!(
            confine_requested_cameras(&admin, &[]).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            confine_requested_cameras(&admin, &v(&["cam_SENTINEL_B"])).unwrap(),
            v(&["cam_SENTINEL_B"])
        );
        assert_eq!(
            confine_planned_cameras(&admin, &[]).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            confine_planned_cameras(&admin, &v(&["cam_SENTINEL_B"])).unwrap(),
            v(&["cam_SENTINEL_B"])
        );
    }

    #[test]
    fn an_empty_request_never_means_every_camera_for_a_scoped_credential() {
        // This is the whole trap: every executor downstream reads an empty list as "no filter".
        let p = scoped(&["cam_a", "cam_c"]);
        assert_eq!(
            confine_requested_cameras(&p, &[]).unwrap(),
            v(&["cam_a", "cam_c"])
        );
        assert_eq!(
            confine_planned_cameras(&p, &[]).unwrap(),
            v(&["cam_a", "cam_c"])
        );
    }

    #[test]
    fn a_requested_camera_outside_the_scope_is_refused_and_named_in_no_message() {
        let p = scoped(&["cam_a"]);
        assert_eq!(
            confine_requested_cameras(&p, &v(&["cam_a"])).unwrap(),
            v(&["cam_a"])
        );
        for asked in [
            v(&["cam_SENTINEL_B"]),
            v(&["cam_a", "cam_SENTINEL_B"]),
            // A camera that does not exist at all is refused identically to one that does but is
            // held by someone else — the request cannot be used to probe the roster.
            v(&["cam_zzz"]),
        ] {
            let err = confine_requested_cameras(&p, &asked).unwrap_err();
            assert!(
                matches!(err, AppError::Forbidden(_)),
                "{asked:?} -> {err:?}"
            );
            assert!(!err.to_string().contains("cam_SENTINEL_B"));
            assert!(!err.to_string().contains("cam_zzz"));
        }
    }

    #[test]
    fn a_planned_camera_outside_the_scope_is_dropped_rather_than_refused() {
        // Planner output is the model's guess, not the caller's demand: an invented id must not turn
        // a question into a 403. What it must never do is widen the search.
        let p = scoped(&["cam_a", "cam_c"]);
        assert_eq!(
            confine_planned_cameras(&p, &v(&["cam_a", "cam_SENTINEL_B"])).unwrap(),
            v(&["cam_a"])
        );
        // Nothing survived: fall back to the credential's OWN scope, never to "all".
        assert_eq!(
            confine_planned_cameras(&p, &v(&["cam_SENTINEL_B", "cam_zzz"])).unwrap(),
            v(&["cam_a", "cam_c"])
        );
    }

    #[test]
    fn the_degenerate_empty_scope_finds_nothing_rather_than_everything() {
        // A credential scoped to the empty set has no non-empty list to fall back to, and an empty
        // one would be read downstream as "every camera" — so both paths refuse, identically.
        let none = scoped(&[]);
        let a = confine_requested_cameras(&none, &[]).unwrap_err();
        let b = confine_planned_cameras(&none, &[]).unwrap_err();
        assert!(matches!(a, AppError::Forbidden(_)), "got {a:?}");
        assert_eq!(a.to_string(), b.to_string());
    }
}
