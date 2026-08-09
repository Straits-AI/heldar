//! Sidecar module registration + health (Phase B of the plugin platform).
//!
//! Registering a sidecar atomically mints three reversible things: a scoped API key the sidecar uses
//! to call kernel APIs, a webhook subscription that feeds it the events it subscribes to, and (via the
//! stored row) a reverse-proxy mount at `/m/{id}/*`. Unregistering deletes all three. A background
//! loop probes each sidecar's `/heldar/health` and records reachability.

use std::time::Duration;

use chrono::Utc;
use sqlx::types::Json as SqlxJson;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::auth;
use crate::error::{AppError, AppResult};
use crate::modules::{ModuleRegisterRequest, ModuleRegistration, NavEntry};

/// Path on the sidecar that receives signed event deliveries (the minted webhook points here).
pub const WEBHOOK_EVENTS_PATH: &str = "/heldar/events";
/// Path on the sidecar the health probe hits; a 2xx means healthy.
pub const HEALTH_PATH: &str = "/heldar/health";
const WEBHOOK_SECRET_PREFIX: &str = "whsec_";

/// The ceiling on what a sidecar's minted key may ever hold, whatever role it asks for.
///
/// This is the same least-privilege intent `validate_plugin_role` had (never `admin`/`manager`/`guard`),
/// expressed as a capability INTERSECTION so it keeps holding now that grants are explicit: a sidecar
/// registration cannot mint itself out of the ceiling by naming capabilities instead of a role. The
/// three privileged capabilities are absent by construction, as is `ai:embedwork` (the operator's
/// pending search text is not a sidecar's business).
const PLUGIN_ALLOWED_CAPS: auth::CapSet = auth::CapSet::of_const(&[
    auth::Cap::CameraRead,
    auth::Cap::VideoLive,
    auth::Cap::VideoPlayback,
    auth::Cap::EventsRead,
    auth::Cap::AiTasks,
    auth::Cap::AiFrames,
    auth::Cap::AiIngest,
    auth::Cap::SystemRead,
]);

/// Plugin keys are least-privilege: only `viewer` (read) or `integration` (read + ingest) are
/// grantable. `admin`/`manager`/`guard` are never minted for a sidecar.
fn validate_plugin_role(role: &str) -> AppResult<&'static str> {
    match role {
        "viewer" => Ok("viewer"),
        "integration" => Ok("integration"),
        _ => Err(AppError::BadRequest(
            "`role` must be viewer|integration".into(),
        )),
    }
}

/// The explicit capability grant stored on a sidecar's minted key: its role expansion INTERSECTED with
/// [`PLUGIN_ALLOWED_CAPS`].
///
/// Storing an explicit grant (rather than leaving `capabilities` NULL) is what makes a sidecar key
/// contained on day one, in every tier — it never rides the legacy role expansion at all. The base is
/// the LEGACY expansion, not the enforced one, so a newly registered sidecar is never narrower than the
/// ceiling allows regardless of which tier the box happens to be running.
fn plugin_capabilities(role: &str) -> Vec<String> {
    let expanded = auth::Role::parse(role)
        .map(auth::legacy_caps)
        .unwrap_or(auth::CapSet::NONE);
    PLUGIN_ALLOWED_CAPS
        .iter()
        .filter(|c| expanded.contains(*c))
        .map(|c| c.slug().to_string())
        .collect()
}

fn validate_id(id: &str) -> AppResult<()> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !ok {
        return Err(AppError::BadRequest(
            "`id` must be a slug of [A-Za-z0-9_-], 1..=64 chars".into(),
        ));
    }
    Ok(())
}

/// Validate + normalize the sidecar origin (http/https, no trailing slash, no path/query). A sidecar
/// legitimately runs on loopback or the LAN, so LAN targets are permitted; the shared egress guard
/// still rejects the cloud-metadata/link-local range so a registered sidecar's `base_url` — which
/// drives an unattended health probe, webhook deliveries, and the `can_view` reverse-proxy — can't be
/// pointed at the metadata endpoint.
fn normalize_base_url(url: &str) -> AppResult<String> {
    let u = url.trim().trim_end_matches('/');
    // Reject an obviously malformed origin (scheme with no host) with a targeted message before the
    // egress guard's generic parse error.
    let after_scheme = u.split_once("://").map(|(_, rest)| rest).unwrap_or("");
    if after_scheme.is_empty() || after_scheme.starts_with('/') {
        return Err(AppError::BadRequest(
            "`base_url` must include a host".into(),
        ));
    }
    crate::net_guard::validate_egress_url(u, &crate::net_guard::EgressPolicy::LAN).map_err(
        |e| AppError::BadRequest(format!("`base_url` is not a valid sidecar origin: {e}")),
    )?;
    Ok(u.to_string())
}

/// Register a sidecar: mint its scoped key + webhook subscription and persist the row, atomically.
/// Returns the stored row plus the once-only credentials (plaintext key + webhook secret).
pub async fn register(
    pool: &SqlitePool,
    req: ModuleRegisterRequest,
    reserved_ids: &[String],
) -> AppResult<(ModuleRegistration, String, String)> {
    validate_id(&req.id)?;
    if reserved_ids.iter().any(|r| r == &req.id) {
        return Err(AppError::Conflict(format!(
            "module id `{}` is reserved by a built-in module",
            req.id
        )));
    }
    if get_registered(pool, &req.id).await?.is_some() {
        return Err(AppError::Conflict(format!(
            "module `{}` is already registered",
            req.id
        )));
    }
    let name = req.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("`name` is required".into()));
    }
    let base_url = normalize_base_url(&req.base_url)?;
    let role = validate_plugin_role(req.role.as_deref().unwrap_or("integration").trim())?;

    // Default to a single nav entry at /{id} if the plugin declared none.
    let nav = if req.nav.is_empty() {
        vec![NavEntry::new(&format!("/{}", req.id), name, &req.id)]
    } else {
        req.nav.clone()
    };
    let subscribes: Vec<String> = req
        .subscribes
        .clone()
        .map(|s| {
            s.into_iter()
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| vec!["*".to_string()]);

    // Mint the scoped API key (plaintext returned once; only its hash is stored).
    let api_key = auth::random_token(auth::APIKEY_PREFIX);
    let key_prefix: String = api_key.chars().take(12).collect();
    let api_key_id = format!("key_{}", Uuid::new_v4().simple());
    // Mint the webhook subscription that feeds the sidecar.
    let webhook_secret = auth::random_token(WEBHOOK_SECRET_PREFIX);
    let webhook_id = format!("whs_{}", Uuid::new_v4().simple());
    let webhook_url = format!("{base_url}{WEBHOOK_EVENTS_PATH}");
    let now = Utc::now();

    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO api_keys (id, name, key_hash, key_prefix, role, active, created_at, capabilities)
         VALUES (?,?,?,?,?,1,?,?)",
    )
    .bind(&api_key_id)
    .bind(format!("module:{}", req.id))
    .bind(auth::token_hash(&api_key))
    .bind(&key_prefix)
    .bind(role)
    .bind(now)
    .bind(serde_json::to_string(&plugin_capabilities(role)).unwrap_or_else(|_| "[]".into()))
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO webhook_subscriptions
           (id, name, url, event_types, min_severity, secret, enabled, cursor_at, created_at, updated_at)
         VALUES (?,?,?,?,?,?,1,?,?,?)",
    )
    .bind(&webhook_id)
    .bind(format!("module:{}", req.id))
    .bind(&webhook_url)
    .bind(SqlxJson(&subscribes))
    .bind("info")
    .bind(&webhook_secret)
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO module_registrations
           (id, name, version, publisher, description, base_url, nav, subscribes, role,
            api_key_id, webhook_id, health, created_at, updated_at)
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(&req.id)
    .bind(name)
    .bind(req.version.trim())
    .bind(req.publisher.trim())
    .bind(req.description.trim())
    .bind(&base_url)
    .bind(SqlxJson(&nav))
    .bind(SqlxJson(&subscribes))
    .bind(role)
    .bind(&api_key_id)
    .bind(&webhook_id)
    .bind("unknown")
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let row = get_registered(pool, &req.id)
        .await?
        .ok_or_else(|| AppError::Other(anyhow::anyhow!("module row missing after insert")))?;
    Ok((row, api_key, webhook_secret))
}

/// Unregister a sidecar: delete its row, revoke its API key, and remove its webhook subscription
/// (its delivery ledger cascades). Idempotent-ish: returns NotFound if the id is unknown.
pub async fn unregister(pool: &SqlitePool, id: &str) -> AppResult<()> {
    let row = get_registered(pool, id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("module `{id}` not found")))?;
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM module_registrations WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    if let Some(key_id) = &row.api_key_id {
        sqlx::query("DELETE FROM api_keys WHERE id = ?")
            .bind(key_id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(webhook_id) = &row.webhook_id {
        sqlx::query("DELETE FROM webhook_subscriptions WHERE id = ?")
            .bind(webhook_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn list_registered(pool: &SqlitePool) -> AppResult<Vec<ModuleRegistration>> {
    Ok(sqlx::query_as::<_, ModuleRegistration>(
        "SELECT * FROM module_registrations ORDER BY created_at ASC",
    )
    .fetch_all(pool)
    .await?)
}

pub async fn get_registered(pool: &SqlitePool, id: &str) -> AppResult<Option<ModuleRegistration>> {
    Ok(
        sqlx::query_as::<_, ModuleRegistration>("SELECT * FROM module_registrations WHERE id = ?")
            .bind(id)
            .fetch_all(pool)
            .await?
            .into_iter()
            .next(),
    )
}

/// Background loop: probe each registered sidecar's `/heldar/health` and record reachability so the
/// dashboard can badge healthy/unreachable plugins. Never returns (supervised).
pub async fn run(pool: SqlitePool) {
    let policy = crate::net_guard::EgressPolicy::LAN;
    let timeout = Duration::from_secs(5);
    let mut tick = tokio::time::interval(Duration::from_secs(30));
    loop {
        tick.tick().await;
        let mods = match list_registered(&pool).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, "modules: health sweep failed to list registrations");
                continue;
            }
        };
        for m in mods {
            let url = format!("{}{}", m.base_url, HEALTH_PATH);
            // Resolve-validate-PIN per probe: `base_url` was screened at registration, but the health
            // probe is unattended and a hostname base_url could resolve (or rebind) to the cloud
            // metadata/link-local range — so re-check and pin the connection to the validated IP here.
            let health = if probe_health(&url, &policy, timeout).await {
                "healthy"
            } else {
                "unreachable"
            };
            if let Err(e) = sqlx::query(
                "UPDATE module_registrations SET health = ?, health_checked_at = ? WHERE id = ?",
            )
            .bind(health)
            .bind(Utc::now())
            .bind(&m.id)
            .execute(&pool)
            .await
            {
                tracing::warn!(module = %m.id, error = %e, "modules: failed to record health");
            }
        }
    }
}

/// One health probe: resolve-validate the sidecar health URL, pin the connection to the checked
/// address, and GET it. Returns `true` only on a 2xx. A blocked/forbidden target, a resolution failure,
/// or any transport error is reported as unhealthy (never a connection to an unvalidated address).
async fn probe_health(
    url: &str,
    policy: &crate::net_guard::EgressPolicy,
    timeout: Duration,
) -> bool {
    let Ok(parsed) = crate::net_guard::validate_egress_url(url, policy) else {
        return false;
    };
    let Ok(client) = crate::net_guard::resolve_validate_pin(&parsed, policy, timeout).await else {
        return false;
    };
    matches!(client.get(url).send().await, Ok(r) if r.status().is_success())
}
