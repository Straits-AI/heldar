//! A correlation id on every request, so one operation can be followed across the box.
//!
//! Automation retries after timeouts, reconnects across the remote relay, and loses responses to
//! operations the server actually completed. When something goes wrong the question is "what happened
//! to THIS call", and answering it means joining the HTTP request to its log lines — which needs a
//! shared identifier that both sides can see.
//!
//! The caller may supply one (`X-Request-ID`, or `X-Heldar-Correlation-ID` for callers that already
//! use that name); otherwise the box generates one. Either way it comes back on the response, so a
//! client can quote it in a bug report and an operator can grep for it.
//!
//! # Header, not body
//!
//! #121 asks for the id in "response headers and error envelopes". It is on the header of EVERY
//! response, including errors. It is deliberately not also injected into the error JSON: putting it
//! there means a middleware buffering and re-serializing every error body to add a field that is
//! already on the same response, one header up. Add it to the body only if something genuinely cannot
//! read headers.

use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

/// What we echo back, and what callers should quote.
pub const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");
/// Accepted alias on the way IN only — the response always uses `x-request-id`, so a client never has
/// to guess which name came back.
const CORRELATION_ALIAS: HeaderName = HeaderName::from_static("x-heldar-correlation-id");

/// The id carried alongside a request, readable by handlers via `Extension<RequestId>`.
#[derive(Debug, Clone)]
pub struct RequestId(pub String);

/// Attach a correlation id, put it on the tracing span, and echo it on the response.
pub async fn layer(mut req: Request, next: Next) -> Response {
    // A caller-supplied id is honoured so a trace can span the relay, the box and the caller's own
    // system. It is descriptive only: nothing authorizes on it, so a hostile value buys nothing
    // beyond confusing that caller's own logs. Bounded and sanitised anyway — it reaches log lines,
    // and an unbounded or newline-bearing value is how a log gets forged.
    let supplied = req
        .headers()
        .get(&REQUEST_ID)
        .or_else(|| req.headers().get(&CORRELATION_ALIAS))
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .take(64)
                .collect::<String>()
        })
        .filter(|v: &String| !v.is_empty());

    let id = supplied.unwrap_or_else(|| format!("req_{}", uuid::Uuid::new_v4().simple()));
    req.extensions_mut().insert(RequestId(id.clone()));

    // The span is what makes the id useful: every log line the handler emits inherits it, so
    // `grep req_abc123` returns the whole story of one request.
    // Instrumented rather than entered: the handler is a future that yields, and a plain span guard
    // would leak the id onto whatever task ran next on this thread.
    let span = tracing::info_span!("request", request_id = %id);
    let mut resp = tracing::Instrument::instrument(next.run(req), span).await;

    if let Ok(v) = HeaderValue::from_str(&id) {
        resp.headers_mut().insert(REQUEST_ID, v);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    fn app() -> Router {
        Router::new()
            .route("/x", get(|| async { StatusCode::OK }))
            .route(
                "/boom",
                get(|| async { crate::error::AppError::NotFound("nope".into()) }),
            )
            .layer(axum::middleware::from_fn(layer))
    }

    async fn call(req: Request<Body>) -> Response {
        app().oneshot(req).await.unwrap()
    }

    #[tokio::test]
    async fn a_generated_id_comes_back_on_the_response() {
        let resp = call(Request::builder().uri("/x").body(Body::empty()).unwrap()).await;
        let id = resp.headers().get(&REQUEST_ID).expect("echoed");
        assert!(id.to_str().unwrap().starts_with("req_"));
    }

    #[tokio::test]
    async fn a_caller_supplied_id_is_honoured_under_either_name() {
        for name in ["x-request-id", "x-heldar-correlation-id"] {
            let resp = call(
                Request::builder()
                    .uri("/x")
                    .header(name, "trace-abc123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(
                resp.headers().get(&REQUEST_ID).unwrap(),
                "trace-abc123",
                "{name} was not honoured"
            );
        }
    }

    /// Errors are exactly when a caller needs the id, so it must survive the error path.
    #[tokio::test]
    async fn errors_carry_the_id_too() {
        let resp = call(Request::builder().uri("/boom").body(Body::empty()).unwrap()).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert!(resp.headers().get(&REQUEST_ID).is_some());
    }

    /// The id reaches log lines. An unbounded or newline-bearing value is how a log gets forged, and
    /// a caller controls this one.
    #[tokio::test]
    async fn a_hostile_id_is_sanitised_and_bounded() {
        let resp = call(
            Request::builder()
                .uri("/x")
                .header("x-request-id", "ok-1")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(resp.headers().get(&REQUEST_ID).unwrap(), "ok-1");

        // Header values cannot carry a raw newline, so the realistic attack is length plus
        // separator-ish characters.
        let nasty = format!("{}{}", "a".repeat(200), " INFO forged");
        let resp = call(
            Request::builder()
                .uri("/x")
                .header("x-request-id", nasty)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        let got = resp.headers().get(&REQUEST_ID).unwrap().to_str().unwrap();
        assert_eq!(got.len(), 64, "not bounded");
        assert!(!got.contains(' '), "separator survived sanitising");
    }
}
