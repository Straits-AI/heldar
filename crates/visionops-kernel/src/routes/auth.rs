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
    let user = match candidate {
        Some(u) if u.active && password_ok => u,
        _ => return Err(AppError::Unauthorized("invalid credentials".into())),
    };
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
    // Guard against locking everyone out: do not let the last active admin be demoted/disabled.
    if cur.role == "admin" && (role != "admin" || !active) {
        let other_admins: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM users WHERE role = 'admin' AND active = 1 AND id != ?",
        )
        .bind(&id)
        .fetch_one(&st.pool)
        .await?;
        if other_admins == 0 {
            return Err(AppError::BadRequest(
                "cannot demote or disable the last active admin".into(),
            ));
        }
    }
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
    sqlx::query(
        "UPDATE users SET password_hash=?, role=?, display_name=?, active=?, updated_at=? WHERE id=?",
    )
    .bind(&password_hash)
    .bind(&role)
    .bind(&display_name)
    .bind(active)
    .bind(Utc::now())
    .bind(&id)
    .execute(&st.pool)
    .await?;
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
    if cur.role == "admin" {
        let other_admins: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM users WHERE role = 'admin' AND active = 1 AND id != ?",
        )
        .bind(&id)
        .fetch_one(&st.pool)
        .await?;
        if other_admins == 0 {
            return Err(AppError::BadRequest(
                "cannot delete the last active admin".into(),
            ));
        }
    }
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(&id)
        .execute(&st.pool)
        .await?;
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
