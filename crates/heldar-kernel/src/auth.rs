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

/// In-memory liveness for sessions, keyed by the hashed session id (never the raw token).
///
/// The idle timeout asks "was this operator active recently?", and the durable answer is
/// `sessions.last_used_at`. But that stamp is a debounced, best-effort WRITE: under write pressure it
/// can hit the pool's `busy_timeout` and be dropped, and while the box is unreachable the dashboard's
/// polls never arrive to refresh it at all. Because the idle check then *deletes* the session, an
/// operator who never stopped working gets logged out — worst of all during an outage, which is exactly
/// when they are trying to diagnose something.
///
/// So liveness is tracked here too, and the idle check uses whichever signal is newer. This map is
/// authoritative for "recently seen" while the process lives; `last_used_at` remains the restart-durable
/// fallback. Entries are bounded: anything past the idle window is dropped once the map grows.
static SESSION_SEEN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, i64>>> =
    std::sync::OnceLock::new();

/// Above this many tracked sessions, prune entries older than the idle window on the next touch.
const SESSION_SEEN_PRUNE_AT: usize = 4096;

fn session_seen() -> &'static std::sync::Mutex<std::collections::HashMap<String, i64>> {
    SESSION_SEEN.get_or_init(Default::default)
}

/// Record that a session was just used. Infallible and lock-poison-tolerant: liveness must never be the
/// thing that fails, since a failure here logs a working operator out.
fn mark_session_seen(sid: &str, now: DateTime<Utc>, idle_minutes: i64) {
    let Ok(mut map) = session_seen().lock() else {
        return;
    };
    if map.len() > SESSION_SEEN_PRUNE_AT {
        let cutoff = (now - Duration::minutes(idle_minutes.max(1))).timestamp();
        map.retain(|_, seen| *seen >= cutoff);
    }
    map.insert(sid.to_string(), now.timestamp());
}

/// The most recent in-memory sighting of a session, if any.
fn session_last_seen(sid: &str) -> Option<i64> {
    session_seen().lock().ok()?.get(sid).copied()
}

/// Forget a session's liveness (on logout / expiry) so the map does not pin dead sessions.
fn forget_session_seen(sid: &str) {
    if let Ok(mut map) = session_seen().lock() {
        map.remove(sid);
    }
}

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
            // Best-effort last-used stamp (does not gate the request). Debounced to once a
            // minute per key: the AI worker authenticates every request with its key, and an
            // unconditional UPDATE put a write on SQLite's single writer for every poll —
            // observed live stalling 1–2s each under recorder/ingest write load.
            let _ = sqlx::query(
                "UPDATE api_keys SET last_used_at = ?
                 WHERE id = ? AND (last_used_at IS NULL OR last_used_at < ?)",
            )
            .bind(now)
            .bind(&id)
            .bind(now - Duration::minutes(1))
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
        "SELECT s.id AS sid, s.created_at, s.expires_at, s.last_used_at, u.id AS uid, u.display_name, u.role, u.active
           FROM sessions s JOIN users u ON u.id = s.user_id
          WHERE s.id = ?",
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await?;
    if let Some(r) = row {
        // Absolute TTL, then idle timeout — either drops the session.
        //
        // Idle is judged on the NEWER of the durable stamp and the in-memory sighting (see SESSION_SEEN):
        // the stamp below is best-effort and can be silently dropped under write pressure, so trusting it
        // alone logs out operators who never went idle.
        let last_active = match session_last_seen(&r.sid) {
            Some(seen) if seen > r.last_used_at.timestamp() => {
                DateTime::from_timestamp(seen, 0).unwrap_or(r.last_used_at)
            }
            _ => r.last_used_at,
        };
        let idle_expired = idle_minutes > 0 && last_active < now - Duration::minutes(idle_minutes);
        if r.expires_at <= now || idle_expired {
            // Say WHY before the row disappears. Expiry deletes the session, so without this an
            // operator asking "why was I logged out?" has nothing to look at, and a genuine timeout is
            // indistinguishable from a bad token or a logout. Both reasons are reported, and the idle
            // case carries the numbers needed to judge whether the timeout is set too aggressively.
            if r.expires_at <= now {
                tracing::info!(
                    user = %r.uid,
                    session_age_min = (now - r.created_at).num_minutes(),
                    "auth: session reached its absolute TTL; re-login required"
                );
            } else {
                tracing::info!(
                    user = %r.uid,
                    idle_min = (now - last_active).num_minutes(),
                    idle_timeout_min = idle_minutes,
                    "auth: session idle-expired; re-login required"
                );
            }
            forget_session_seen(&r.sid);
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
        // Liveness first, and in memory, so it cannot fail: this is what the idle check reads, and the
        // operator is demonstrably active right now.
        mark_session_seen(&r.sid, now, idle_minutes);
        // Then the durable stamp, debounced like the api-key one (idle timeouts are measured in minutes,
        // so 1-minute granularity is lossless for the idle check and spares the SQLite writer).
        //
        // NOTE the third bind. It was missing, so the debounce placeholder was left UNBOUND — SQLite
        // reads an unbound parameter as NULL, `last_used_at < NULL` is never true, and the WHERE never
        // matched. The stamp therefore never updated after login (rows_affected=0, always), so every
        // session idled out a fixed interval after LOGIN no matter how actively it was used.
        if let Err(e) = sqlx::query(
            "UPDATE sessions SET last_used_at = ?
             WHERE id = ? AND (last_used_at IS NULL OR last_used_at < ?)",
        )
        .bind(now)
        .bind(&r.sid)
        .bind(now - Duration::minutes(1))
        .execute(pool)
        .await
        {
            // Best-effort, but no longer silent: a persistently failing stamp means sessions stop
            // surviving a restart, and that is worth seeing.
            tracing::warn!(
                error = %e,
                "auth: could not persist session last_used_at; liveness held in memory only"
            );
        }
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
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    last_used_at: DateTime<Utc>,
    uid: String,
    display_name: Option<String>,
    role: String,
    active: bool,
}

/// Resolve the caller from request headers — the single credential-resolution path shared by the
/// [`Principal`] extractor and the router-level [`require_api_auth`] floor.
async fn resolve_request_principal(
    st: &AppState,
    headers: &axum::http::HeaderMap,
) -> Result<Principal, AppError> {
    match token_from_headers(headers) {
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

impl FromRequestParts<AppState> for Principal {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Self::Rejection> {
        // The /api/v1 auth floor (require_api_auth) already resolved the caller — reuse it rather
        // than hitting the sessions/api_keys tables a second time per request.
        if let Some(p) = parts.extensions.get::<Principal>() {
            return Ok(p.clone());
        }
        resolve_request_principal(st, &parts.headers).await
    }
}

/// `/api/v1` paths that are deliberately reachable without a credential. Keep this list SHORT and
/// commented — every entry is a hole in the floor.
const API_AUTH_ALLOWLIST: &[&str] = &[
    // Pre-auth by definition: exchanges username+password for the session token.
    "/api/v1/auth/login",
    // Logout must stay idempotent: it reads the bearer token itself and clears the cookie, so an
    // already-expired session still gets a clean `Set-Cookie: Max-Age=0` instead of a 401 here.
    "/api/v1/auth/logout",
];

/// Router-level authentication floor for the whole `/api/v1` surface (June 2026 audit
/// recommendation, issue #52). In this kernel a handler is authenticated only if it NAMES
/// [`Principal`] in its signature — a handler without it silently answered unauthenticated (the
/// exact class behind the six-handler audit finding fixed in c6d68bd). This middleware makes that
/// class impossible: every `/api/v1/*` request must resolve a caller (or auth must be disabled)
/// before ANY handler runs, allowlist excepted. The resolved [`Principal`] is stashed in request
/// extensions so per-handler extractors don't pay a second lookup; per-handler
/// [`Principal::require`] remains the RBAC layer on top.
///
/// Non-`/api/v1` paths (`/healthz`, `/readyz`, `/metrics`, `/media/*` — separately guarded —
/// `/internal/*`, the SPA fallback) pass through untouched.
pub async fn require_api_auth(
    axum::extract::State(st): axum::extract::State<AppState>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let path = req.uri().path();
    if !path.starts_with("/api/v1") || API_AUTH_ALLOWLIST.contains(&path) {
        return next.run(req).await;
    }
    match resolve_request_principal(&st, req.headers()).await {
        Ok(principal) => {
            req.extensions_mut().insert(principal);
            next.run(req).await
        }
        Err(e) => e.into_response(),
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

    async fn mem_pool_migrated() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        pool
    }

    /// Seed a user + a session whose `last_used_at` is `age_minutes` old, returning (raw token, sid).
    async fn seed_session(pool: &SqlitePool, age_minutes: i64) -> (String, String) {
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, display_name, role, active, created_at, updated_at)
             VALUES ('u1','op',?,'Op','admin',1,?,?)",
        )
        .bind(hash_password("pw").unwrap())
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
        let token = random_token(SESSION_PREFIX);
        let sid = token_hash(&token);
        sqlx::query(
            "INSERT INTO sessions (id, user_id, created_at, expires_at, last_used_at)
             VALUES (?, 'u1', ?, ?, ?)",
        )
        .bind(&sid)
        .bind(now - Duration::minutes(age_minutes))
        .bind(now + Duration::hours(8))
        .bind(now - Duration::minutes(age_minutes))
        .execute(pool)
        .await
        .unwrap();
        (token, sid)
    }

    /// Regression: the debounce placeholder in the `last_used_at` UPDATE was left unbound, so SQLite
    /// read it as NULL, `last_used_at < NULL` never held, and the stamp NEVER advanced after login —
    /// every session idled out a fixed interval after login regardless of activity.
    #[tokio::test]
    async fn using_a_session_advances_last_used_at() {
        let pool = mem_pool_migrated().await;
        let (token, sid) = seed_session(&pool, 10).await;

        let before: DateTime<Utc> =
            sqlx::query_scalar("SELECT last_used_at FROM sessions WHERE id = ?")
                .bind(&sid)
                .fetch_one(&pool)
                .await
                .unwrap();

        let p = resolve_token(&pool, &token, 45).await.unwrap();
        assert!(p.is_some(), "a 10-minute-old session must still resolve");

        let after: DateTime<Utc> =
            sqlx::query_scalar("SELECT last_used_at FROM sessions WHERE id = ?")
                .bind(&sid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            after > before,
            "using a session must advance last_used_at (was {before}, now {after}) — otherwise the \
             idle timeout measures time since LOGIN, not since last activity"
        );
    }

    /// An operator who keeps working must not be idled out just because the durable stamp is stale:
    /// the in-memory sighting is the newer signal and wins.
    #[tokio::test]
    async fn in_memory_liveness_keeps_an_active_session_alive() {
        let pool = mem_pool_migrated().await;
        // Stamp is older than the idle window, but the session was just seen in-process.
        let (token, sid) = seed_session(&pool, 90).await;
        mark_session_seen(&sid, Utc::now(), 45);

        assert!(
            resolve_token(&pool, &token, 45).await.unwrap().is_some(),
            "a session seen in-memory within the idle window must survive a stale durable stamp"
        );
    }

    /// ...but a genuinely idle session still expires, and is cleaned up.
    #[tokio::test]
    async fn genuinely_idle_session_still_expires() {
        let pool = mem_pool_migrated().await;
        let (token, sid) = seed_session(&pool, 90).await;
        forget_session_seen(&sid);

        assert!(
            resolve_token(&pool, &token, 45).await.unwrap().is_none(),
            "a session idle past the window must be rejected"
        );
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE id = ?")
            .bind(&sid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 0, "an idle-expired session is deleted");
    }

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

    // ---- require_api_auth middleware (issue #52) --------------------------------------------

    async fn auth_mw_state(auth_enabled: bool) -> AppState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let mut cfg = Config::from_env();
        cfg.auth_enabled = auth_enabled;
        let cfg = std::sync::Arc::new(cfg);
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
            started_at: Utc::now(),
            pool,
            cfg,
        }
    }

    /// The regression this middleware exists to prevent: a handler that FORGETS to name
    /// `Principal` must still be unreachable unauthenticated once the floor is applied.
    fn app_with_naked_handler(st: AppState) -> axum::Router {
        use axum::routing::get;
        axum::Router::new()
            .route("/api/v1/naked", get(|| async { "secret" }))
            .route("/api/v1/auth/login", get(|| async { "login-page" }))
            .route("/healthz", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                st.clone(),
                require_api_auth,
            ))
            .with_state(st)
    }

    async fn status_of(
        app: &axum::Router,
        path: &str,
        bearer: Option<&str>,
    ) -> axum::http::StatusCode {
        use tower::ServiceExt;
        let mut req = axum::http::Request::builder().uri(path);
        if let Some(t) = bearer {
            req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        app.clone()
            .oneshot(req.body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn floor_blocks_unannotated_api_handler_when_auth_enabled() {
        let st = auth_mw_state(true).await;
        let app = app_with_naked_handler(st);
        // The naked /api/v1 handler is unreachable without a credential — the whole point.
        assert_eq!(
            status_of(&app, "/api/v1/naked", None).await,
            axum::http::StatusCode::UNAUTHORIZED
        );
        // Allowlisted login + logout (idempotent) + non-/api/v1 health pass through.
        assert_eq!(
            status_of(&app, "/api/v1/auth/login", None).await,
            axum::http::StatusCode::OK
        );
        assert!(API_AUTH_ALLOWLIST.contains(&"/api/v1/auth/logout"));
        assert_eq!(
            status_of(&app, "/healthz", None).await,
            axum::http::StatusCode::OK
        );
        // A bogus bearer is still rejected.
        assert_eq!(
            status_of(&app, "/api/v1/naked", Some("vos_deadbeef")).await,
            axum::http::StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn floor_admits_valid_session_and_is_open_when_auth_disabled() {
        // Valid session → through.
        let st = auth_mw_state(true).await;
        sqlx::query("INSERT INTO users (id, username, password_hash, role, active, created_at, updated_at) VALUES ('u1','u1',?, 'admin', 1, ?, ?)")
            .bind(hash_password("pw").unwrap())
            .bind(Utc::now())
            .bind(Utc::now())
            .execute(&st.pool)
            .await
            .unwrap();
        let (tok, _) = issue_session(&st.pool, &st.cfg, "u1").await.unwrap();
        let app = app_with_naked_handler(st);
        assert_eq!(
            status_of(&app, "/api/v1/naked", Some(&tok)).await,
            axum::http::StatusCode::OK
        );

        // Auth disabled (LAN appliance default) → the floor is a no-op.
        let open = auth_mw_state(false).await;
        let app_open = app_with_naked_handler(open);
        assert_eq!(
            status_of(&app_open, "/api/v1/naked", None).await,
            axum::http::StatusCode::OK
        );
    }
}
