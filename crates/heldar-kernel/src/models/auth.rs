use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Operator account. `password_hash` is never serialized; use [`UserView`] for output.
#[derive(Debug, Clone, FromRow)]
pub struct User {
    pub id: String,
    pub username: String,
    pub password_hash: String,
    pub role: String,
    pub display_name: Option<String>,
    pub active: bool,
    /// Consecutive failed logins (brute-force lockout). Never serialized (see [`UserView`]).
    pub failed_login_count: i64,
    /// Instant before which login is refused; `None` = not locked. Never serialized.
    pub locked_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct UserView {
    pub id: String,
    pub username: String,
    pub role: String,
    pub display_name: Option<String>,
    pub active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<User> for UserView {
    fn from(u: User) -> Self {
        UserView {
            id: u.id,
            username: u.username,
            role: u.role,
            display_name: u.display_name,
            active: u.active,
            created_at: u.created_at,
            updated_at: u.updated_at,
        }
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UserCreate {
    pub username: String,
    pub password: String,
    pub role: Option<String>,
    pub display_name: Option<String>,
    pub active: Option<bool>,
}

#[derive(Debug, Deserialize, Default, utoipa::ToSchema)]
pub struct UserUpdate {
    pub password: Option<String>,
    pub role: Option<String>,
    pub display_name: Option<String>,
    pub active: Option<bool>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct ApiKey {
    pub id: String,
    pub name: String,
    /// Mapped from the row for completeness; never exposed (see [`ApiKeyView`]).
    pub key_hash: String,
    pub key_prefix: String,
    pub role: String,
    pub active: bool,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// JSON array of capability slugs. `None` = legacy key, expanded from `role` (see
    /// `auth::legacy_caps`).
    pub capabilities: Option<String>,
    /// `all` | `cameras`.
    pub scope_kind: String,
    /// JSON array of camera ids, honoured only when `scope_kind = "cameras"`.
    pub scope_cameras: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    /// Soft revocation: the row survives so audit entries keep resolving.
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiKeyView {
    pub id: String,
    pub name: String,
    pub key_prefix: String,
    pub role: String,
    pub active: bool,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// The capability slugs this key actually holds. For a legacy key this is the role expansion under
    /// the CURRENT tier, so the dashboard shows effective reach rather than a misleading `null`.
    pub capabilities: Vec<String>,
    /// True when `capabilities` above was derived from the role rather than stored on the row.
    pub legacy_role_expansion: bool,
    /// Stored slugs this kernel does not recognize (dropped, granting nothing). Surfaced so a key
    /// minted by a newer kernel and then rolled back is diagnosable instead of mysteriously 403ing.
    pub unknown_capabilities: Vec<String>,
    pub scope_kind: String,
    pub scope_cameras: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize, Default, utoipa::ToSchema)]
pub struct ApiKeyCreate {
    pub name: String,
    pub role: Option<String>,
    /// Explicit capability grant. Omitted = fall back to role expansion (what the dashboard and
    /// `validate_rbac.sh` do today), reported back as `legacy_role_expansion: true`.
    pub capabilities: Option<Vec<String>>,
    /// `all` (default) | `cameras`.
    pub scope_kind: Option<String>,
    pub scope_cameras: Option<Vec<String>>,
    pub expires_at: Option<DateTime<Utc>>,
    /// Required to be `true` when the grant includes admin / registry:manage / gate:operate.
    #[serde(default)]
    pub confirm_privileged: bool,
}

/// Partial update of an api key. Every field is optional; `revoked_at` is the soft-revoke switch.
#[derive(Debug, Deserialize, Default, utoipa::ToSchema)]
pub struct ApiKeyUpdate {
    pub active: Option<bool>,
    pub capabilities: Option<Vec<String>>,
    pub scope_kind: Option<String>,
    pub scope_cameras: Option<Vec<String>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub confirm_privileged: bool,
}
