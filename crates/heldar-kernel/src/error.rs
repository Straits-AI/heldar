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
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type AppResult<T> = Result<T, AppError>;

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            AppError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            AppError::Conflict(m) => (StatusCode::CONFLICT, m),
            AppError::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m),
            AppError::Forbidden(m) => (StatusCode::FORBIDDEN, m),
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
        let mut resp = (status, Json(json!({ "error": msg }))).into_response();
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
