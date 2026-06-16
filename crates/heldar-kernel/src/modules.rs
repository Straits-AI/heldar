//! Module manifests — the compile-time half of the plugin platform.
//!
//! A [`ModuleManifest`] describes one loaded module (an app crate today; a runtime-registered sidecar
//! plugin in a later phase). The composing binary collects every module's manifest into
//! [`crate::state::AppState::modules`], and `GET /api/v1/modules` serves the set so the dashboard
//! renders its nav + routes from live truth instead of a hardcoded list. The kernel itself ships no
//! manifest and names no module — it only carries and serves whatever the binary composes.

use serde::{Deserialize, Serialize};

/// Where a module comes from. Drives how the (future) plugin store shelves it and how the dashboard
/// badges it. Runtime-imported plugins use [`ModuleKind::Imported`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleKind {
    /// First-party, open (Apache-2.0) module compiled into the build.
    Core,
    /// First-party proprietary vertical compiled into the build.
    Proprietary,
    /// Third-party / user module loaded at runtime (later phase).
    Imported,
}

/// A nav destination a module contributes to the dashboard. `icon` is a key the dashboard resolves to
/// a glyph, falling back to a generic module glyph for unknown keys (so imported plugins still render).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NavEntry {
    /// Client route path, e.g. `/entry`.
    pub path: String,
    /// Human label shown in the nav rail.
    pub label: String,
    /// Icon key the dashboard maps to a glyph.
    pub icon: String,
}

/// Describes one loaded module. Serialized as-is at `GET /api/v1/modules`.
#[derive(Clone, Debug, Serialize)]
pub struct ModuleManifest {
    /// Stable id, e.g. `entry`. The dashboard keys its page registry on this.
    pub id: String,
    /// Display name, e.g. `Access Control`.
    pub name: String,
    /// Module version (the crate version for compiled modules).
    pub version: String,
    /// Who publishes the module.
    pub publisher: String,
    /// Provenance (core / proprietary / imported).
    pub kind: ModuleKind,
    /// One-line description for the module list / store.
    pub description: String,
    /// Nav entries this module contributes (usually one).
    pub nav: Vec<NavEntry>,
    /// How the dashboard renders the module's content. Compiled modules use a `bundled` page; runtime
    /// sidecar plugins are `iframe`-mounted at `/m/{id}/` (the kernel reverse-proxies to the sidecar).
    pub mount: MountKind,
    /// Reachability of a sidecar's base URL (`unknown`/`healthy`/`unreachable`); `None` for compiled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<String>,
}

/// How the dashboard renders a module's content area.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MountKind {
    /// A page component shipped in the dashboard bundle, keyed by module id (compiled modules).
    Bundled,
    /// An iframe to `/m/{id}/`, which the kernel reverse-proxies to the sidecar (imported modules).
    Iframe,
}

impl ModuleManifest {
    /// Convenience builder for a single-nav-entry compiled (bundled) module.
    pub fn new(
        id: &str,
        name: &str,
        version: &str,
        publisher: &str,
        kind: ModuleKind,
        description: &str,
        nav: Vec<NavEntry>,
    ) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            version: version.to_string(),
            publisher: publisher.to_string(),
            kind,
            description: description.to_string(),
            nav,
            mount: MountKind::Bundled,
            health: None,
        }
    }
}

impl NavEntry {
    pub fn new(path: &str, label: &str, icon: &str) -> Self {
        Self {
            path: path.to_string(),
            label: label.to_string(),
            icon: icon.to_string(),
        }
    }
}

/* ------------------------------------------------------------------ */
/* Runtime sidecar registrations (Phase B)                            */
/* ------------------------------------------------------------------ */

/// The manifest a sidecar plugin presents to register itself (POST /api/v1/modules). The kernel mints
/// a scoped API key + a webhook subscription from it and reverse-proxies `/m/{id}/*` to `base_url`.
#[derive(Clone, Debug, Deserialize)]
pub struct ModuleRegisterRequest {
    /// Stable id (slug): the `/m/{id}/` mount + nav key. Must not collide with a compiled module.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub publisher: String,
    #[serde(default)]
    pub description: String,
    /// The sidecar's origin the kernel reverse-proxies to (http/https), e.g. `http://127.0.0.1:9123`.
    pub base_url: String,
    /// Nav entries to surface (defaults to one entry at `/{id}` if omitted).
    #[serde(default)]
    pub nav: Vec<NavEntry>,
    /// Event types to deliver to the sidecar's webhook (`["*"]` = all). Defaults to all.
    #[serde(default)]
    pub subscribes: Option<Vec<String>>,
    /// Role of the minted API key. Restricted to least-privilege (`viewer` | `integration`).
    #[serde(default)]
    pub role: Option<String>,
}

/// A stored sidecar registration row.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct ModuleRegistration {
    pub id: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub description: String,
    pub base_url: String,
    /// JSON array of [`NavEntry`].
    pub nav: sqlx::types::Json<Vec<NavEntry>>,
    /// JSON array of event-type tokens.
    pub subscribes: sqlx::types::Json<Vec<String>>,
    pub role: String,
    pub api_key_id: Option<String>,
    pub webhook_id: Option<String>,
    pub health: String,
    pub health_checked_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl ModuleRegistration {
    /// Project the stored row into the uniform manifest the dashboard consumes (kind = imported,
    /// iframe-mounted, with live health).
    pub fn to_manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: self.id.clone(),
            name: self.name.clone(),
            version: self.version.clone(),
            publisher: self.publisher.clone(),
            kind: ModuleKind::Imported,
            description: self.description.clone(),
            nav: self.nav.0.clone(),
            mount: MountKind::Iframe,
            health: Some(self.health.clone()),
        }
    }
}

/// Admin-only detail for a single registration (includes the sidecar URL + minted resource ids).
#[derive(Clone, Debug, Serialize)]
pub struct ModuleDetail {
    pub id: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub description: String,
    pub base_url: String,
    pub nav: Vec<NavEntry>,
    pub subscribes: Vec<String>,
    pub role: String,
    pub api_key_id: Option<String>,
    pub webhook_id: Option<String>,
    pub health: String,
    pub health_checked_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<&ModuleRegistration> for ModuleDetail {
    fn from(r: &ModuleRegistration) -> Self {
        ModuleDetail {
            id: r.id.clone(),
            name: r.name.clone(),
            version: r.version.clone(),
            publisher: r.publisher.clone(),
            description: r.description.clone(),
            base_url: r.base_url.clone(),
            nav: r.nav.0.clone(),
            subscribes: r.subscribes.0.clone(),
            role: r.role.clone(),
            api_key_id: r.api_key_id.clone(),
            webhook_id: r.webhook_id.clone(),
            health: r.health.clone(),
            health_checked_at: r.health_checked_at,
            created_at: r.created_at,
        }
    }
}

/// The once-returned credentials a freshly registered sidecar needs to configure itself.
#[derive(Clone, Debug, Serialize)]
pub struct ModuleRegistered {
    pub module: ModuleDetail,
    /// The minted API key (plaintext, returned ONCE) the sidecar uses to call kernel APIs.
    pub api_key: String,
    /// The HMAC-SHA256 secret (returned ONCE) the kernel signs the sidecar's webhook deliveries with.
    pub webhook_secret: String,
}
