//! Semantic-search HTTP surface: structured search, natural-language search (plan → execute → proof),
//! and a plan dry-run. Reads need can_view; every search is logged (search_log) and identity-bearing
//! queries are audited (kernel audit_log). Answers are the executed query's rows — never model output.

use std::sync::Arc;

use axum::extract::{Extension, State};
use axum::routing::post;
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use heldar_kernel::auth::{self, Principal};
use heldar_kernel::error::AppResult;
use heldar_kernel::state::AppState;

use crate::config::SearchConfig;
use crate::query::{self, QueryPlan, SearchHit};

pub fn router(cfg: Arc<SearchConfig>) -> Router<AppState> {
    Router::new()
        .route("/api/v1/search/events", post(search_events))
        .route("/api/v1/search/nl", post(search_nl))
        .route("/api/v1/search/plan", post(plan_only))
        .layer(Extension(cfg))
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
    let hits = query::execute(&st.pool, &plan, cfg.max_results).await?;
    log_search(
        &st,
        &principal,
        "structured",
        None,
        &plan,
        "structured",
        hits.len(),
    )
    .await;
    Ok(Json(response(None, "structured", &plan, hits)))
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
    let hits = query::execute(&st.pool, &plan, cfg.max_results).await?;
    log_search(&st, &principal, "nl", Some(q), &plan, planner, hits.len()).await;
    Ok(Json(response(Some(q), planner, &plan, hits)))
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

fn response(query: Option<&str>, planner: &str, plan: &QueryPlan, hits: Vec<SearchHit>) -> Value {
    let proof = crate::proof::build(query, planner, plan, &hits);
    json!({
        "query": query,
        "planner": planner,
        "plan": plan,
        "count": hits.len(),
        "hits": hits,
        "proof": proof,
    })
}
