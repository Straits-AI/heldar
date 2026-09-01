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
    /// The stable machine-readable code for a response status.
    ///
    /// Derived from the STATUS the classifier already chose, not from the variant. Deriving it from
    /// the variant meant classifying twice, and the two disagreed: `into_response` splits
    /// `Db(Database(_))` four ways — busy is 503, a unique violation 409, a foreign-key violation
    /// 400, anything else 500 — while a second match here returned "conflict" for all four. A busy
    /// SQLite therefore answered `503` with `Retry-After` AND `code: "conflict", retryable: false`,
    /// contradicting itself inside one response.
    ///
    /// One classification, one place. This is the same rule that `CapSet::expanded` and
    /// `subject_still_stands` exist to enforce: a question with two implementations eventually has
    /// two answers.
    pub fn code_for_status(status: StatusCode) -> &'static str {
        match status {
            StatusCode::BAD_REQUEST => "bad_request",
            StatusCode::UNAUTHORIZED => "unauthorized",
            StatusCode::FORBIDDEN => "forbidden",
            StatusCode::NOT_FOUND => "not_found",
            StatusCode::CONFLICT => "conflict",
            StatusCode::PAYLOAD_TOO_LARGE => "payload_too_large",
            StatusCode::TOO_MANY_REQUESTS => "rate_limited",
            StatusCode::SERVICE_UNAVAILABLE => "unavailable",
            _ => "internal",
        }
    }

    /// Every code [`code_for_status`] can return, in the order a reader would want them.
    ///
    /// THE PUBLISHED CONTRACT ENUMERATES THIS LIST, and it must not be a second, hand-typed copy.
    /// It was: the OpenAPI schema's description listed `busy` — a code this function has never
    /// returned — and omitted `payload_too_large` and `rate_limited`, which it returns routinely.
    /// A client branching on `busy` would have waited forever for a code that does not exist, and
    /// one hitting a 429 would have met an identifier the contract never mentioned.
    ///
    /// That is the exact drift the contract module was written to prevent, inside the contract
    /// module. `codes_documented_match_codes_returned` now holds the two together.
    ///
    /// [`code_for_status`]: AppError::code_for_status
    pub const ALL_CODES: &'static [&'static str] = &[
        "bad_request",
        "unauthorized",
        "forbidden",
        "not_found",
        "conflict",
        "payload_too_large",
        "rate_limited",
        "unavailable",
        "internal",
    ];

    /// Whether retrying the SAME request could plausibly succeed.
    ///
    /// Only transient saturation: 503 (a backend missing, or the database busy) and 429. A 404 or a
    /// validation failure fails identically forever, and a client retrying them only adds load to a
    /// box that is answering correctly.
    pub fn retryable_for_status(status: StatusCode) -> bool {
        matches!(
            status,
            StatusCode::SERVICE_UNAVAILABLE | StatusCode::TOO_MANY_REQUESTS
        )
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
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
        // Derived AFTER the classifier, from its own answer.
        let code = AppError::code_for_status(status);
        let retryable = AppError::retryable_for_status(status);
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
            let (status, body) = body_of(e).await;
            assert_eq!(
                body["retryable"].as_bool(),
                Some(want),
                "wrong retryability for status {status}"
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

    /// The bug this replaced: `code`/`retryable` were derived from the VARIANT while the status came
    /// from a four-way split inside `Db(Database(_))`. A busy SQLite answered 503 + Retry-After and
    /// `code: "conflict", retryable: false` in the same response.
    ///
    /// Driven through a REAL sqlx error, because the previous test asserted on `.code()` directly and
    /// never constructed the `Database(_)` variant at all — the one whose name promised to cover the
    /// split was the one that skipped it.
    #[tokio::test]
    async fn a_busy_database_is_not_labelled_a_conflict() {
        // A real SQLITE_BUSY, produced by holding a write lock on one connection while another
        // writes with no busy_timeout to wait it out.
        let dir = std::env::temp_dir().join(format!("heldar_busy_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("busy.db");
        let url = format!("sqlite://{}?mode=rwc", db.display());
        let holder = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE IF NOT EXISTS t (a INTEGER)")
            .execute(&holder)
            .await
            .unwrap();
        let mut tx = holder.begin().await.unwrap();
        sqlx::query("INSERT INTO t VALUES (1)")
            .execute(&mut *tx)
            .await
            .unwrap();

        // busy_timeout(0) so the second writer reports SQLITE_BUSY immediately instead of waiting.
        let opts: sqlx::sqlite::SqliteConnectOptions = url
            .parse::<sqlx::sqlite::SqliteConnectOptions>()
            .unwrap()
            .busy_timeout(std::time::Duration::from_millis(0));
        let other = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        let err = sqlx::query("INSERT INTO t VALUES (2)")
            .execute(&other)
            .await
            .expect_err("the write should hit the holder's lock");

        let (status, body) = body_of(AppError::Db(err)).await;
        let _ = tx.rollback().await;
        let _ = std::fs::remove_dir_all(&dir);

        // Whatever the classifier decided, the code and retryability must AGREE with it.
        assert_eq!(
            body["code"].as_str(),
            Some(AppError::code_for_status(status)),
            "code disagrees with the status the classifier chose ({status})"
        );
        assert_eq!(
            body["retryable"].as_bool(),
            Some(AppError::retryable_for_status(status))
        );
        if status == StatusCode::SERVICE_UNAVAILABLE {
            assert_eq!(body["code"].as_str(), Some("unavailable"));
            assert_eq!(body["retryable"].as_bool(), Some(true));
        }
    }

    /// The general form of the same rule: for every variant, the emitted code and retryability are
    /// the ones the chosen status implies. Nothing may classify twice.
    #[tokio::test]
    async fn code_and_retryable_always_agree_with_the_status() {
        for e in [
            AppError::NotFound("x".into()),
            AppError::BadRequest("x".into()),
            AppError::Conflict("x".into()),
            AppError::Unauthorized("x".into()),
            AppError::Forbidden("x".into()),
            AppError::Unavailable("x".into()),
            AppError::Db(sqlx::Error::PoolTimedOut),
            AppError::Db(sqlx::Error::RowNotFound),
            AppError::Other(anyhow::anyhow!("x")),
        ] {
            let (status, body) = body_of(e).await;
            assert_eq!(
                body["code"].as_str(),
                Some(AppError::code_for_status(status)),
                "code disagrees with status {status}"
            );
            assert_eq!(
                body["retryable"].as_bool(),
                Some(AppError::retryable_for_status(status)),
                "retryable disagrees with status {status}"
            );
        }
    }
}
