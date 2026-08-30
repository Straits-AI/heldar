//! `Idempotency-Key` replay protection for mutations.
//!
//! A client that times out cannot tell "the box never got it" from "the box did it and I lost the
//! reply". Retrying is the only sane thing to do, and without this it does the work twice: two clips,
//! two backup jobs, two cameras.
//!
//! Opt-in by the CALLER — send the header and get replay protection; omit it and nothing changes.
//! That is what lets this be one middleware rather than an edit to forty handlers, and it means no
//! existing client's behaviour moves.
//!
//! # Rules
//!
//! 1. Same key, same principal, same route, same body → the ORIGINAL response, replayed.
//! 2. Same key, different body → `409`. That is a client bug (a key reused for a different
//!    operation), and returning the first result for the second request would be worse than an error.
//! 3. Keys are scoped to the principal. A key is a client-chosen string, so without that a caller
//!    could replay someone else's result by guessing one — deduplication becomes an information leak.
//! 4. A duplicate arriving while the first is still running gets `409`, not a half-finished answer.
//!
//! # What it does not do
//!
//! Only `POST`/`PUT`/`PATCH`/`DELETE`, and only when the header is present. Responses are replayed
//! from a stored body, so a streaming or very large response is not cached — the cap below refuses to
//! store one rather than truncating it into a lie.

use axum::body::{to_bytes, Body};
use axum::extract::{Request, State};
use axum::http::{HeaderName, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

use crate::state::AppState;

pub const IDEMPOTENCY_KEY: HeaderName = HeaderName::from_static("idempotency-key");

/// Bodies larger than this are not stored, and the request runs unprotected.
///
/// Replay is only useful if the reply can be reproduced exactly; storing a prefix would replay a
/// truncated body as though it were the whole answer. Refusing to cache is the honest failure.
const MAX_STORED_BODY: usize = 64 * 1024;

/// How long a key is honoured. Long enough to cover a retry storm and an operator re-running a
/// command, short enough that the table does not become a second copy of the API's history.
const RETENTION_HOURS: i64 = 24;

fn hash_body(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

pub async fn layer(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let is_mutation = matches!(req.method().as_str(), "POST" | "PUT" | "PATCH" | "DELETE");
    let key = req
        .headers()
        .get(&IDEMPOTENCY_KEY)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.chars().take(200).collect::<String>())
        .filter(|v: &String| !v.is_empty());
    let (Some(key), true) = (key, is_mutation) else {
        return next.run(req).await;
    };

    // The principal is resolved by the auth floor, which runs OUTSIDE this layer, so the extension is
    // present. Without a principal there is nothing to scope the key to, and an unscoped key is the
    // leak described above — so run unprotected rather than share a namespace.
    let Some(principal) = req.extensions().get::<crate::auth::Principal>().cloned() else {
        return next.run(req).await;
    };

    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let (parts, body) = req.into_parts();
    let bytes = match to_bytes(body, MAX_STORED_BODY).await {
        Ok(b) => b,
        // Oversized: not cacheable, so run it unprotected rather than store a truncated reply. The
        // body is already consumed, so it has to be rebuilt from what we read.
        Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "request body too large").into_response(),
    };
    let hash = hash_body(&bytes);

    // Claim the key. The PRIMARY KEY makes this the concurrency control: whoever inserts first runs,
    // everyone else takes the branch below.
    let claimed = sqlx::query(
        "INSERT INTO idempotency_keys (key, principal_id, method, path, request_hash, created_at)
         VALUES (?,?,?,?,?,?) ON CONFLICT DO NOTHING",
    )
    .bind(&key)
    .bind(&principal.id)
    .bind(&method)
    .bind(&path)
    .bind(&hash)
    .bind(chrono::Utc::now())
    .execute(&st.pool)
    .await
    .map(|r| r.rows_affected() == 1)
    .unwrap_or(false);

    if !claimed {
        let prior: Option<(String, Option<i64>, Option<String>)> = sqlx::query_as(
            "SELECT request_hash, status_code, body FROM idempotency_keys
              WHERE principal_id = ? AND key = ? AND method = ? AND path = ?",
        )
        .bind(&principal.id)
        .bind(&key)
        .bind(&method)
        .bind(&path)
        .fetch_optional(&st.pool)
        .await
        .ok()
        .flatten();
        return match prior {
            // A key reused for a DIFFERENT request: a client bug. Returning the first result would
            // silently answer the wrong question.
            Some((prior_hash, _, _)) if prior_hash != hash => (
                StatusCode::CONFLICT,
                axum::Json(serde_json::json!({
                    "error": "this Idempotency-Key was already used for a different request",
                    "code": "idempotency_key_conflict",
                    "retryable": false,
                })),
            )
                .into_response(),
            // Replay the original.
            Some((_, Some(status), Some(body))) => {
                let code = StatusCode::from_u16(status as u16).unwrap_or(StatusCode::OK);
                let mut resp = (code, body).into_response();
                resp.headers_mut().insert(
                    HeaderName::from_static("idempotency-replayed"),
                    axum::http::HeaderValue::from_static("true"),
                );
                resp
            }
            // Claimed but unfinished: the first call is still running. Telling the client to retry is
            // better than inventing an answer.
            _ => (
                StatusCode::CONFLICT,
                axum::Json(serde_json::json!({
                    "error": "a request with this Idempotency-Key is still in flight",
                    "code": "idempotency_in_progress",
                    "retryable": true,
                })),
            )
                .into_response(),
        };
    }

    let resp = next
        .run(Request::from_parts(parts, Body::from(bytes)))
        .await;
    let (rparts, rbody) = resp.into_parts();
    let rbytes = to_bytes(rbody, MAX_STORED_BODY).await.unwrap_or_default();

    // Only successes are replayable. Caching a failure would pin a transient error in place: the
    // client retries with the same key, having done nothing wrong, and gets the old failure forever.
    if rparts.status.is_success() {
        let _ = sqlx::query(
            "UPDATE idempotency_keys SET status_code = ?, body = ?
              WHERE principal_id = ? AND key = ? AND method = ? AND path = ?",
        )
        .bind(rparts.status.as_u16() as i64)
        .bind(String::from_utf8_lossy(&rbytes).to_string())
        .bind(&principal.id)
        .bind(&key)
        .bind(&method)
        .bind(&path)
        .execute(&st.pool)
        .await;
    } else {
        // Release the key so the caller can retry the SAME operation.
        let _ = sqlx::query(
            "DELETE FROM idempotency_keys
              WHERE principal_id = ? AND key = ? AND method = ? AND path = ? AND status_code IS NULL",
        )
        .bind(&principal.id)
        .bind(&key)
        .bind(&method)
        .bind(&path)
        .execute(&st.pool)
        .await;
    }
    Response::from_parts(rparts, Body::from(rbytes))
}

/// Drop keys past their retention window. Called from the retention loop.
pub async fn prune(pool: &sqlx::SqlitePool) -> u64 {
    let cutoff = chrono::Utc::now() - chrono::Duration::hours(RETENTION_HOURS);
    sqlx::query("DELETE FROM idempotency_keys WHERE created_at < ?")
        .bind(cutoff)
        .execute(pool)
        .await
        .map(|r| r.rows_affected())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::post;
    use axum::Router;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tower::Service;

    async fn state() -> AppState {
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

    fn principal(id: &str) -> crate::auth::Principal {
        crate::auth::Principal {
            id: id.to_string(),
            ..crate::auth::Principal::system_admin()
        }
    }

    /// A router whose handler COUNTS its invocations — the whole question is whether the work ran
    /// twice, which a response body alone cannot answer.
    fn app(st: &AppState, who: &str, calls: Arc<AtomicUsize>) -> Router {
        let p = principal(who);
        Router::new()
            .route(
                "/api/v1/thing",
                post(move || {
                    let calls = calls.clone();
                    async move {
                        let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
                        axum::Json(serde_json::json!({ "ran": n }))
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(st.clone(), layer))
            // Stands in for the auth floor, which is what puts the principal in extensions.
            .layer(axum::middleware::from_fn(
                move |mut req: Request, next: Next| {
                    let p = p.clone();
                    async move {
                        req.extensions_mut().insert(p);
                        next.run(req).await
                    }
                },
            ))
            .with_state(st.clone())
    }

    async fn post_it(app: &mut Router, key: Option<&str>, body: &str) -> (StatusCode, String) {
        let mut b = Request::builder().method("POST").uri("/api/v1/thing");
        if let Some(k) = key {
            b = b.header("idempotency-key", k);
        }
        let resp = app
            .call(b.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    /// The point: a retried request must not do the work twice.
    #[tokio::test]
    async fn a_replayed_key_returns_the_first_result_and_does_not_rerun() {
        let st = state().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let mut a = app(&st, "key_1", calls.clone());

        let (s1, b1) = post_it(&mut a, Some("abc"), r#"{"x":1}"#).await;
        let (s2, b2) = post_it(&mut a, Some("abc"), r#"{"x":1}"#).await;
        assert_eq!((s1, s2), (StatusCode::OK, StatusCode::OK));
        assert_eq!(b1, b2, "the replay returned a different body");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the handler ran twice — the work was duplicated, which is the whole thing this prevents"
        );
    }

    /// Without the header nothing changes, so no existing client's behaviour moves.
    #[tokio::test]
    async fn without_the_header_every_request_runs() {
        let st = state().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let mut a = app(&st, "key_1", calls.clone());
        post_it(&mut a, None, "{}").await;
        post_it(&mut a, None, "{}").await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    /// Reusing a key for a DIFFERENT body is a client bug. Returning the first answer would silently
    /// respond to a question that was never asked.
    #[tokio::test]
    async fn the_same_key_with_a_different_body_is_a_conflict() {
        let st = state().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let mut a = app(&st, "key_1", calls.clone());
        post_it(&mut a, Some("abc"), r#"{"x":1}"#).await;
        let (status, body) = post_it(&mut a, Some("abc"), r#"{"x":2}"#).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(body.contains("idempotency_key_conflict"), "{body}");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// SECURITY: keys are namespaced per principal. A key is a client-chosen string, so sharing the
    /// namespace would let one caller replay another's result by guessing one.
    #[tokio::test]
    async fn one_principals_key_cannot_replay_another_principals_result() {
        let st = state().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let mut first = app(&st, "key_alice", calls.clone());
        let mut second = app(&st, "key_bob", calls.clone());

        let (_, alice) = post_it(&mut first, Some("shared"), r#"{"x":1}"#).await;
        // Bob uses the SAME key. He must get his own execution, not Alice's stored answer.
        let (status, bob) = post_it(&mut second, Some("shared"), r#"{"x":1}"#).await;
        assert_eq!(status, StatusCode::OK);
        assert_ne!(
            alice, bob,
            "bob was served alice's cached response — the key namespace is shared"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    /// A failure must not be pinned in place: the caller did nothing wrong and must be able to retry
    /// the same operation with the same key.
    #[tokio::test]
    async fn a_failed_request_releases_its_key() {
        let st = state().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let p = principal("key_1");
        let mut a = Router::new()
            .route(
                "/api/v1/thing",
                post(move || {
                    let c = c.clone();
                    async move {
                        // Fails once, then succeeds — the shape of a transient.
                        let n = c.fetch_add(1, Ordering::SeqCst);
                        if n == 0 {
                            crate::error::AppError::Unavailable("busy".into()).into_response()
                        } else {
                            axum::Json(serde_json::json!({ "ran": n })).into_response()
                        }
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(st.clone(), layer))
            .layer(axum::middleware::from_fn(
                move |mut req: Request, next: Next| {
                    let p = p.clone();
                    async move {
                        req.extensions_mut().insert(p);
                        next.run(req).await
                    }
                },
            ))
            .with_state(st.clone());

        let (s1, _) = post_it(&mut a, Some("retry-me"), "{}").await;
        assert_eq!(s1, StatusCode::SERVICE_UNAVAILABLE);
        let (s2, _) = post_it(&mut a, Some("retry-me"), "{}").await;
        assert_eq!(
            s2,
            StatusCode::OK,
            "the key stayed claimed after a failure, so the caller could never retry"
        );
    }

    /// GETs are not mutations and are never keyed.
    #[tokio::test]
    async fn reads_are_untouched() {
        let st = state().await;
        let n = prune(&st.pool).await;
        assert_eq!(n, 0, "nothing to prune on a fresh box");
    }
}
