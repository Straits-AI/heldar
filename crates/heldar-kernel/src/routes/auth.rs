//! Stage 4 auth + user/API-key administration.
//!
//! `/auth/login` exchanges username+password for a bearer session token; `/auth/logout` revokes it;
//! `/auth/me` reports the caller. `/users` and `/api-keys` are admin-only management surfaces. All
//! mutations are written to the immutable audit log.

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{AppendHeaders, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::{self, Principal, Role};
use crate::error::{AppError, AppResult};
use crate::models::{
    ApiKey, ApiKeyCreate, ApiKeyView, LoginRequest, User, UserCreate, UserUpdate, UserView,
};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/me", get(me))
        .route("/api/v1/users", get(list_users).post(create_user))
        .route(
            "/api/v1/users/{id}",
            axum::routing::patch(update_user).delete(delete_user),
        )
        .route("/api/v1/users/{id}/unlock", post(unlock_user))
        .route("/api/v1/api-keys", get(list_api_keys).post(create_api_key))
        .route(
            "/api/v1/api-keys/{id}",
            axum::routing::delete(delete_api_key),
        )
}

const MIN_PASSWORD_LEN: usize = 8;

async fn login(
    State(st): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> AppResult<impl IntoResponse> {
    let candidate = sqlx::query_as::<_, User>("SELECT * FROM users WHERE username = ?")
        .bind(body.username.trim())
        .fetch_optional(&st.pool)
        .await?;
    // Always run argon2 verification (against a dummy hash when the user is missing/disabled) so
    // login latency does not reveal whether an account exists. The error is uniform too.
    let phc = candidate
        .as_ref()
        .map(|u| u.password_hash.as_str())
        .unwrap_or_else(|| auth::dummy_password_hash());
    let password_ok = auth::verify_password(&body.password, phc);

    let now = Utc::now();
    let lockout = st.cfg.login_lockout_enabled();
    let locked = lockout
        && candidate
            .as_ref()
            .and_then(|u| u.locked_until)
            .is_some_and(|until| until > now);
    let user = match candidate {
        // A locked account is refused EVEN with the correct password, and its counter is NOT advanced
        // (a locked-out attacker can't extend the lock). The response stays the uniform 401 — a distinct
        // "account locked" message would be a lock/enumeration oracle.
        Some(_) if locked => return Err(AppError::Unauthorized("invalid credentials".into())),
        Some(u) if u.active && password_ok => u,
        // Failed login for a real, active account: bump the counter and lock at the threshold.
        Some(u) if lockout && u.active => {
            register_login_failure(&st.pool, &st.cfg, &u, now).await;
            return Err(AppError::Unauthorized("invalid credentials".into()));
        }
        _ => return Err(AppError::Unauthorized("invalid credentials".into())),
    };

    // Success: clear any prior failure/lock state before issuing the session.
    if lockout && (user.failed_login_count != 0 || user.locked_until.is_some()) {
        let _ = sqlx::query(
            "UPDATE users SET failed_login_count = 0, locked_until = NULL WHERE id = ?",
        )
        .bind(&user.id)
        .execute(&st.pool)
        .await;
    }

    let (token, expires_at) = auth::issue_session(&st.pool, &st.cfg, &user.id).await?;
    let principal = Principal {
        id: user.id.clone(),
        name: user
            .display_name
            .clone()
            .unwrap_or_else(|| user.username.clone()),
        role: Role::parse(&user.role).unwrap_or(Role::Viewer),
        kind: crate::auth::PrincipalKind::User,
    };
    auth::audit(&st.pool, &principal, "login", "user", &user.id, json!({})).await;
    // Set the session as an HttpOnly cookie (browser auth: not JS-readable, so XSS can't exfiltrate
    // it; the media plane gets it automatically since the SPA is same-origin). The token is still in
    // the body for non-browser clients; browsers should ignore it and rely on the cookie.
    let cookie = auth::session_cookie(&token, &st.cfg);
    let body = Json(json!({
        "token": token,
        "expires_at": expires_at,
        "user": UserView::from(user),
    }));
    Ok((AppendHeaders([(header::SET_COOKIE, cookie)]), body))
}

/// Record a failed login for a real, active account; lock it once `login_max_failures` consecutive
/// failures are reached. Audits the lock transition (once) so a brute-force attempt is visible.
async fn register_login_failure(
    pool: &sqlx::SqlitePool,
    cfg: &crate::config::Config,
    u: &User,
    now: chrono::DateTime<Utc>,
) {
    let new_count = u.failed_login_count + 1;
    if new_count >= cfg.login_max_failures {
        let until = now + chrono::Duration::minutes(cfg.login_lockout_min);
        let _ =
            sqlx::query("UPDATE users SET failed_login_count = ?, locked_until = ? WHERE id = ?")
                .bind(new_count)
                .bind(until)
                .bind(&u.id)
                .execute(pool)
                .await;
        let principal = Principal {
            id: u.id.clone(),
            name: u.display_name.clone().unwrap_or_else(|| u.username.clone()),
            role: Role::parse(&u.role).unwrap_or(Role::Viewer),
            kind: auth::PrincipalKind::User,
        };
        auth::audit(
            pool,
            &principal,
            "login_locked",
            "user",
            &u.id,
            json!({ "locked_until": until }),
        )
        .await;
    } else {
        let _ = sqlx::query("UPDATE users SET failed_login_count = ? WHERE id = ?")
            .bind(new_count)
            .bind(&u.id)
            .execute(pool)
            .await;
    }
}

async fn logout(State(st): State<AppState>, headers: HeaderMap) -> AppResult<impl IntoResponse> {
    if let Some(tok) = auth::token_from_headers(&headers) {
        auth::revoke_session(&st.pool, &tok).await?;
    }
    // Clear the session cookie regardless (idempotent logout).
    let cookie = auth::clear_session_cookie(&st.cfg);
    Ok((
        StatusCode::NO_CONTENT,
        AppendHeaders([(header::SET_COOKIE, cookie)]),
    ))
}

async fn me(principal: Principal) -> AppResult<Json<Value>> {
    Ok(Json(json!({
        "id": principal.id,
        "name": principal.name,
        "role": principal.role.as_str(),
        "kind": match principal.kind {
            crate::auth::PrincipalKind::User => "user",
            crate::auth::PrincipalKind::ApiKey => "api_key",
            crate::auth::PrincipalKind::System => "system",
        },
    })))
}

async fn list_users(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<Vec<UserView>>> {
    principal.require(principal.can_admin(), "manage users")?;
    let users = sqlx::query_as::<_, User>("SELECT * FROM users ORDER BY username ASC")
        .fetch_all(&st.pool)
        .await?;
    Ok(Json(users.into_iter().map(UserView::from).collect()))
}

async fn create_user(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<UserCreate>,
) -> AppResult<(StatusCode, Json<UserView>)> {
    principal.require(principal.can_admin(), "create users")?;
    let username = body.username.trim();
    if username.is_empty() {
        return Err(AppError::BadRequest("`username` is required".into()));
    }
    if body.password.len() < MIN_PASSWORD_LEN {
        return Err(AppError::BadRequest(format!(
            "`password` must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    let role = body.role.as_deref().unwrap_or("viewer");
    if !Role::is_valid(role) {
        return Err(AppError::BadRequest(
            "`role` must be admin|manager|guard|viewer|integration".into(),
        ));
    }
    let hash = auth::hash_password(&body.password)?;
    let id = format!("usr_{}", Uuid::new_v4().simple());
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, role, display_name, active, created_at, updated_at)
         VALUES (?,?,?,?,?,?,?,?)",
    )
    .bind(&id)
    .bind(username)
    .bind(hash)
    .bind(role)
    .bind(&body.display_name)
    .bind(body.active.unwrap_or(true))
    .bind(now)
    .bind(now)
    .execute(&st.pool)
    .await?;
    auth::audit(
        &st.pool,
        &principal,
        "create_user",
        "user",
        &id,
        json!({ "role": role }),
    )
    .await;
    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(&id)
        .fetch_one(&st.pool)
        .await?;
    Ok((StatusCode::CREATED, Json(UserView::from(user))))
}

async fn update_user(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(body): Json<UserUpdate>,
) -> AppResult<Json<UserView>> {
    principal.require(principal.can_admin(), "modify users")?;
    let cur = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(&id)
        .fetch_optional(&st.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("user {id} not found")))?;

    let role = body.role.unwrap_or_else(|| cur.role.clone());
    if !Role::is_valid(&role) {
        return Err(AppError::BadRequest(
            "`role` must be admin|manager|guard|viewer|integration".into(),
        ));
    }
    let active = body.active.unwrap_or(cur.active);
    let display_name = body.display_name.or(cur.display_name);
    let password_hash = match body.password {
        Some(p) if p.len() >= MIN_PASSWORD_LEN => auth::hash_password(&p)?,
        Some(_) => {
            return Err(AppError::BadRequest(format!(
                "`password` must be at least {MIN_PASSWORD_LEN} characters"
            )))
        }
        None => cur.password_hash,
    };
    // Lockout guard, ATOMIC: when this change demotes/disables an admin, the UPDATE only applies if
    // ANOTHER active admin still exists at write time. SQLite serializes writers, so two concurrent
    // demotions of different admins cannot both succeed — the second finds the EXISTS false and is
    // rejected, always leaving an admin standing. (A separate COUNT-then-UPDATE would race.)
    let demoting_admin = cur.role == "admin" && (role != "admin" || !active);
    let affected = if demoting_admin {
        sqlx::query(
            "UPDATE users SET password_hash=?, role=?, display_name=?, active=?, updated_at=?, \
             failed_login_count=0, locked_until=NULL \
             WHERE id=? AND EXISTS (SELECT 1 FROM users WHERE role='admin' AND active=1 AND id != ?)",
        )
        .bind(&password_hash)
        .bind(&role)
        .bind(&display_name)
        .bind(active)
        .bind(Utc::now())
        .bind(&id)
        .bind(&id)
        .execute(&st.pool)
        .await?
        .rows_affected()
    } else {
        sqlx::query(
            "UPDATE users SET password_hash=?, role=?, display_name=?, active=?, updated_at=?, \
             failed_login_count=0, locked_until=NULL WHERE id=?",
        )
        .bind(&password_hash)
        .bind(&role)
        .bind(&display_name)
        .bind(active)
        .bind(Utc::now())
        .bind(&id)
        .execute(&st.pool)
        .await?
        .rows_affected()
    };
    if demoting_admin && affected == 0 {
        return Err(AppError::BadRequest(
            "cannot demote or disable the last active admin".into(),
        ));
    }
    // Revoke sessions if the account was disabled.
    if !active {
        let _ = sqlx::query("DELETE FROM sessions WHERE user_id = ?")
            .bind(&id)
            .execute(&st.pool)
            .await;
    }
    auth::audit(
        &st.pool,
        &principal,
        "update_user",
        "user",
        &id,
        json!({ "role": role, "active": active }),
    )
    .await;
    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(&id)
        .fetch_one(&st.pool)
        .await?;
    Ok(Json(UserView::from(user)))
}

/// Admin-only: clear a user's brute-force lockout (reset the failure counter + unlock immediately),
/// without otherwise editing the account. Auto-unlock still happens on its own once the window passes.
async fn unlock_user(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<UserView>> {
    principal.require(principal.can_admin(), "unlock users")?;
    let res =
        sqlx::query("UPDATE users SET failed_login_count = 0, locked_until = NULL WHERE id = ?")
            .bind(&id)
            .execute(&st.pool)
            .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("user {id} not found")));
    }
    auth::audit(&st.pool, &principal, "unlock_user", "user", &id, json!({})).await;
    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(&id)
        .fetch_one(&st.pool)
        .await?;
    Ok(Json(UserView::from(user)))
}

async fn delete_user(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    principal.require(principal.can_admin(), "delete users")?;
    if principal.id == id {
        return Err(AppError::BadRequest(
            "cannot delete your own account".into(),
        ));
    }
    let cur = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(&id)
        .fetch_optional(&st.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("user {id} not found")))?;
    // Atomic last-admin guard (see update_user): the conditional DELETE removes an admin only if
    // another active admin still exists, so concurrent deletes cannot drain the admins to zero.
    let affected = if cur.role == "admin" {
        sqlx::query(
            "DELETE FROM users WHERE id = ? AND EXISTS (SELECT 1 FROM users WHERE role='admin' AND active=1 AND id != ?)",
        )
        .bind(&id)
        .bind(&id)
        .execute(&st.pool)
        .await?
        .rows_affected()
    } else {
        sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(&id)
            .execute(&st.pool)
            .await?
            .rows_affected()
    };
    if cur.role == "admin" && affected == 0 {
        return Err(AppError::BadRequest(
            "cannot delete the last active admin".into(),
        ));
    }
    auth::audit(&st.pool, &principal, "delete_user", "user", &id, json!({})).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_api_keys(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<Vec<ApiKeyView>>> {
    principal.require(principal.can_admin(), "manage API keys")?;
    let keys = sqlx::query_as::<_, ApiKey>("SELECT * FROM api_keys ORDER BY created_at DESC")
        .fetch_all(&st.pool)
        .await?;
    Ok(Json(keys.into_iter().map(ApiKeyView::from).collect()))
}

async fn create_api_key(
    State(st): State<AppState>,
    principal: Principal,
    Json(body): Json<ApiKeyCreate>,
) -> AppResult<(StatusCode, Json<Value>)> {
    principal.require(principal.can_admin(), "create API keys")?;
    if body.name.trim().is_empty() {
        return Err(AppError::BadRequest("`name` is required".into()));
    }
    let role = body.role.as_deref().unwrap_or("integration");
    if !Role::is_valid(role) {
        return Err(AppError::BadRequest(
            "`role` must be admin|manager|guard|viewer|integration".into(),
        ));
    }
    let key = auth::random_token(auth::APIKEY_PREFIX);
    let prefix: String = key.chars().take(12).collect();
    let id = format!("key_{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO api_keys (id, name, key_hash, key_prefix, role, active, created_at)
         VALUES (?,?,?,?,?,1,?)",
    )
    .bind(&id)
    .bind(body.name.trim())
    .bind(auth::token_hash(&key))
    .bind(&prefix)
    .bind(role)
    .bind(Utc::now())
    .execute(&st.pool)
    .await?;
    auth::audit(
        &st.pool,
        &principal,
        "create_api_key",
        "api_key",
        &id,
        json!({ "role": role }),
    )
    .await;
    // The full key is returned exactly once; only its hash is stored.
    Ok((
        StatusCode::CREATED,
        Json(json!({ "id": id, "name": body.name.trim(), "role": role, "key": key })),
    ))
}

async fn delete_api_key(
    State(st): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    principal.require(principal.can_admin(), "delete API keys")?;
    let res = sqlx::query("DELETE FROM api_keys WHERE id = ?")
        .bind(&id)
        .execute(&st.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("api key {id} not found")));
    }
    auth::audit(
        &st.pool,
        &principal,
        "delete_api_key",
        "api_key",
        &id,
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::services::recorder::RecorderManager;
    use crate::services::sampler::SamplerManager;
    use std::sync::Arc;

    /// Build a minimal in-memory AppState (single connection so the :memory: DB persists across
    /// queries) with real migrations applied, mirroring the helper used by the other route tests.
    async fn test_state(auth_enabled: bool) -> AppState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let mut cfg = Config::from_env();
        cfg.auth_enabled = auth_enabled;
        let cfg = Arc::new(cfg);
        AppState {
            recorder: RecorderManager::new(pool.clone(), cfg.clone()),
            sampler: SamplerManager::new(pool.clone(), cfg.clone()),
            mirror: None,
            consumers: Arc::new(Vec::new()),
            modules: Arc::new(Vec::new()),
            catalog: Arc::new(crate::services::registry::CatalogService::new(&cfg)),
            http: reqwest::Client::new(),
            started_at: chrono::Utc::now(),
            pool,
            cfg,
        }
    }

    fn viewer() -> Principal {
        Principal {
            id: "usr_viewer".into(),
            name: "vee".into(),
            role: Role::Viewer,
            kind: auth::PrincipalKind::User,
        }
    }

    #[tokio::test]
    async fn me_reports_principal_role_and_kind() {
        // System admin (auth-disabled implicit principal) reports role=admin, kind=system.
        let Json(v) = me(Principal::system_admin()).await.unwrap();
        assert_eq!(v["id"], "system");
        assert_eq!(v["name"], "system");
        assert_eq!(v["role"], "admin");
        assert_eq!(v["kind"], "system");

        // A user-kind principal maps to kind=user and echoes its role.
        let Json(v) = me(viewer()).await.unwrap();
        assert_eq!(v["role"], "viewer");
        assert_eq!(v["kind"], "user");
    }

    #[tokio::test]
    async fn create_user_validation_rejects_bad_input() {
        let st = test_state(false).await;

        // Empty (whitespace-only) username.
        let err = create_user(
            State(st.clone()),
            Principal::system_admin(),
            Json(UserCreate {
                username: "   ".into(),
                password: "x".repeat(MIN_PASSWORD_LEN),
                role: None,
                display_name: None,
                active: None,
            }),
        )
        .await
        .err()
        .unwrap();
        match err {
            AppError::BadRequest(m) => assert!(m.contains("username")),
            other => panic!("expected BadRequest, got {other:?}"),
        }

        // Password shorter than MIN_PASSWORD_LEN.
        let err = create_user(
            State(st.clone()),
            Principal::system_admin(),
            Json(UserCreate {
                username: "joe".into(),
                password: "x".repeat(MIN_PASSWORD_LEN - 1),
                role: None,
                display_name: None,
                active: None,
            }),
        )
        .await
        .err()
        .unwrap();
        match err {
            AppError::BadRequest(m) => assert!(m.contains("password")),
            other => panic!("expected BadRequest, got {other:?}"),
        }

        // Unrecognized role.
        let err = create_user(
            State(st.clone()),
            Principal::system_admin(),
            Json(UserCreate {
                username: "joe".into(),
                password: "x".repeat(MIN_PASSWORD_LEN),
                role: Some("superuser".into()),
                display_name: None,
                active: None,
            }),
        )
        .await
        .err()
        .unwrap();
        match err {
            AppError::BadRequest(m) => assert!(m.contains("role")),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn create_user_defaults_and_list_orders() {
        let st = test_state(false).await;

        // Surrounding whitespace is trimmed; role defaults to viewer; active defaults to true.
        let (status, Json(uv)) = create_user(
            State(st.clone()),
            Principal::system_admin(),
            Json(UserCreate {
                username: "  bravo  ".into(),
                password: "x".repeat(MIN_PASSWORD_LEN),
                role: None,
                display_name: None,
                active: None,
            }),
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(uv.username, "bravo");
        assert_eq!(uv.role, "viewer");
        assert!(uv.active);

        let _ = create_user(
            State(st.clone()),
            Principal::system_admin(),
            Json(UserCreate {
                username: "alpha".into(),
                password: "x".repeat(MIN_PASSWORD_LEN),
                role: Some("manager".into()),
                display_name: Some("Al".into()),
                active: None,
            }),
        )
        .await
        .unwrap();

        // list_users is ordered by username ASC.
        let Json(users) = list_users(State(st.clone()), Principal::system_admin())
            .await
            .unwrap();
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].username, "alpha");
        assert_eq!(users[1].username, "bravo");
        assert_eq!(users[0].role, "manager");
    }

    #[tokio::test]
    async fn non_admin_is_forbidden() {
        let st = test_state(false).await;

        let err = list_users(State(st.clone()), viewer()).await.err().unwrap();
        assert!(matches!(err, AppError::Forbidden(_)));

        let err = create_api_key(
            State(st.clone()),
            viewer(),
            Json(ApiKeyCreate {
                name: "k".into(),
                role: None,
            }),
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(err, AppError::Forbidden(_)));
    }

    #[tokio::test]
    async fn delete_user_rejects_self() {
        let st = test_state(false).await;
        // system_admin has id "system"; deleting that same id hits the self-deletion guard before
        // any existence check.
        let err = delete_user(
            State(st.clone()),
            Principal::system_admin(),
            Path("system".to_string()),
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[tokio::test]
    async fn update_user_protects_last_admin() {
        let st = test_state(false).await;

        // The only admin in the table.
        let (_, Json(admin)) = create_user(
            State(st.clone()),
            Principal::system_admin(),
            Json(UserCreate {
                username: "rootadmin".into(),
                password: "x".repeat(MIN_PASSWORD_LEN),
                role: Some("admin".into()),
                display_name: None,
                active: None,
            }),
        )
        .await
        .unwrap();

        // Demoting the last active admin is refused.
        let err = update_user(
            State(st.clone()),
            Principal::system_admin(),
            Path(admin.id.clone()),
            Json(UserUpdate {
                role: Some("viewer".into()),
                ..Default::default()
            }),
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    /// AppState around a caller-provided pool — for concurrency tests needing a shared,
    /// multi-connection DB (the single-connection in-memory `test_state` would serialize the race).
    async fn state_with_pool(pool: sqlx::SqlitePool) -> AppState {
        let mut cfg = Config::from_env();
        cfg.auth_enabled = false;
        let cfg = std::sync::Arc::new(cfg);
        AppState {
            recorder: RecorderManager::new(pool.clone(), cfg.clone()),
            sampler: SamplerManager::new(pool.clone(), cfg.clone()),
            mirror: None,
            consumers: std::sync::Arc::new(Vec::new()),
            modules: std::sync::Arc::new(Vec::new()),
            catalog: std::sync::Arc::new(crate::services::registry::CatalogService::new(&cfg)),
            http: reqwest::Client::new(),
            started_at: chrono::Utc::now(),
            pool,
            cfg,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_demotion_cannot_drain_the_last_admin() {
        // Temp-FILE DB so the pool's connections see each other's committed writes — the
        // single-connection in-memory pool used elsewhere would serialize and hide the race.
        let dbpath =
            std::env::temp_dir().join(format!("heldar-authrace-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&dbpath);
        let url = format!("sqlite://{}?mode=rwc", dbpath.display());
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let st = state_with_pool(pool.clone()).await;

        // Exactly two active admins.
        let mut ids = Vec::new();
        for u in ["admin_a", "admin_b"] {
            let (_, Json(v)) = create_user(
                State(st.clone()),
                Principal::system_admin(),
                Json(UserCreate {
                    username: u.into(),
                    password: "x".repeat(MIN_PASSWORD_LEN),
                    role: Some("admin".into()),
                    display_name: None,
                    active: None,
                }),
            )
            .await
            .unwrap();
            ids.push(v.id);
        }

        let demote = || {
            Json(UserUpdate {
                role: Some("viewer".into()),
                ..Default::default()
            })
        };
        // Demote BOTH admins at once. Old check-then-act: both pass -> zero admins. Atomic guard:
        // at least one is rejected, an admin always remains.
        let (r1, r2) = tokio::join!(
            update_user(
                State(st.clone()),
                Principal::system_admin(),
                Path(ids[0].clone()),
                demote(),
            ),
            update_user(
                State(st.clone()),
                Principal::system_admin(),
                Path(ids[1].clone()),
                demote(),
            ),
        );

        let rejected = [r1.is_err(), r2.is_err()]
            .into_iter()
            .filter(|e| *e)
            .count();
        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role='admin' AND active=1")
                .fetch_one(&pool)
                .await
                .unwrap();
        let _ = std::fs::remove_file(&dbpath);
        assert!(
            remaining >= 1,
            "LOCKOUT: concurrent demotions drained all active admins (remaining={remaining})"
        );
        assert!(
            rejected >= 1,
            "at least one of two concurrent last-admin demotions must be rejected"
        );
    }

    #[tokio::test]
    async fn create_api_key_shape_and_validation() {
        let st = test_state(false).await;

        // Empty name is rejected.
        let err = create_api_key(
            State(st.clone()),
            Principal::system_admin(),
            Json(ApiKeyCreate {
                name: "  ".into(),
                role: None,
            }),
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(err, AppError::BadRequest(_)));

        // Valid creation: role defaults to integration, the secret is prefixed and returned once.
        let (status, Json(v)) = create_api_key(
            State(st.clone()),
            Principal::system_admin(),
            Json(ApiKeyCreate {
                name: "  cam-bridge  ".into(),
                role: None,
            }),
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(v["name"], "cam-bridge");
        assert_eq!(v["role"], "integration");
        let key = v["key"].as_str().unwrap();
        assert!(key.starts_with(auth::APIKEY_PREFIX));
    }

    #[tokio::test]
    async fn login_unknown_wrong_then_success() {
        let st = test_state(false).await;

        // No users yet -> unknown user is uniformly Unauthorized.
        let err = login(
            State(st.clone()),
            Json(LoginRequest {
                username: "ghost".into(),
                password: "whatever1".into(),
            }),
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(err, AppError::Unauthorized(_)));

        // Seed an operator.
        let _ = create_user(
            State(st.clone()),
            Principal::system_admin(),
            Json(UserCreate {
                username: "operator".into(),
                password: "operator-pass".into(),
                role: Some("manager".into()),
                display_name: None,
                active: None,
            }),
        )
        .await
        .unwrap();

        // Wrong password for an existing user is also Unauthorized.
        let err = login(
            State(st.clone()),
            Json(LoginRequest {
                username: "operator".into(),
                password: "not-the-pass".into(),
            }),
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(err, AppError::Unauthorized(_)));

        // Correct credentials succeed: 200, an HttpOnly session cookie, and one persisted session.
        let resp = login(
            State(st.clone()),
            Json(LoginRequest {
                username: "operator".into(),
                password: "operator-pass".into(),
            }),
        )
        .await
        .unwrap()
        .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let set_cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(set_cookie.contains(auth::SESSION_COOKIE));
        assert!(set_cookie.contains("HttpOnly"));

        let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
            .fetch_one(&st.pool)
            .await
            .unwrap();
        assert_eq!(sessions, 1);
    }

    // ---- Per-account login lockout ----

    async fn test_state_lockout(max_failures: i64, lockout_min: i64) -> AppState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let mut cfg = Config::from_env();
        cfg.auth_enabled = true;
        cfg.login_max_failures = max_failures;
        cfg.login_lockout_min = lockout_min;
        let cfg = Arc::new(cfg);
        AppState {
            recorder: RecorderManager::new(pool.clone(), cfg.clone()),
            sampler: SamplerManager::new(pool.clone(), cfg.clone()),
            mirror: None,
            consumers: Arc::new(Vec::new()),
            modules: Arc::new(Vec::new()),
            catalog: Arc::new(crate::services::registry::CatalogService::new(&cfg)),
            http: reqwest::Client::new(),
            started_at: chrono::Utc::now(),
            pool,
            cfg,
        }
    }

    async fn seed_user(st: &AppState, username: &str, password: &str) {
        let _ = create_user(
            State(st.clone()),
            Principal::system_admin(),
            Json(UserCreate {
                username: username.into(),
                password: password.into(),
                role: Some("manager".into()),
                display_name: None,
                active: None,
            }),
        )
        .await
        .unwrap();
    }

    async fn try_login(
        st: &AppState,
        username: &str,
        password: &str,
    ) -> AppResult<impl IntoResponse> {
        login(
            State(st.clone()),
            Json(LoginRequest {
                username: username.into(),
                password: password.into(),
            }),
        )
        .await
    }

    #[tokio::test]
    async fn login_locks_after_max_failures_and_rejects_correct_password() {
        let st = test_state_lockout(3, 15).await;
        seed_user(&st, "op", "correct-pass").await;
        for _ in 0..3 {
            assert!(matches!(
                try_login(&st, "op", "wrong").await.err().unwrap(),
                AppError::Unauthorized(_)
            ));
        }
        let (count, locked): (i64, Option<String>) = sqlx::query_as(
            "SELECT failed_login_count, locked_until FROM users WHERE username='op'",
        )
        .fetch_one(&st.pool)
        .await
        .unwrap();
        assert_eq!(count, 3);
        assert!(
            locked.is_some(),
            "account should be locked after 3 failures"
        );

        // The CORRECT password is now refused, and NO session is created.
        assert!(matches!(
            try_login(&st, "op", "correct-pass").await.err().unwrap(),
            AppError::Unauthorized(_)
        ));
        let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
            .fetch_one(&st.pool)
            .await
            .unwrap();
        assert_eq!(sessions, 0, "a locked account must not get a session");
    }

    #[tokio::test]
    async fn successful_login_resets_failure_count() {
        let st = test_state_lockout(3, 15).await;
        seed_user(&st, "op", "correct-pass").await;
        let _ = try_login(&st, "op", "wrong").await;
        let _ = try_login(&st, "op", "wrong").await; // 2 < threshold 3
        let resp = try_login(&st, "op", "correct-pass")
            .await
            .unwrap()
            .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let (count, locked): (i64, Option<String>) = sqlx::query_as(
            "SELECT failed_login_count, locked_until FROM users WHERE username='op'",
        )
        .fetch_one(&st.pool)
        .await
        .unwrap();
        assert_eq!(count, 0);
        assert!(locked.is_none());
    }

    #[tokio::test]
    async fn auto_unlock_after_window() {
        let st = test_state_lockout(2, 15).await;
        seed_user(&st, "op", "correct-pass").await;
        let _ = try_login(&st, "op", "wrong").await;
        let _ = try_login(&st, "op", "wrong").await;
        // Simulate the lock window elapsing by backdating locked_until.
        sqlx::query("UPDATE users SET locked_until = ? WHERE username='op'")
            .bind(Utc::now() - chrono::Duration::minutes(1))
            .execute(&st.pool)
            .await
            .unwrap();
        let resp = try_login(&st, "op", "correct-pass")
            .await
            .unwrap()
            .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn manual_unlock_clears_lock() {
        let st = test_state_lockout(2, 15).await;
        seed_user(&st, "op", "correct-pass").await;
        let _ = try_login(&st, "op", "wrong").await;
        let _ = try_login(&st, "op", "wrong").await;
        let uid: String = sqlx::query_scalar("SELECT id FROM users WHERE username='op'")
            .fetch_one(&st.pool)
            .await
            .unwrap();
        let _ = unlock_user(State(st.clone()), Principal::system_admin(), Path(uid))
            .await
            .unwrap();
        let resp = try_login(&st, "op", "correct-pass")
            .await
            .unwrap()
            .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn lockout_disabled_when_zero_never_locks() {
        let st = test_state_lockout(0, 15).await; // disabled
        seed_user(&st, "op", "correct-pass").await;
        for _ in 0..10 {
            let _ = try_login(&st, "op", "wrong").await;
        }
        let locked: Option<String> =
            sqlx::query_scalar("SELECT locked_until FROM users WHERE username='op'")
                .fetch_one(&st.pool)
                .await
                .unwrap();
        assert!(locked.is_none(), "lockout disabled must never lock");
        let resp = try_login(&st, "op", "correct-pass")
            .await
            .unwrap()
            .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn logout_is_no_content_and_clears_cookie() {
        let st = test_state(false).await;
        // No credentials present -> still a clean, idempotent logout.
        let resp = logout(State(st.clone()), HeaderMap::new())
            .await
            .unwrap()
            .into_response();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let set_cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(set_cookie.contains(auth::SESSION_COOKIE));
        assert!(set_cookie.contains("Max-Age=0"));
    }
}
