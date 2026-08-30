use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Application error type, convertible into an HTTP response.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Forbidden(String),
    /// A required backend is temporarily missing (e.g. no embedding worker answering) — 503 with
    /// a Retry-After hint, like the DB-busy mapping below.
    #[error("{0}")]
    Unavailable(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    /// A stable, machine-readable identifier for this failure.
    ///
    /// Separate from the message on purpose: messages get reworded, and an integration that matched
    /// on prose would break silently when someone improved a sentence. These strings are API.
    pub fn code(&self) -> &'static str {
        match self {
            AppError::NotFound(_) => "not_found",
            AppError::BadRequest(_) => "bad_request",
            AppError::Conflict(_) => "conflict",
            AppError::Unauthorized(_) => "unauthorized",
            AppError::Forbidden(_) => "forbidden",
            AppError::Unavailable(_) => "unavailable",
            // The DB variant splits by cause: a busy pool is worth retrying, a constraint violation
            // never is, and a client cannot tell them apart from a bare 500.
            AppError::Db(sqlx::Error::RowNotFound) => "not_found",
            AppError::Db(sqlx::Error::PoolTimedOut) => "busy",
            AppError::Db(sqlx::Error::Database(_)) => "conflict",
            AppError::Db(_) => "internal",
            AppError::Other(_) => "internal",
        }
    }

    /// Whether retrying the SAME request could plausibly succeed.
    ///
    /// Only transient saturation qualifies. A 404 or a validation failure will fail identically
    /// forever, and a client that retries them just adds load to a box already answering correctly.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            AppError::Unavailable(_) | AppError::Db(sqlx::Error::PoolTimedOut)
        )
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let code = self.code();
        let retryable = self.is_retryable();
        let (status, msg) = match self {
            AppError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            AppError::Conflict(m) => (StatusCode::CONFLICT, m),
            AppError::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m),
            AppError::Forbidden(m) => (StatusCode::FORBIDDEN, m),
            AppError::Unavailable(m) => (StatusCode::SERVICE_UNAVAILABLE, m),
            AppError::Db(sqlx::Error::RowNotFound) => {
                (StatusCode::NOT_FOUND, "resource not found".to_string())
            }
            // Pool exhausted: all connections were busy past the acquire timeout. Transient
            // saturation, not a server fault — ask the client to retry.
            AppError::Db(sqlx::Error::PoolTimedOut) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "database busy; retry shortly".to_string(),
            ),
            // Map common constraint violations to 4xx instead of 500 (e.g. duplicate id,
            // or a site_id/foreign key that does not exist).
            AppError::Db(sqlx::Error::Database(ref dbe)) => {
                use sqlx::error::ErrorKind;
                // SQLite busy/locked under write contention is transient: the pool's busy_timeout
                // waits, but if it is ever exceeded the correct answer is 503 + Retry-After, not a
                // 500. (SQLITE_BUSY=5 and its extended codes 261/517/773.)
                let busy = matches!(
                    dbe.code().as_deref(),
                    Some("5") | Some("261") | Some("517") | Some("773")
                ) || {
                    let m = dbe.message().to_ascii_lowercase();
                    m.contains("database is locked") || m.contains("database is busy")
                };
                if busy {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "database busy; retry shortly".to_string(),
                    )
                } else {
                    match dbe.kind() {
                        ErrorKind::UniqueViolation => {
                            (StatusCode::CONFLICT, "resource already exists".to_string())
                        }
                        ErrorKind::ForeignKeyViolation => (
                            StatusCode::BAD_REQUEST,
                            "referenced resource does not exist (check site_id)".to_string(),
                        ),
                        _ => {
                            tracing::error!(error = %dbe, "database error");
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                "database error".to_string(),
                            )
                        }
                    }
                }
            }
            AppError::Db(e) => {
                tracing::error!(error = %e, "database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "database error".to_string(),
                )
            }
            AppError::Other(e) => {
                tracing::error!(error = ?e, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                )
            }
        };
        // `error` stays a STRING. #120 proposed nesting it as an object, and that would break every
        // existing client in one release: the dashboard does `data.error ?? data.message`, so an
        // object renders as "[object Object]". The machine-readable half is added ALONGSIDE instead —
        // same information, no migration, no version bump. Nest it later only if something actually
        // needs the nesting.
        let mut resp = (
            status,
            Json(json!({
                "error": msg,
                // A stable identifier to branch on. The message is for humans and may be reworded;
                // this may not.
                "code": code,
                // Explicit rather than "infer it from the status", which every client would
                // otherwise hardcode slightly differently.
                "retryable": retryable,
            })),
        )
            .into_response();
        // A retryable transient (busy/saturated) gets a Retry-After hint.
        if status == StatusCode::SERVICE_UNAVAILABLE {
            resp.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                axum::http::HeaderValue::from_static("1"),
            );
        }
        resp
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn body_of(e: AppError) -> (StatusCode, serde_json::Value) {
        let resp = e.into_response();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    /// COMPATIBILITY: `error` must stay a string. The dashboard does `data.error ?? data.message`,
    /// so nesting it as an object would render "[object Object]" in every error toast.
    #[tokio::test]
    async fn error_stays_a_plain_string_for_existing_clients() {
        let (status, body) = body_of(AppError::NotFound("camera cam_x not found".into())).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            body["error"].as_str(),
            Some("camera cam_x not found"),
            "`error` changed shape; every existing client reads it as a string"
        );
    }

    /// ...and the machine-readable half rides alongside it.
    #[tokio::test]
    async fn the_code_is_stable_and_separate_from_the_message() {
        let (_, body) = body_of(AppError::Forbidden("nope".into())).await;
        assert_eq!(body["code"].as_str(), Some("forbidden"));
        assert_eq!(body["retryable"].as_bool(), Some(false));
    }

    /// Only transient saturation is retryable. A client that retries a 404 adds load to a box that
    /// is answering correctly.
    #[tokio::test]
    async fn only_transient_failures_are_marked_retryable() {
        for (e, want) in [
            (AppError::Unavailable("no worker".into()), true),
            (AppError::Db(sqlx::Error::PoolTimedOut), true),
            (AppError::NotFound("x".into()), false),
            (AppError::BadRequest("x".into()), false),
            (AppError::Forbidden("x".into()), false),
        ] {
            let code = e.code();
            let (_, body) = body_of(e).await;
            assert_eq!(
                body["retryable"].as_bool(),
                Some(want),
                "wrong retryability for code `{code}`"
            );
        }
    }

    /// A 503 keeps its Retry-After: `retryable: true` with no hint tells a client to retry but not
    /// when, and they all pick a different interval.
    #[tokio::test]
    async fn retryable_responses_still_carry_retry_after() {
        let resp = AppError::Unavailable("no embedding worker".into()).into_response();
        assert_eq!(
            resp.headers().get(axum::http::header::RETRY_AFTER),
            Some(&axum::http::HeaderValue::from_static("1"))
        );
    }

    /// The DB variant splits by cause — a busy pool is worth retrying, a constraint violation is not,
    /// and a bare 500 cannot tell a client which it hit.
    #[test]
    fn db_errors_split_by_cause() {
        assert_eq!(AppError::Db(sqlx::Error::PoolTimedOut).code(), "busy");
        assert_eq!(AppError::Db(sqlx::Error::RowNotFound).code(), "not_found");
        assert_eq!(AppError::Other(anyhow::anyhow!("x")).code(), "internal");
    }
}
