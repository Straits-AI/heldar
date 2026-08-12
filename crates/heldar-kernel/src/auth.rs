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

use std::collections::HashSet;
use std::fmt::Write as _;
use std::sync::Arc;

use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier};
use argon2::{password_hash::SaltString, Argon2};
use axum::extract::FromRequestParts;
use axum::http::header;
use axum::http::request::Parts;
use chrono::{DateTime, Duration, Utc};
use rand_core::RngCore;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::config::{Config, EnforcementTier};
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

/// A single grantable capability.
///
/// Capabilities replace the old `Principal::can_view()`, which returned `true` UNCONDITIONALLY — so an
/// integration key minted for an AI worker could read the vehicle registry, the watchlist, live video,
/// clip export, the network scanner and every sidecar's reverse proxy. The split is drawn where the
/// blast radius is, not where the router is: `video:live` (mints a MediaMTX token AND starts an ffmpeg
/// publisher), `video:export` (transcodes and writes a file) and `ai:frames` (the live JPEG of faces and
/// plates) are three different privileges even though the dashboard treats them as one "media" screen.
///
/// `Cap::Admin` is a super-capability: [`Principal::has`] answers true for everything when it is held.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum Cap {
    /// Full administrative control; implies every other capability.
    Admin,
    /// Manage the identity registry: vehicles + watchlist (the old `can_manage_registry`).
    RegistryManage,
    /// Operate the gate: visitor check-in/out, create passes, confirm/reject entries.
    GateOperate,
    /// Read the camera inventory and per-camera device configuration.
    CameraRead,
    /// Open a live stream (mints a media read token and can START a transcoding publisher).
    VideoLive,
    /// Read recorded video: segments, timelines, playback sessions, snapshots.
    VideoPlayback,
    /// Export recorded video off the box: clip transcode + archive export.
    VideoExport,
    /// Read the event/detection surface: events, zones, incidents, search, movement, entry events.
    EventsRead,
    /// Read identifying records: vehicles, watchlist entries, visitor passes, gate configuration.
    IdentityRead,
    /// Read the AI task list (what to analyze, at what fps, on which stream).
    AiTasks,
    /// Read sampled camera frames (`/cameras/{id}/frame`) — the actual pixels.
    AiFrames,
    /// Post perception results into the ingest pipeline.
    AiIngest,
    /// Claim and answer embedding-query work (the operator's pending semantic-search text).
    AiEmbedWork,
    /// Read system/operational state: system info, metrics, backups, modules, plugin store.
    SystemRead,
    /// Actively scan the network for cameras (an outbound probe sweep, not a read).
    NetScan,
    /// Reach a registered sidecar through the kernel's reverse proxy at `/m/{id}/*`.
    ModuleProxy,
}

impl Cap {
    /// Every capability, in declaration order. The single source of truth for `CapSet::ALL`, the
    /// auth-off sweep test, and the docs table.
    pub const ALL: [Cap; 16] = [
        Cap::Admin,
        Cap::RegistryManage,
        Cap::GateOperate,
        Cap::CameraRead,
        Cap::VideoLive,
        Cap::VideoPlayback,
        Cap::VideoExport,
        Cap::EventsRead,
        Cap::IdentityRead,
        Cap::AiTasks,
        Cap::AiFrames,
        Cap::AiIngest,
        Cap::AiEmbedWork,
        Cap::SystemRead,
        Cap::NetScan,
        Cap::ModuleProxy,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Cap::Admin => "admin",
            Cap::RegistryManage => "registry:manage",
            Cap::GateOperate => "gate:operate",
            Cap::CameraRead => "camera:read",
            Cap::VideoLive => "video:live",
            Cap::VideoPlayback => "video:playback",
            Cap::VideoExport => "video:export",
            Cap::EventsRead => "events:read",
            Cap::IdentityRead => "identity:read",
            Cap::AiTasks => "ai:tasks",
            Cap::AiFrames => "ai:frames",
            Cap::AiIngest => "ai:ingest",
            Cap::AiEmbedWork => "ai:embedwork",
            Cap::SystemRead => "system:read",
            Cap::NetScan => "net:scan",
            Cap::ModuleProxy => "module:proxy",
        }
    }

    pub fn parse(s: &str) -> Option<Cap> {
        Cap::ALL.into_iter().find(|c| c.slug() == s)
    }

    const fn bit(self) -> u64 {
        1u64 << (self as u64)
    }
}

/// A set of capabilities, packed into a bitmask so a `Principal` stays cheap to clone per request.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CapSet(u64);

impl CapSet {
    pub const NONE: CapSet = CapSet(0);

    /// Every capability. `Principal::system_admin()` holds this, which is how constraint 1 (auth
    /// disabled keeps working) holds BY CONSTRUCTION rather than by a special case in each handler.
    pub const ALL: CapSet = {
        let mut bits = 0u64;
        let mut i = 0;
        while i < Cap::ALL.len() {
            bits |= Cap::ALL[i].bit();
            i += 1;
        }
        CapSet(bits)
    };

    pub fn of(caps: &[Cap]) -> CapSet {
        let mut bits = 0u64;
        for c in caps {
            bits |= c.bit();
        }
        CapSet(bits)
    }

    pub fn contains(self, c: Cap) -> bool {
        self.0 & c.bit() != 0
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    pub fn with(self, c: Cap) -> CapSet {
        CapSet(self.0 | c.bit())
    }
    pub fn union(self, other: CapSet) -> CapSet {
        CapSet(self.0 | other.0)
    }
    /// Capabilities in `self` that are NOT in `other` — the deny-preview under `warn`.
    pub fn minus(self, other: CapSet) -> CapSet {
        CapSet(self.0 & !other.0)
    }
    pub fn iter(self) -> impl Iterator<Item = Cap> {
        Cap::ALL.into_iter().filter(move |c| self.contains(*c))
    }
    pub fn slugs(self) -> Vec<&'static str> {
        self.iter().map(|c| c.slug()).collect()
    }
}

/// Every capability that the unconditional `can_view()` used to hand out. Kept as one constant because
/// the legacy expansion's correctness argument is "this is exactly what `can_view()` unlocked", and that
/// argument is only checkable if the set is written down once.
const LEGACY_VIEW_CAPS: CapSet = CapSet::of_const(&[
    Cap::CameraRead,
    Cap::VideoLive,
    Cap::VideoPlayback,
    Cap::VideoExport,
    Cap::EventsRead,
    Cap::IdentityRead,
    Cap::AiTasks,
    Cap::AiFrames,
    Cap::SystemRead,
    Cap::NetScan,
    Cap::ModuleProxy,
]);

impl CapSet {
    /// `of` in a const context (used for the role tables).
    pub const fn of_const(caps: &[Cap]) -> CapSet {
        let mut bits = 0u64;
        let mut i = 0;
        while i < caps.len() {
            bits |= caps[i].bit();
            i += 1;
        }
        CapSet(bits)
    }
}

/// Back-compat expansion for a credential with NO explicit capability grant (`capabilities IS NULL`).
///
/// This reproduces TODAY'S REACH EXACTLY, so no deployed key changes behaviour when 0012 lands. The
/// argument, per role:
///   * `can_view()` was unconditionally true  -> every role gets [`LEGACY_VIEW_CAPS`];
///   * `can_operate_gate()` was admin|manager|guard;
///   * `can_manage_registry()` was admin|manager;
///   * `can_ingest()` was admin|integration — which is why manager does NOT get `ai:ingest` here even
///     though it otherwise looks like "everything but admin".
pub fn legacy_caps(role: Role) -> CapSet {
    match role {
        Role::Admin => CapSet::ALL,
        Role::Manager => LEGACY_VIEW_CAPS
            .with(Cap::GateOperate)
            .with(Cap::RegistryManage),
        Role::Guard => LEGACY_VIEW_CAPS.with(Cap::GateOperate),
        Role::Viewer => LEGACY_VIEW_CAPS,
        Role::Integration => LEGACY_VIEW_CAPS.with(Cap::AiIngest).with(Cap::AiEmbedWork),
    }
}

/// Contained expansion for a legacy credential under `HELDAR_MACHINE_AUTH=enforce`.
///
/// Identical to [`legacy_caps`] for the four HUMAN roles — an operator's reach is not what hole (a) is
/// about. `integration` (the machine role, and the only role a sidecar or worker key is ever minted
/// with) narrows to what a real AI worker actually calls, losing identity:read, system:read, net:scan,
/// module:proxy and all three video capabilities. No key is bricked and none needs re-minting.
pub fn enforced_caps(role: Role) -> CapSet {
    match role {
        Role::Integration => CapSet::of(&[
            Cap::AiTasks,
            Cap::AiFrames,
            Cap::AiIngest,
            Cap::AiEmbedWork,
            Cap::CameraRead,
            Cap::EventsRead,
        ]),
        other => legacy_caps(other),
    }
}

/// Tier selector: which expansion a capability-less credential gets.
pub fn role_caps(role: Role, tier: EnforcementTier) -> CapSet {
    match tier {
        EnforcementTier::Off | EnforcementTier::Warn => legacy_caps(role),
        EnforcementTier::Enforce => enforced_caps(role),
    }
}

/// Parse a stored `capabilities` JSON array into a [`CapSet`] plus the slugs that were not recognized.
///
/// An unknown slug is DROPPED with a warning rather than denying the whole key. Dropping grants
/// nothing, so it is still fail-closed, and it keeps a key minted by a newer kernel usable after a
/// rollback. This deliberately differs from [`Role::parse`], where denying is correct because the
/// fallback there would be a capability-BEARING default.
pub fn parse_capability_slugs(slugs: &[String]) -> (CapSet, Vec<String>) {
    let mut set = CapSet::NONE;
    let mut unknown = Vec::new();
    for s in slugs {
        let trimmed = s.trim();
        match Cap::parse(trimmed) {
            Some(c) => set = set.with(c),
            None if trimmed.is_empty() => {}
            None => unknown.push(trimmed.to_string()),
        }
    }
    (set, unknown)
}

/// Which cameras a credential may address.
///
/// `Cameras` exists for a key handed to one integrator covering one lane. There is ZERO tenancy in the
/// schema (no `site_id` predicate anywhere), so it is refused at mint time in combination with
/// `events:read` / `identity:read`, which read cross-camera tables it cannot filter.
#[derive(Clone, Debug)]
pub enum Scope {
    All,
    Cameras(Arc<HashSet<String>>),
}

impl Scope {
    pub fn kind(&self) -> &'static str {
        match self {
            Scope::All => "all",
            Scope::Cameras(_) => "cameras",
        }
    }
}

/// How a session's expiry behaves, resolved from config.
///
/// `max_lifetime_hours == 0` keeps expiry ABSOLUTE — `expires_at` is fixed at login, so an operator
/// working continuously is still logged out after `ttl_hours`. Above 0, an in-use session SLIDES: each
/// use pushes `expires_at` to `now + ttl_hours`, never past `created_at + max_lifetime_hours`.
///
/// Sliding one opaque, DB-backed session is what a stateless design would need a refresh token for.
/// Here the session is already revocable by deleting a row, so a second long-lived credential would add
/// attack surface and buy nothing. The cap is mandatory: uncapped sliding makes a stolen cookie
/// immortal, which is exactly what the absolute TTL exists to prevent.
#[derive(Clone, Copy, Debug)]
pub struct SessionPolicy {
    pub idle_minutes: i64,
    pub ttl_hours: i64,
    pub max_lifetime_hours: i64,
}

impl SessionPolicy {
    /// The new `expires_at` for a session used at `now`, or None when expiry is absolute or the slide
    /// would not move it forward (so the common case writes nothing).
    fn slide_to(
        &self,
        now: DateTime<Utc>,
        created_at: DateTime<Utc>,
        current: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        if self.max_lifetime_hours <= 0 {
            return None;
        }
        let ceiling = created_at + Duration::hours(self.max_lifetime_hours);
        let target = (now + Duration::hours(self.ttl_hours)).min(ceiling);
        (target > current).then_some(target)
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
    /// What this caller may DO. For a user or a legacy API key this is the role expansion; for a key
    /// minted with an explicit grant it is exactly that grant.
    pub caps: CapSet,
    /// Which cameras this caller may address.
    pub scope: Scope,
}

impl Principal {
    /// The implicit principal used when auth is disabled.
    ///
    /// Holds [`CapSet::ALL`] and [`Scope::All`], so every `require_cap` / `require_camera` added
    /// anywhere in the tree passes for it — the LAN-appliance default cannot be broken by adding a
    /// capability, only by removing one from this constructor. (`auth_off_sweep_grants_every_cap`
    /// pins that.)
    pub fn system_admin() -> Self {
        Principal {
            id: "system".into(),
            name: "system".into(),
            role: Role::Admin,
            kind: PrincipalKind::System,
            caps: CapSet::ALL,
            scope: Scope::All,
        }
    }

    /// A principal whose capabilities come from its role (users, and API keys with no explicit grant).
    pub fn from_role(
        id: String,
        name: String,
        role: Role,
        kind: PrincipalKind,
        caps: CapSet,
    ) -> Self {
        Principal {
            id,
            name,
            role,
            kind,
            caps,
            scope: Scope::All,
        }
    }

    /// Whether this caller holds `c`. `Cap::Admin` implies everything.
    pub fn has(&self, c: Cap) -> bool {
        self.caps.contains(Cap::Admin) || self.caps.contains(c)
    }

    pub fn can_admin(&self) -> bool {
        self.caps.contains(Cap::Admin)
    }
    /// Manage the registry: vehicles + watchlist.
    pub fn can_manage_registry(&self) -> bool {
        self.has(Cap::RegistryManage)
    }
    /// Operate the gate: visitor check-in/out, create passes, confirm/reject entries.
    pub fn can_operate_gate(&self) -> bool {
        self.has(Cap::GateOperate)
    }
    /// Post perception/ANPR events into the entry pipeline (machine clients + admins).
    pub fn can_ingest(&self) -> bool {
        self.has(Cap::AiIngest)
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

    /// Assert a named capability. The message names the missing slug so an operator can fix the grant
    /// without reading the source.
    pub fn require_cap(&self, c: Cap, action: &str) -> AppResult<()> {
        if self.has(c) {
            Ok(())
        } else {
            Err(AppError::Forbidden(format!(
                "role `{}` is not permitted to {action} (missing capability `{}`)",
                self.role.as_str(),
                c.slug()
            )))
        }
    }

    /// The camera allowlist, or None when unrestricted — for building an `IN (...)` list predicate.
    pub fn camera_scope(&self) -> Option<&HashSet<String>> {
        match &self.scope {
            Scope::All => None,
            Scope::Cameras(set) => Some(set),
        }
    }

    pub fn camera_allowed(&self, camera_id: &str) -> bool {
        match &self.scope {
            Scope::All => true,
            Scope::Cameras(set) => set.contains(camera_id),
        }
    }

    /// Assert camera scope. Deliberately checked BEFORE the camera row is loaded, so an out-of-scope id
    /// answers 403 whether or not it exists — the boundary must not be an existence oracle.
    pub fn require_camera(&self, camera_id: &str, action: &str) -> AppResult<()> {
        if self.camera_allowed(camera_id) {
            Ok(())
        } else {
            Err(AppError::Forbidden(format!(
                "credential is not scoped to camera `{camera_id}` (cannot {action})"
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
    policy: SessionPolicy,
    tier: EnforcementTier,
) -> AppResult<Option<Principal>> {
    let idle_minutes = policy.idle_minutes;
    let hash = token_hash(token);
    let now = Utc::now();
    if token.starts_with(APIKEY_PREFIX) {
        // The five capability columns ride the EXISTING single seek on `key_hash UNIQUE` — no second
        // round trip. That is deliberate: the AI worker authenticates on every request, and an extra
        // query here is paid at worker poll rate.
        let row: Option<ApiKeyRow> = sqlx::query_as(
            "SELECT id, name, role, active, capabilities, scope_kind, scope_cameras, expires_at, revoked_at
               FROM api_keys WHERE key_hash = ?",
        )
        .bind(&hash)
        .fetch_optional(pool)
        .await?;
        if let Some(r) = row {
            if !r.active {
                return Ok(None);
            }
            if let Some(revoked) = r.revoked_at {
                tracing::debug!(api_key = %r.id, %revoked, "auth: api key is revoked; denying");
                return Ok(None);
            }
            if let Some(exp) = r.expires_at {
                if exp <= now {
                    tracing::debug!(api_key = %r.id, %exp, "auth: api key has expired; denying");
                    return Ok(None);
                }
            }
            // An unparseable stored role means a corrupt/tampered row — deny rather than fail open
            // to a capability-bearing default.
            let Some(role) = Role::parse(&r.role) else {
                tracing::error!(api_key = %r.id, role = %r.role, "auth: api key has unparseable role; denying");
                return Ok(None);
            };
            let caps = match resolve_key_caps(&r.id, r.capabilities.as_deref(), role, tier) {
                Some(c) => c,
                // A malformed capabilities blob is a corrupt/tampered row, and unlike an unknown slug
                // there is no safe subset to fall back to — deny, same as an unparseable role.
                None => return Ok(None),
            };
            let scope = resolve_key_scope(&r.id, &r.scope_kind, r.scope_cameras.as_deref());
            // Best-effort last-used stamp (does not gate the request). Debounced to once a
            // minute per key: the AI worker authenticates every request with its key, and an
            // unconditional UPDATE put a write on SQLite's single writer for every poll —
            // observed live stalling 1–2s each under recorder/ingest write load.
            let _ = sqlx::query(
                "UPDATE api_keys SET last_used_at = ?
                 WHERE id = ? AND (last_used_at IS NULL OR last_used_at < ?)",
            )
            .bind(now)
            .bind(&r.id)
            .bind(now - Duration::minutes(1))
            .execute(pool)
            .await;
            return Ok(Some(Principal {
                id: r.id,
                name: r.name,
                role,
                kind: PrincipalKind::ApiKey,
                caps,
                scope,
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
        //
        // Sliding expiry rides the SAME debounced statement (see SessionPolicy::slide_to): when it is
        // enabled, an in-use session's `expires_at` is pushed forward, capped at
        // created_at + max_lifetime_hours. Folding it in here means no extra write per request, and the
        // 1-minute debounce bounds how often expiry moves at all.
        let slid = policy.slide_to(now, r.created_at, r.expires_at);
        let sql = if slid.is_some() {
            "UPDATE sessions SET last_used_at = ?, expires_at = ?
             WHERE id = ? AND (last_used_at IS NULL OR last_used_at < ?)"
        } else {
            "UPDATE sessions SET last_used_at = ?
             WHERE id = ? AND (last_used_at IS NULL OR last_used_at < ?)"
        };
        let mut q = sqlx::query(sql).bind(now);
        if let Some(new_exp) = slid {
            q = q.bind(new_exp);
        }
        if let Err(e) = q
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
        return Ok(Some(Principal::from_role(
            r.uid,
            r.display_name.unwrap_or_default(),
            role,
            PrincipalKind::User,
            role_caps(role, tier),
        )));
    }
    Ok(None)
}

/// The api-key row as the auth path reads it — one seek, every column the principal needs.
#[derive(sqlx::FromRow)]
struct ApiKeyRow {
    id: String,
    name: String,
    role: String,
    active: bool,
    capabilities: Option<String>,
    scope_kind: String,
    scope_cameras: Option<String>,
    expires_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
}

/// Resolve an api key's capability set. `None` means "deny this key" (corrupt row).
///
/// An explicit grant is used verbatim. A NULL grant is the LEGACY case and expands from the role under
/// the current tier — which is what contains already-deployed keys without anyone re-minting them.
fn resolve_key_caps(
    key_id: &str,
    capabilities: Option<&str>,
    role: Role,
    tier: EnforcementTier,
) -> Option<CapSet> {
    let Some(raw) = capabilities else {
        // Legacy key. Under `warn`, tell the operator exactly what `enforce` would take away — a tier
        // flip is otherwise a blind switch.
        if tier == EnforcementTier::Warn {
            let would_lose = legacy_caps(role).minus(enforced_caps(role));
            if !would_lose.is_empty() && should_log_deny_preview(key_id) {
                tracing::warn!(
                    target: "heldar::security",
                    api_key = %key_id,
                    role = %role.as_str(),
                    would_be_denied = %would_lose.slugs().join(","),
                    "HELDAR_MACHINE_AUTH=warn: this key has no explicit capability grant; under \
                     `enforce` it would LOSE these capabilities. Re-mint it with an explicit \
                     `capabilities` list before switching."
                );
            }
        }
        return Some(role_caps(role, tier));
    };
    let slugs: Vec<String> = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(
                api_key = %key_id, error = %e,
                "auth: api key has an unparseable `capabilities` blob; denying"
            );
            return None;
        }
    };
    let (set, unknown) = parse_capability_slugs(&slugs);
    if !unknown.is_empty() {
        tracing::warn!(
            api_key = %key_id, unknown = %unknown.join(","),
            "auth: dropping unrecognized capability slug(s) on api key (granting nothing for them)"
        );
    }
    Some(set)
}

/// Resolve an api key's camera scope. Anything we cannot read as an explicit, well-formed allowlist
/// fails CLOSED (an empty allowlist), never open.
fn resolve_key_scope(key_id: &str, scope_kind: &str, scope_cameras: Option<&str>) -> Scope {
    match scope_kind {
        "all" => Scope::All,
        "cameras" => {
            let ids: Vec<String> = scope_cameras
                .and_then(|raw| match serde_json::from_str(raw) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        tracing::error!(
                            api_key = %key_id, error = %e,
                            "auth: api key has an unparseable `scope_cameras` blob; scoping to NO cameras"
                        );
                        None
                    }
                })
                .unwrap_or_default();
            Scope::Cameras(Arc::new(ids.into_iter().collect()))
        }
        other => {
            tracing::error!(
                api_key = %key_id, scope_kind = %other,
                "auth: api key has an unrecognized `scope_kind`; scoping to NO cameras"
            );
            Scope::Cameras(Arc::new(HashSet::new()))
        }
    }
}

/// Rate-limit the `warn`-tier deny preview to once per key per hour: it fires on the auth path, which
/// the AI worker walks on every request.
fn should_log_deny_preview(key_id: &str) -> bool {
    static SEEN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, i64>>> =
        std::sync::OnceLock::new();
    let now = Utc::now().timestamp();
    let Ok(mut map) = SEEN.get_or_init(Default::default).lock() else {
        return false;
    };
    match map.get(key_id) {
        Some(last) if now - *last < 3600 => false,
        _ => {
            if map.len() > 1024 {
                map.retain(|_, last| now - *last < 3600);
            }
            map.insert(key_id.to_string(), now);
            true
        }
    }
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
            match resolve_token(
                &st.pool,
                &tok,
                SessionPolicy {
                    idle_minutes: st.cfg.session_idle_timeout_minutes,
                    ttl_hours: st.cfg.session_ttl_hours,
                    max_lifetime_hours: st.cfg.session_max_lifetime_hours,
                },
                st.cfg.machine_auth,
            )
            .await?
            {
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

    fn test_policy(idle_minutes: i64) -> SessionPolicy {
        // Absolute expiry (max_lifetime 0) unless a test opts into sliding — matches the shipped default.
        SessionPolicy {
            idle_minutes,
            ttl_hours: 8,
            max_lifetime_hours: 0,
        }
    }

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

    /// Sliding expiry: enabled, an in-use session's expiry moves forward — but never past the hard
    /// ceiling. Without the cap, sliding would make a stolen cookie immortal, which is the exact thing
    /// the absolute TTL exists to prevent, so the ceiling is the load-bearing half of this feature.
    #[test]
    fn sliding_expiry_moves_forward_but_never_past_the_ceiling() {
        let now = Utc::now();
        let created = now - Duration::hours(20);

        // Absolute by default (max_lifetime 0): never slides, whatever the current expiry.
        let absolute = SessionPolicy {
            idle_minutes: 45,
            ttl_hours: 8,
            max_lifetime_hours: 0,
        };
        assert_eq!(
            absolute.slide_to(now, created, now + Duration::hours(1)),
            None
        );

        // Sliding on: a session expiring in an hour is pushed out to now + ttl.
        let sliding = SessionPolicy {
            idle_minutes: 45,
            ttl_hours: 8,
            max_lifetime_hours: 168,
        };
        let got = sliding
            .slide_to(now, created, now + Duration::hours(1))
            .unwrap();
        assert_eq!(got, now + Duration::hours(8));

        // The ceiling wins: created 167h ago with a 168h cap leaves only an hour, not the full ttl.
        let old = now - Duration::hours(167);
        let capped = sliding
            .slide_to(now, old, now + Duration::minutes(5))
            .unwrap();
        assert_eq!(capped, old + Duration::hours(168));
        assert!(
            capped < now + Duration::hours(8),
            "the cap must bound the slide"
        );

        // Past the ceiling entirely: nothing to extend, so the session is allowed to die.
        let ancient = now - Duration::hours(200);
        assert_eq!(
            sliding.slide_to(now, ancient, now + Duration::hours(1)),
            None
        );

        // No pointless write when the slide would not move expiry forward.
        assert_eq!(
            sliding.slide_to(now, created, now + Duration::hours(20)),
            None
        );
    }

    /// End-to-end: with sliding on, using a session actually pushes `expires_at` in the database.
    #[tokio::test]
    async fn using_a_session_extends_expiry_when_sliding_is_enabled() {
        let pool = mem_pool_migrated().await;
        let (token, sid) = seed_session(&pool, 10).await;
        let before: DateTime<Utc> =
            sqlx::query_scalar("SELECT expires_at FROM sessions WHERE id = ?")
                .bind(&sid)
                .fetch_one(&pool)
                .await
                .unwrap();

        let policy = SessionPolicy {
            idle_minutes: 45,
            ttl_hours: 24,
            max_lifetime_hours: 168,
        };
        assert!(resolve_token(&pool, &token, policy, EnforcementTier::Warn)
            .await
            .unwrap()
            .is_some());

        let after: DateTime<Utc> =
            sqlx::query_scalar("SELECT expires_at FROM sessions WHERE id = ?")
                .bind(&sid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            after > before,
            "sliding expiry must extend expires_at on use (was {before}, now {after})"
        );
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

        let p = resolve_token(&pool, &token, test_policy(45), EnforcementTier::Warn)
            .await
            .unwrap();
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
            resolve_token(&pool, &token, test_policy(45), EnforcementTier::Warn)
                .await
                .unwrap()
                .is_some(),
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
            resolve_token(&pool, &token, test_policy(45), EnforcementTier::Warn)
                .await
                .unwrap()
                .is_none(),
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
        // Build each principal from its ROLE EXPANSION, not from `system_admin()`. Capabilities are
        // no longer derived from `role`, so `Principal { role, ..system_admin() }` carries CapSet::ALL
        // and every negative assertion below would silently pass on a principal that can do
        // everything — a test that reads as a matrix check while pinning nothing.
        let with = |role: Role, caps: CapSet| Principal {
            role,
            caps,
            ..Principal::system_admin()
        };
        let admin = with(Role::Admin, legacy_caps(Role::Admin));
        let guard = with(Role::Guard, legacy_caps(Role::Guard));
        let integ = with(Role::Integration, legacy_caps(Role::Integration));

        assert!(admin.can_admin() && admin.can_ingest() && admin.can_manage_registry());
        assert!(guard.can_operate_gate() && !guard.can_manage_registry() && !guard.can_admin());
        assert!(integ.can_ingest() && !integ.can_operate_gate());
        // The hole this whole change exists to close: an integration credential must not be an admin
        // and must not manage the registry, under EITHER expansion.
        for caps in [
            legacy_caps(Role::Integration),
            enforced_caps(Role::Integration),
        ] {
            let p = with(Role::Integration, caps);
            assert!(!p.can_admin(), "integration must never be admin");
            assert!(
                !p.can_manage_registry(),
                "integration must not manage the registry"
            );
            assert!(p.can_ingest(), "an AI worker must still be able to ingest");
        }
        // `enforce` must actually take reach away from a machine credential, or the tier is cosmetic.
        let legacy = legacy_caps(Role::Integration);
        let enforced = enforced_caps(Role::Integration);
        assert_ne!(
            legacy, enforced,
            "enforced_caps must narrow the integration role"
        );
        // ...and must not touch the human roles.
        for role in [Role::Admin, Role::Manager, Role::Guard, Role::Viewer] {
            assert_eq!(
                legacy_caps(role),
                enforced_caps(role),
                "enforce must not change {role:?}"
            );
        }
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
