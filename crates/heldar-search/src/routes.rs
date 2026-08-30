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
#[utoipa::path(
    get, path = "/api/v1/modules/search/ui/index.js", tag = "search",
    operation_id = "getSearchModuleUi",
    responses(
        (status = 200, description = "The module UI bundle, as `text/javascript`"),
        (status = 403, description = "Missing `events:read`", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn serve_ui(principal: Principal) -> AppResult<axum::response::Response> {
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

/// Does this plan's answer actually depend on which clock it is read on?
///
/// Only hour filters and the relative windows the planner produces are wall-clock notions. An
/// absolute RFC3339 range with no hour filter means the same instants in every zone, so a zone
/// disagreement among its cameras is irrelevant and refusing it would be obstruction, not safety.
///
/// This gate is what keeps the cross-zone refusal from permanently breaking a camera-scoped
/// credential whose cameras happen to span two sites: a plain plate lookup does not care.
fn zone_dependent(plan: &QueryPlan) -> bool {
    plan.hour_min.is_some() || plan.hour_max.is_some()
}

/// The EFFECTIVE zone of every camera the plan will read, as a set.
///
/// Every camera contributes exactly one entry, using the same fallback chain a single-camera query
/// would get: its site, then the box default, then UTC. That matters in both directions —
///
/// - a camera with NO site must not silently inherit a sibling's zone, so it contributes its own
///   fallback rather than nothing;
/// - a camera with no site must not manufacture a disagreement either, so it contributes the box
///   default rather than a distinct "unset" marker.
///
/// An empty camera list means the whole fleet, which is a strict superset of any list — so it is
/// expanded rather than treated as "no cameras", which is how the refusal was bypassable.
/// The zone a plan's hour filter and relative dates are read in, and where it came from (#125).
///
/// `explicit` is the zone the CALLER supplied — not whatever happens to be sitting in `plan.tz`.
/// Those are different once a planner has written into the plan, and conflating them was a real
/// bug: the natural-language route resolved a zone, stamped it onto the plan, then re-resolved and
/// saw its own value as "explicit". The site was never consulted on that route at all, and both the
/// response and the `search_log` accountability row asserted a provenance the caller never gave.
///
/// Order: the caller's zone, then the single zone shared by every camera the plan reads, then the
/// box default, then UTC — search's historical behaviour, so an unconfigured box is unchanged.
async fn resolve_tz(
    st: &AppState,
    explicit: Option<&str>,
    plan: &QueryPlan,
) -> AppResult<(chrono_tz::Tz, &'static str)> {
    if let Some(raw) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        let tz = heldar_kernel::services::tz::parse(raw).ok_or_else(|| {
            AppError::BadRequest(format!(
                "`tz` must be an IANA timezone identifier such as `Asia/Kuala_Lumpur` (got {raw:?})"
            ))
        })?;
        return Ok((tz, "explicit"));
    }

    // One implementation, in the kernel, so search and the entry reports cannot drift into
    // disagreeing about what "yesterday" means on the same box.
    let (zones, from_site) = heldar_kernel::services::tz::zones_for(&st.pool, &plan.cameras).await;
    if zones.len() > 1 {
        // ONLY when the answer actually depends on the clock. Refusing an absolute-window plate
        // lookup because the credential's cameras sit in two sites is obstruction: nothing about
        // that query is zone-dependent, and the caller has no way to make it agree.
        if zone_dependent(plan) {
            return Err(AppError::BadRequest(format!(
                "this query filters by time of day across cameras in different timezones ({}). \
                 A wall-clock hour means something different at each, so pass an explicit `tz`, or \
                 search one site at a time — resolving it silently would return plausible results \
                 shifted by hours.",
                zones.into_iter().collect::<Vec<_>>().join(", ")
            )));
        }
        // Not zone-dependent: any zone gives the same answer. Say UTC rather than pick a site's.
        return Ok((chrono_tz::Tz::UTC, "not_time_of_day"));
    }

    if let Some(one) = zones.into_iter().next() {
        if let Some(tz) = heldar_kernel::services::tz::parse(&one) {
            // The source is what the cameras actually reported, not something inferred by comparing
            // the value to the box default — a site that deliberately chose the same zone as the
            // default is still a site's choice, and an unconfigured box falling back to UTC is not
            // a site at all.
            let (boxwide, _) = heldar_kernel::services::tz::site_tz(&st.pool, None).await;
            let src = if from_site {
                "site"
            } else if boxwide.is_some() {
                "default"
            } else {
                "utc_fallback"
            };
            return Ok((tz, src));
        }
    }

    Ok((chrono_tz::Tz::UTC, "utc_fallback"))
}

/// Execute a structured query plan against the box's stored event facts.
///
/// An empty `cameras` means every camera — but for a camera-scoped credential it is expanded to
/// that credential's own cameras, and naming a camera it does not hold is refused outright (403)
/// rather than silently narrowed. The plan is echoed back with `tz` set to the zone the answer was
/// actually computed in; a time-of-day filter (`hour_min`/`hour_max`) spanning cameras in different
/// timezones is a 400 unless the caller supplies `tz`, because a wall-clock hour means something
/// different at each.
#[utoipa::path(
    post, path = "/api/v1/search/events", tag = "search",
    operation_id = "searchEvents",
    request_body = crate::query::QueryPlan,
    responses(
        (status = 200, description = "The executed plan, its resolved timezone, the matching hits and a proof block"),
        (status = 400, description = "Invalid `tz`, or a time-of-day filter across cameras in different timezones", body = heldar_kernel::openapi::ErrorBody),
        (status = 403, description = "Missing `events:read`, or a camera requested that this credential does not hold", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn search_events(
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
    // On THIS route `plan.tz` really is the caller's — they sent the plan.
    let caller_tz = plan.tz.clone();
    let (tz, tz_source) = resolve_tz(&st, caller_tz.as_deref(), &plan).await?;
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

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct NlBody {
    /// The question, in plain language. Must be non-empty.
    pub query: String,
}

/// Answer a natural-language question: plan it into a structured query, execute that plan, return
/// the rows.
///
/// The planner (an LLM when configured, otherwise a transparent rule parser) only ever produces the
/// plan — the answer is the executed query's rows, never model output, and the plan is echoed so it
/// can be checked or re-run. A camera the planner names but the credential does not hold is dropped
/// rather than refused (an invented id must not turn a question into a 403); when nothing survives,
/// a scoped credential falls back to its own cameras, never to "all". Use `/api/v1/search/plan` to
/// see the interpretation without executing it.
#[utoipa::path(
    post, path = "/api/v1/search/nl", tag = "search",
    operation_id = "searchNaturalLanguage",
    request_body = NlBody,
    responses(
        (status = 200, description = "The plan the question became, its resolved timezone, the matching hits and a proof block"),
        (status = 400, description = "Empty `query`, or a time-of-day question across cameras in different timezones", body = heldar_kernel::openapi::ErrorBody),
        (status = 403, description = "Missing `events:read`, or a credential scoped to no camera", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn search_nl(
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
    // TWO PASSES, because the zone depends on the cameras and the cameras come from parsing.
    // The first pass exists only to learn which cameras the query names; its date windows are
    // discarded. Resolving from an EMPTY plan instead — which is what this did first — meant the
    // site was never consulted on this route at all.
    let (mut plan, planner) = match crate::planner::plan_llm(&st.http, &cfg, q, &cams).await {
        Some(p) => (crate::planner::sanitize(p), "llm"),
        None => (
            crate::planner::parse_rules_in(q, &cams, chrono_tz::Tz::UTC),
            "rules",
        ),
    };
    // The roster above already confined what the planner could name, but the LLM is free to invent an
    // id, so the plan is confined again before it executes — and, because it is echoed, before it is
    // shown. Planner output is narrowed rather than refused; see `confine_planned_cameras`.
    plan.cameras = confine_planned_cameras(&principal, &plan.cameras)?;
    // A PLANNER'S ZONE IS NOT A CALLER'S ZONE. The natural-language body carries no `tz` field, so
    // anything sitting in `plan.tz` here was invented by the rule parser or hallucinated by the
    // model. Treating it as explicit — which is what happened — meant a model could pick the clock
    // an operator's question was answered on, and the response would call that choice "explicit".
    plan.tz = None;
    let (tz, tz_source) = resolve_tz(&st, None, &plan).await?;

    // Re-parse now that the zone is known, so "yesterday" is the SITE's calendar day. The first
    // pass only told us which cameras were named.
    if planner == "rules" {
        let cameras = plan.cameras.clone();
        plan = crate::planner::parse_rules_in(q, &cams, tz);
        plan.cameras = cameras;
    }
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

/// Show how a natural-language question would be interpreted, without executing it.
///
/// A dry run: no rows are read and no data is returned, only the plan and the timezone it would be
/// read in. It is planned, confined and zone-resolved exactly as `/api/v1/search/nl` does, so what
/// it shows is the plan that would actually run.
#[utoipa::path(
    post, path = "/api/v1/search/plan", tag = "search",
    operation_id = "planSearch",
    request_body = NlBody,
    responses(
        (status = 200, description = "The plan, the planner that produced it, and the timezone it would be read in"),
        (status = 400, description = "Empty `query`, or a time-of-day question across cameras in different timezones", body = heldar_kernel::openapi::ErrorBody),
        (status = 403, description = "Missing `events:read`, or a credential scoped to no camera", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn plan_only(
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
    // Resolved and re-parsed exactly as `search_nl` does, for the reason stated below: this route
    // had kept the UTC-only parser, so on a box with a zone configured the dry-run showed a window
    // eight hours away from the one the real route would use — and positively asserted "tz": "UTC".
    // A plan you cannot trust is worse than no dry-run, and one that confidently states the wrong
    // clock is worse still.
    let (mut plan, planner) = match crate::planner::plan_llm(&st.http, &cfg, q, &cams).await {
        Some(p) => (crate::planner::sanitize(p), "llm"),
        None => (
            crate::planner::parse_rules_in(q, &cams, chrono_tz::Tz::UTC),
            "rules",
        ),
    };
    // Confined identically to `search_nl`, so the dry-run shows the plan that WOULD run rather than
    // one the executor will then narrow — a plan you cannot trust is worse than no dry-run.
    plan.cameras = confine_planned_cameras(&principal, &plan.cameras)?;
    plan.tz = None;
    let (tz, tz_source) = resolve_tz(&st, None, &plan).await?;
    if planner == "rules" {
        let cameras = plan.cameras.clone();
        plan = crate::planner::parse_rules_in(q, &cams, tz);
        plan.cameras = cameras;
    }
    plan.tz = Some(tz.to_string());
    Ok(Json(json!({
        "query": body.query,
        "planner": planner,
        "plan": plan,
        "interpretation": {
            "timezone": tz.to_string(),
            "timezone_source": tz_source,
        },
    })))
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
// The rustdoc above is the wrapper's rationale, for maintainers. utoipa would otherwise lift its
// first line into the document as the operation summary, so the caller-facing text is stated here.
#[utoipa::path(
    post, path = "/api/v1/search/semantic", tag = "search",
    operation_id = "searchSemantic",
    summary = "Similarity search over stored crop embeddings",
    description = "Similarity search over the box's stored crop embeddings, from a text description \
or a query image.\n\nResults are similarity-ranked estimates from a learned embedding space, not \
facts — the proof block marks the whole ranking as a fallible inference, so a high score is not a \
match. Send exactly one of `text` or `image_b64`; `image_b64` may carry a `data:` prefix, is capped \
at 10,000,000 characters, and must decode to a JPEG, PNG, WebP, GIF or BMP. `zone` pins its owning \
camera implicitly, and a zone this credential does not hold is reported identically to one that \
does not exist, so the zone id space cannot be enumerated. An empty `cameras` means every camera, \
except for a camera-scoped credential, where it is confined to that credential's own cameras.",
    request_body = crate::semantic::SemanticBody,
    responses(
        (status = 200, description = "Ranked hits, the model that embedded them, and a proof block"),
        (status = 400, description = "Neither or both of `text`/`image_b64`, an undecodable or unsupported image, an invalid or inverted `from`/`to`, or a `zone` whose camera is not in `cameras`", body = heldar_kernel::openapi::ErrorBody),
        (status = 403, description = "Missing `events:read`; a camera requested that this credential does not hold; or, for a camera-scoped credential, a `zone` it does not hold — reported identically to a zone that does not exist, so the zone id space cannot be enumerated", body = heldar_kernel::openapi::ErrorBody),
        (status = 404, description = "No such zone. Reachable only for an unscoped credential: for a camera-scoped one a missing zone is the 403 above", body = heldar_kernel::openapi::ErrorBody),
        (status = 413, description = "Request body over 12 MB. Rejected before deserialization, so this response is not the standard error envelope"),
        (status = 503, description = "No embedding worker answered in time — the query was never embedded", body = heldar_kernel::openapi::ErrorBody),
    ),
)]
pub async fn search_semantic_scoped(
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

/// `resolve_tz` had NO tests, and all three of the route defects an independent review found lived
/// inside it: the natural-language route never consulting the site, the cross-zone refusal being
/// bypassable by an empty camera list, and a camera with no site silently inheriting a sibling's
/// zone. A guard with no tests is a guard nobody has checked.
#[cfg(test)]
mod tz_resolution_tests {
    use super::*;

    async fn state_with(sites: &[(&str, Option<&str>)], cams: &[(&str, Option<&str>)]) -> AppState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        heldar_kernel::db::run_migrations(&pool).await.unwrap();
        let now = chrono::Utc::now();
        for (id, tz) in sites {
            sqlx::query("INSERT INTO sites (id, name, timezone, created_at) VALUES (?,?,?,?)")
                .bind(id)
                .bind(id)
                .bind(tz)
                .bind(now)
                .execute(&pool)
                .await
                .unwrap();
        }
        for (id, site) in cams {
            sqlx::query(
                "INSERT INTO cameras (id, site_id, name, created_at, updated_at) VALUES (?,?,?,?,?)",
            )
            .bind(id)
            .bind(site)
            .bind(id)
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        }
        let cfg = Arc::new(heldar_kernel::config::Config::from_env());
        AppState {
            recorder: heldar_kernel::services::recorder::RecorderManager::new(
                pool.clone(),
                cfg.clone(),
            ),
            sampler: heldar_kernel::services::sampler::SamplerManager::new(
                pool.clone(),
                cfg.clone(),
            ),
            live: heldar_kernel::services::live_publisher::LivePublisherManager::new(
                pool.clone(),
                cfg.clone(),
                heldar_kernel::reqwest::Client::new(),
            ),
            mirror: None,
            consumers: Arc::new(Vec::new()),
            modules: Arc::new(Vec::new()),
            catalog: Arc::new(heldar_kernel::services::registry::CatalogService::new(&cfg)),
            http: heldar_kernel::reqwest::Client::new(),
            media_jobs: heldar_kernel::services::media_jobs::MediaJobGovernor::new(2),
            started_at: chrono::Utc::now(),
            pool,
            cfg,
        }
    }

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn hour_plan(cameras: &[&str]) -> QueryPlan {
        QueryPlan {
            hour_min: Some(18),
            cameras: v(cameras),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_single_sites_zone_wins_and_names_itself() {
        let st = state_with(
            &[("s1", Some("Asia/Kuala_Lumpur"))],
            &[("cam_a", Some("s1"))],
        )
        .await;
        let (tz, src) = resolve_tz(&st, None, &hour_plan(&["cam_a"])).await.unwrap();
        assert_eq!(tz, chrono_tz::Tz::Asia__Kuala_Lumpur);
        assert_eq!(src, "site");
    }

    #[tokio::test]
    async fn an_unconfigured_box_still_answers_in_utc() {
        let st = state_with(&[], &[("cam_a", None)]).await;
        let (tz, src) = resolve_tz(&st, None, &hour_plan(&["cam_a"])).await.unwrap();
        assert_eq!(
            tz,
            chrono_tz::Tz::UTC,
            "the historical behaviour must not move"
        );
        assert_eq!(src, "utc_fallback");
    }

    #[tokio::test]
    async fn a_callers_zone_wins_and_a_bad_one_is_refused() {
        let st = state_with(
            &[("s1", Some("Asia/Kuala_Lumpur"))],
            &[("cam_a", Some("s1"))],
        )
        .await;
        let (tz, src) = resolve_tz(&st, Some("America/New_York"), &hour_plan(&["cam_a"]))
            .await
            .unwrap();
        assert_eq!(tz, chrono_tz::Tz::America__New_York);
        assert_eq!(src, "explicit");
        assert!(resolve_tz(&st, Some("Asia/KL"), &hour_plan(&["cam_a"]))
            .await
            .is_err());
    }

    /// The refusal was bypassable by asking for EVERYTHING: an empty camera list means the whole
    /// fleet, a strict superset of the list that gets refused, and it contributed no zones at all.
    #[tokio::test]
    async fn an_empty_camera_list_cannot_bypass_the_cross_zone_refusal() {
        let st = state_with(
            &[
                ("s_kl", Some("Asia/Kuala_Lumpur")),
                ("s_ny", Some("America/New_York")),
            ],
            &[("cam_kl", Some("s_kl")), ("cam_ny", Some("s_ny"))],
        )
        .await;
        assert!(
            resolve_tz(&st, None, &hour_plan(&["cam_kl", "cam_ny"]))
                .await
                .is_err(),
            "naming both cameras must be refused"
        );
        assert!(
            resolve_tz(&st, None, &hour_plan(&[])).await.is_err(),
            "and so must asking for the whole fleet, which INCLUDES both — otherwise the refusal \
             is bypassed by requesting strictly more"
        );
    }

    /// The refusal must not fire on a query whose answer does not depend on the clock. A
    /// camera-scoped credential whose cameras span two sites could otherwise never run ANY
    /// structured search, including a plain plate lookup with an absolute window, and had no way to
    /// satisfy the guard because its complaint was unrelated to what it asked.
    #[tokio::test]
    async fn a_query_with_no_time_of_day_term_is_not_refused_across_zones() {
        let st = state_with(
            &[
                ("s_kl", Some("Asia/Kuala_Lumpur")),
                ("s_ny", Some("America/New_York")),
            ],
            &[("cam_kl", Some("s_kl")), ("cam_ny", Some("s_ny"))],
        )
        .await;
        let plate_lookup = QueryPlan {
            from: Some("2026-06-01T00:00:00Z".into()),
            to: Some("2026-06-02T00:00:00Z".into()),
            plate: Some("ABC123".into()),
            cameras: v(&["cam_kl", "cam_ny"]),
            ..Default::default()
        };
        let (_, src) = resolve_tz(&st, None, &plate_lookup)
            .await
            .expect("an absolute-window plate lookup means the same instants in every zone");
        assert_eq!(src, "not_time_of_day");
    }

    /// A camera with no site has no zone configured; its historical behaviour is UTC. Reading it in
    /// a sibling's zone is a silent shift, and the guard was blind to it because a camera
    /// contributing no zone contributed nothing to the comparison.
    #[tokio::test]
    async fn a_camera_with_no_site_does_not_inherit_a_siblings_zone() {
        let st = state_with(
            &[("s_kl", Some("Asia/Kuala_Lumpur"))],
            &[("cam_kl", Some("s_kl")), ("cam_orphan", None)],
        )
        .await;
        assert!(
            resolve_tz(&st, None, &hour_plan(&["cam_kl", "cam_orphan"]))
                .await
                .is_err(),
            "the orphan falls back to UTC and KL is +08:00 — that is a real disagreement and must \
             be refused, not resolved to whichever camera was listed first"
        );
        // Alone, it is simply UTC.
        let (tz, _) = resolve_tz(&st, None, &hour_plan(&["cam_orphan"]))
            .await
            .unwrap();
        assert_eq!(tz, chrono_tz::Tz::UTC);
    }

    /// ...and the mirror image: a box default is what a site-less camera falls back to, so it must
    /// not manufacture a disagreement with a site that names the same zone.
    #[tokio::test]
    async fn the_box_default_does_not_manufacture_a_disagreement() {
        let st = state_with(
            &[("s_kl", Some("Asia/Kuala_Lumpur"))],
            &[("cam_kl", Some("s_kl")), ("cam_orphan", None)],
        )
        .await;
        heldar_kernel::services::settings::set_str(
            &st.pool,
            heldar_kernel::services::tz::DEFAULT_TIMEZONE,
            "Asia/Kuala_Lumpur",
        )
        .await
        .unwrap();
        let (tz, _) = resolve_tz(&st, None, &hour_plan(&["cam_kl", "cam_orphan"]))
            .await
            .expect("both cameras effectively read Asia/Kuala_Lumpur — there is no disagreement");
        assert_eq!(tz, chrono_tz::Tz::Asia__Kuala_Lumpur);
    }
}
