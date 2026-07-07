//! Stage 4 authentication + RBAC.
//!
//! Two principal kinds carry a role: interactive **users** (password login → opaque bearer session)
//! and machine **API keys** (worker ingest + external integration). Tokens are random 256-bit
//! values; only their SHA-256 is stored, so a database leak does not expose usable credentials.
//! Passwords are argon2id PHC hashes.
//!
//! The [`Principal`] extractor resolves the caller from the `Authorization: Bearer` (or `X-API-Key`)
//! header. When `auth_enabled` is false (the default single-tenant LAN appliance mode) it yields a
//! synthetic admin so the existing open API and tooling keep working; when true it requires a valid
//! token and 401s otherwise. Handlers then assert capabilities with [`Principal::require`].

use std::fmt::Write as _;

use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier};
use argon2::{password_hash::SaltString, Argon2};
use axum::extract::FromRequestParts;
use axum::http::header;
use axum::http::request::Parts;
use chrono::{DateTime, Duration, Utc};
use rand_core::RngCore;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub const SESSION_PREFIX: &str = "vos_";
pub const APIKEY_PREFIX: &str = "vok_";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Admin,
    Manager,
    Guard,
    Viewer,
    Integration,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Manager => "manager",
            Role::Guard => "guard",
            Role::Viewer => "viewer",
            Role::Integration => "integration",
        }
    }
    pub fn parse(s: &str) -> Option<Role> {
        Some(match s {
            "admin" => Role::Admin,
            "manager" => Role::Manager,
            "guard" => Role::Guard,
            "viewer" => Role::Viewer,
            "integration" => Role::Integration,
            _ => return None,
        })
    }
    pub fn is_valid(s: &str) -> bool {
        Role::parse(s).is_some()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrincipalKind {
    User,
    ApiKey,
    System,
}

/// The resolved caller for a request.
#[derive(Clone, Debug)]
pub struct Principal {
    pub id: String,
    pub name: String,
    pub role: Role,
    pub kind: PrincipalKind,
}

impl Principal {
    /// The implicit principal used when auth is disabled.
    pub fn system_admin() -> Self {
        Principal {
            id: "system".into(),
            name: "system".into(),
            role: Role::Admin,
            kind: PrincipalKind::System,
        }
    }

    pub fn can_admin(&self) -> bool {
        self.role == Role::Admin
    }
    /// Manage the registry: vehicles + watchlist.
    pub fn can_manage_registry(&self) -> bool {
        matches!(self.role, Role::Admin | Role::Manager)
    }
    /// Operate the gate: visitor check-in/out, create passes, confirm/reject entries.
    pub fn can_operate_gate(&self) -> bool {
        matches!(self.role, Role::Admin | Role::Manager | Role::Guard)
    }
    /// Post perception/ANPR events into the entry pipeline (machine clients + admins).
    pub fn can_ingest(&self) -> bool {
        matches!(self.role, Role::Admin | Role::Integration)
    }
    /// Read the entry surface. Every authenticated principal can read.
    pub fn can_view(&self) -> bool {
        true
    }

    /// Assert a capability, returning 403 with a useful message otherwise.
    pub fn require(&self, allowed: bool, action: &str) -> AppResult<()> {
        if allowed {
            Ok(())
        } else {
            Err(AppError::Forbidden(format!(
                "role `{}` is not permitted to {action}",
                self.role.as_str()
            )))
        }
    }
}

pub fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// SHA-256 hex of a token string — the at-rest representation of sessions / API keys.
pub fn token_hash(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex_encode(&h.finalize())
}

/// Generate a prefixed 256-bit random token (the full secret returned to the caller once).
pub fn random_token(prefix: &str) -> String {
    let mut buf = [0u8; 32];
    OsRng.fill_bytes(&mut buf);
    format!("{prefix}{}", hex_encode(&buf))
}

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("hashing password: {e}"))
}

pub fn verify_password(password: &str, phc: &str) -> bool {
    match PasswordHash::new(phc) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// A throwaway argon2id hash used to equalize login timing for unknown/disabled users (so the
/// presence of an account cannot be inferred from response latency). Computed once, lazily.
pub fn dummy_password_hash() -> &'static str {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DUMMY
        .get_or_init(|| hash_password("timing-equalizer-not-a-real-credential").unwrap_or_default())
}

/// Issue a login session for a user, returning the bearer token (shown once) and its expiry.
pub async fn issue_session(
    pool: &SqlitePool,
    cfg: &Config,
    user_id: &str,
) -> sqlx::Result<(String, DateTime<Utc>)> {
    let token = random_token(SESSION_PREFIX);
    let now = Utc::now();
    let expires_at = now + Duration::hours(cfg.session_ttl_hours.max(1));
    sqlx::query(
        "INSERT INTO sessions (id, user_id, created_at, expires_at, last_used_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(token_hash(&token))
    .bind(user_id)
    .bind(now)
    .bind(expires_at)
    .bind(now)
    .execute(pool)
    .await?;
    Ok((token, expires_at))
}

/// Revoke a session by its bearer token (idempotent).
pub async fn revoke_session(pool: &SqlitePool, token: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM sessions WHERE id = ?")
        .bind(token_hash(token))
        .execute(pool)
        .await?;
    Ok(())
}

/// Extract the bearer token from `Authorization: Bearer <t>` or the `X-API-Key` header.
pub fn token_from_headers(headers: &axum::http::HeaderMap) -> Option<String> {
    if let Some(h) = headers.get(header::AUTHORIZATION) {
        if let Ok(s) = h.to_str() {
            let s = s.trim();
            if let Some(rest) = s
                .strip_prefix("Bearer ")
                .or_else(|| s.strip_prefix("bearer "))
            {
                let t = rest.trim();
                if !t.is_empty() {
                    return Some(t.to_string());
                }
            }
        }
    }
    if let Some(h) = headers.get("x-api-key") {
        if let Ok(s) = h.to_str() {
            let t = s.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    // Browser session: the HttpOnly `heldar_session` cookie. Checked last so API clients/workers that
    // present an explicit Bearer / X-API-Key header still take precedence.
    if let Some(h) = headers.get(header::COOKIE) {
        if let Ok(s) = h.to_str() {
            let prefix = format!("{SESSION_COOKIE}=");
            for part in s.split(';') {
                if let Some(v) = part.trim().strip_prefix(&prefix) {
                    let t = v.trim();
                    if !t.is_empty() {
                        return Some(t.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Name of the HttpOnly session cookie set on login.
pub const SESSION_COOKIE: &str = "heldar_session";

/// Build the `Set-Cookie` value that stores a session token in an HttpOnly, SameSite=Strict cookie.
/// HttpOnly keeps it unreadable to JS (no XSS exfiltration); SameSite=Strict blocks CSRF; the SPA is
/// same-origin with the API so the cookie still reaches the media plane (`<img>`/`<video>`/HLS).
pub fn session_cookie(token: &str, cfg: &Config) -> String {
    let max_age = cfg.session_ttl_hours.max(1) * 3600;
    let secure = if cfg.auth_cookie_secure {
        "; Secure"
    } else {
        ""
    };
    format!(
        "{SESSION_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={max_age}{secure}"
    )
}

/// Build the `Set-Cookie` value that clears the session cookie (logout).
pub fn clear_session_cookie(cfg: &Config) -> String {
    let secure = if cfg.auth_cookie_secure {
        "; Secure"
    } else {
        ""
    };
    format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0{secure}")
}

/// Resolve a token to a principal, or None if it is unknown / expired / idle-timed-out / disabled.
/// `idle_minutes > 0` rejects a session unused for longer than that (independent of its absolute TTL).
async fn resolve_token(
    pool: &SqlitePool,
    token: &str,
    idle_minutes: i64,
) -> AppResult<Option<Principal>> {
    let hash = token_hash(token);
    let now = Utc::now();
    if token.starts_with(APIKEY_PREFIX) {
        let row: Option<(String, String, String, bool)> =
            sqlx::query_as("SELECT id, name, role, active FROM api_keys WHERE key_hash = ?")
                .bind(&hash)
                .fetch_optional(pool)
                .await?;
        if let Some((id, name, role, active)) = row {
            if !active {
                return Ok(None);
            }
            // An unparseable stored role means a corrupt/tampered row — deny rather than fail open
            // to a capability-bearing default.
            let Some(role) = Role::parse(&role) else {
                tracing::error!(api_key = %id, role = %role, "auth: api key has unparseable role; denying");
                return Ok(None);
            };
            // Best-effort last-used stamp (does not gate the request).
            let _ = sqlx::query("UPDATE api_keys SET last_used_at = ? WHERE id = ?")
                .bind(now)
                .bind(&id)
                .execute(pool)
                .await;
            return Ok(Some(Principal {
                id,
                name,
                role,
                kind: PrincipalKind::ApiKey,
            }));
        }
        return Ok(None);
    }
    // Otherwise treat as a session token.
    let row: Option<SessionRow> = sqlx::query_as(
        "SELECT s.id AS sid, s.expires_at, s.last_used_at, u.id AS uid, u.display_name, u.role, u.active
           FROM sessions s JOIN users u ON u.id = s.user_id
          WHERE s.id = ?",
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await?;
    if let Some(r) = row {
        // Absolute TTL, then idle timeout — either drops the session.
        let idle_expired =
            idle_minutes > 0 && r.last_used_at < now - Duration::minutes(idle_minutes);
        if r.expires_at <= now || idle_expired {
            let _ = sqlx::query("DELETE FROM sessions WHERE id = ?")
                .bind(&r.sid)
                .execute(pool)
                .await;
            return Ok(None);
        }
        if !r.active {
            return Ok(None);
        }
        let Some(role) = Role::parse(&r.role) else {
            tracing::error!(user = %r.uid, role = %r.role, "auth: user has unparseable role; denying");
            return Ok(None);
        };
        let _ = sqlx::query("UPDATE sessions SET last_used_at = ? WHERE id = ?")
            .bind(now)
            .bind(&r.sid)
            .execute(pool)
            .await;
        return Ok(Some(Principal {
            id: r.uid,
            name: r.display_name.unwrap_or_default(),
            role,
            kind: PrincipalKind::User,
        }));
    }
    Ok(None)
}

/// A session joined to its user, for token resolution.
#[derive(sqlx::FromRow)]
struct SessionRow {
    sid: String,
    expires_at: DateTime<Utc>,
    last_used_at: DateTime<Utc>,
    uid: String,
    display_name: Option<String>,
    role: String,
    active: bool,
}

impl FromRequestParts<AppState> for Principal {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Self::Rejection> {
        match token_from_headers(&parts.headers) {
            Some(tok) => {
                match resolve_token(&st.pool, &tok, st.cfg.session_idle_timeout_minutes).await? {
                    Some(p) => Ok(p),
                    None => {
                        if st.cfg.auth_enabled {
                            Err(AppError::Unauthorized(
                                "invalid or expired credentials".into(),
                            ))
                        } else {
                            Ok(Principal::system_admin())
                        }
                    }
                }
            }
            None => {
                if st.cfg.auth_enabled {
                    Err(AppError::Unauthorized("authentication required".into()))
                } else {
                    Ok(Principal::system_admin())
                }
            }
        }
    }
}

/// First-run bootstrap: when auth is enabled and no users exist yet, seed an admin from env.
pub async fn ensure_bootstrap(pool: &SqlitePool, cfg: &Config) -> anyhow::Result<()> {
    if !cfg.auth_enabled {
        return Ok(());
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await?;
    if count > 0 {
        return Ok(());
    }
    match (&cfg.bootstrap_admin_user, &cfg.bootstrap_admin_password) {
        (Some(user), Some(pass)) if !user.trim().is_empty() && pass.len() >= 8 => {
            let hash = hash_password(pass)?;
            let now = Utc::now();
            sqlx::query(
                "INSERT INTO users (id, username, password_hash, role, display_name, active, created_at, updated_at)
                 VALUES (?, ?, ?, 'admin', ?, 1, ?, ?)",
            )
            .bind(format!("usr_{}", uuid::Uuid::new_v4().simple()))
            .bind(user.trim())
            .bind(hash)
            .bind(user.trim())
            .bind(now)
            .bind(now)
            .execute(pool)
            .await?;
            tracing::warn!(user = %user.trim(), "auth: bootstrapped initial admin user from env");
        }
        (Some(_), Some(_)) => {
            tracing::error!(
                "auth: HELDAR_BOOTSTRAP_ADMIN_PASSWORD must be >= 8 chars; no admin created"
            );
        }
        _ => {
            tracing::warn!(
                "auth: enabled but no users exist and HELDAR_BOOTSTRAP_ADMIN_USER/PASSWORD not set; \
                 login is impossible until a user is created (seed one via env then restart)"
            );
        }
    }
    Ok(())
}

/// Append an immutable audit-log entry (best-effort; never fails the caller).
pub async fn audit(
    pool: &SqlitePool,
    actor: &Principal,
    action: &str,
    target_type: &str,
    target_id: &str,
    detail: serde_json::Value,
) {
    let res = sqlx::query(
        "INSERT INTO audit_log (id, actor, actor_name, role, action, target_type, target_id, detail, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(format!("aud_{}", uuid::Uuid::new_v4().simple()))
    .bind(&actor.id)
    .bind(&actor.name)
    .bind(actor.role.as_str())
    .bind(action)
    .bind(target_type)
    .bind(target_id)
    .bind(sqlx::types::Json(detail))
    .bind(Utc::now())
    .execute(pool)
    .await;
    if let Err(e) = res {
        tracing::error!(error = %e, action, "audit: failed to write audit log entry");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hash_roundtrip() {
        let h = hash_password("correct-horse-battery-staple").unwrap();
        assert!(verify_password("correct-horse-battery-staple", &h));
        assert!(!verify_password("wrong", &h));
    }

    #[test]
    fn token_hash_is_stable_and_distinct() {
        assert_eq!(token_hash("abc"), token_hash("abc"));
        assert_ne!(token_hash("abc"), token_hash("abd"));
        assert_eq!(token_hash("abc").len(), 64);
    }

    #[test]
    fn random_tokens_are_unique_and_prefixed() {
        let a = random_token(SESSION_PREFIX);
        let b = random_token(SESSION_PREFIX);
        assert_ne!(a, b);
        assert!(a.starts_with(SESSION_PREFIX));
        assert_eq!(a.len(), SESSION_PREFIX.len() + 64);
    }

    #[test]
    fn role_parse_roundtrip() {
        for r in ["admin", "manager", "guard", "viewer", "integration"] {
            assert_eq!(Role::parse(r).unwrap().as_str(), r);
        }
        assert!(Role::parse("root").is_none());
    }

    #[test]
    fn capability_matrix() {
        let admin = Principal {
            role: Role::Admin,
            ..Principal::system_admin()
        };
        let guard = Principal {
            role: Role::Guard,
            ..Principal::system_admin()
        };
        let integ = Principal {
            role: Role::Integration,
            ..Principal::system_admin()
        };
        assert!(admin.can_admin() && admin.can_ingest() && admin.can_manage_registry());
        assert!(guard.can_operate_gate() && !guard.can_manage_registry() && !guard.can_admin());
        assert!(integ.can_ingest() && !integ.can_operate_gate());
    }
}
