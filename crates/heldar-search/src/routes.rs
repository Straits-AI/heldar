//! Semantic-search HTTP surface: structured search, natural-language search (plan → execute → proof),
//! and a plan dry-run. Reads need can_view; every search is logged (search_log) and identity-bearing
//! queries are audited (kernel audit_log). Answers are the executed query's rows — never model output.

use std::sync::Arc;

use axum::extract::{Extension, State};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use heldar_kernel::auth::{self, Principal};
use heldar_kernel::error::AppResult;
use heldar_kernel::state::AppState;

use crate::config::SearchConfig;
use crate::query::{self, QueryPlan};

pub fn router(cfg: Arc<SearchConfig>) -> Router<AppState> {
    Router::new()
        .route("/api/v1/search/events", post(search_events))
        .route("/api/v1/search/nl", post(search_nl))
        .route("/api/v1/search/plan", post(plan_only))
        .route("/api/v1/modules/search/ui/index.js", get(serve_ui))
        .layer(Extension(cfg))
}

/// The built search module UI bundle, embedded at compile time (regenerate with `make module-bundles`
/// after editing `apps/web/src/modules/search`). It imports React + the shell SDK (`@heldar/shell`) as
/// bare specifiers the dashboard's import map resolves — so this crate ships only the module's own code.
const SEARCH_UI_BUNDLE: &str = include_str!("../ui/search.js");

/// Serve the runtime-loaded search module UI (the dashboard imports it via `ModuleHost`). Any
/// authenticated viewer may load it — it is inert frontend code; the data it fetches is separately
/// gated by the kernel's RBAC.
async fn serve_ui(principal: Principal) -> AppResult<axum::response::Response> {
    principal.require(principal.can_view(), "load the search module UI")?;
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

async fn cameras(pool: &sqlx::SqlitePool) -> Vec<(String, String)> {
    sqlx::query_as("SELECT id, name FROM cameras")
        .fetch_all(pool)
        .await
        .unwrap_or_default()
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

async fn log_search(
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

async fn search_events(
    State(st): State<AppState>,
    principal: Principal,
    Extension(cfg): Extension<Arc<SearchConfig>>,
    Json(plan): Json<QueryPlan>,
) -> AppResult<Json<Value>> {
    principal.require(principal.can_view(), "search events")?;
    // Sanitize the caller-supplied plan (clamp out-of-range hours, etc.) before executing — the same
    // guard applied to LLM-produced plans — so a hand-crafted QueryPlan can't smuggle invalid filters.
    let plan = crate::planner::sanitize(plan);
    let outcome = query::execute(&st.pool, &plan, cfg.max_results).await?;
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
    Ok(Json(response(None, "structured", &plan, outcome)))
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
    principal.require(principal.can_view(), "natural-language search")?;
    let q = body.query.trim();
    if q.is_empty() {
        return Err(heldar_kernel::error::AppError::BadRequest(
            "`query` is required".into(),
        ));
    }
    let cams = cameras(&st.pool).await;
    // LLM planner if configured, else the transparent rule parser. The LLM only PLANS.
    let (plan, planner) = match crate::planner::plan_llm(&st.http, &cfg, q, &cams).await {
        Some(p) => (crate::planner::sanitize(p), "llm"),
        None => (crate::planner::parse_rules(q, &cams), "rules"),
    };
    let outcome = query::execute(&st.pool, &plan, cfg.max_results).await?;
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
    Ok(Json(response(Some(q), planner, &plan, outcome)))
}

async fn plan_only(
    State(st): State<AppState>,
    principal: Principal,
    Extension(cfg): Extension<Arc<SearchConfig>>,
    Json(body): Json<NlBody>,
) -> AppResult<Json<Value>> {
    // Dry-run: show how a question is interpreted (no execution, no data) — useful for trust/debug.
    principal.require(principal.can_view(), "plan a search")?;
    let q = body.query.trim();
    if q.is_empty() {
        return Err(heldar_kernel::error::AppError::BadRequest(
            "`query` is required".into(),
        ));
    }
    let cams = cameras(&st.pool).await;
    let (plan, planner) = match crate::planner::plan_llm(&st.http, &cfg, q, &cams).await {
        Some(p) => (crate::planner::sanitize(p), "llm"),
        None => (
            crate::planner::parse_rules(body.query.trim(), &cams),
            "rules",
        ),
    };
    Ok(Json(
        json!({ "query": body.query, "planner": planner, "plan": plan }),
    ))
}

fn response(
    query: Option<&str>,
    planner: &str,
    plan: &QueryPlan,
    outcome: query::ExecOutcome,
) -> Value {
    let proof = crate::proof::build(query, planner, plan, &outcome.hits, outcome.truncated);
    json!({
        "query": query,
        "planner": planner,
        "plan": plan,
        "count": outcome.hits.len(),
        // Honest signal: the field-filtered result set may omit older in-window matches because a
        // source hit its fetch cap. Clients (and the proof layer) must not treat the count as complete.
        "truncated": outcome.truncated,
        "hits": outcome.hits,
        "proof": proof,
    })
}
