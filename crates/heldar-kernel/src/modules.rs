//! Module manifests — the compile-time half of the plugin platform.
//!
//! A [`ModuleManifest`] describes one loaded module (an app crate today; a runtime-registered sidecar
//! plugin in a later phase). The composing binary collects every module's manifest into
//! [`crate::state::AppState::modules`], and `GET /api/v1/modules` serves the set so the dashboard
//! renders its nav + routes from live truth instead of a hardcoded list. The kernel itself ships no
//! manifest and names no module — it only carries and serves whatever the binary composes.

use serde::{Deserialize, Serialize};

/// Where a module comes from. Drives how the plugin store shelves it and how the dashboard badges it.
/// Runtime-imported (bring-your-own) plugins use [`ModuleKind::Imported`]; catalog-listed third-party
/// plugins use [`ModuleKind::Community`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleKind {
    /// First-party, open (Apache-2.0) module compiled into the build.
    Core,
    /// First-party proprietary vertical compiled into the build.
    Proprietary,
    /// Third-party plugin listed in a registry catalog.
    Community,
    /// A runtime-loaded sidecar plugin (bring-your-own, installed by URL).
    Imported,
}

/// A nav destination a module contributes to the dashboard. `icon` is a key the dashboard resolves to
/// a glyph, falling back to a generic module glyph for unknown keys (so imported plugins still render).
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
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
    /// How the dashboard renders the module's content: a `bundled` page (compiled), a `runtime` UI
    /// bundle imported from `ui_url`, an `iframe` to `/m/{id}/` (sidecar reverse-proxy), or `headless`.
    pub mount: MountKind,
    /// For `mount: runtime`, the URL of the module's UI bundle — an ES module the dashboard imports at
    /// runtime and mounts (native React, shared with the shell). `None` for other mounts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_url: Option<String>,
    /// Reachability of a sidecar's base URL (`unknown`/`healthy`/`unreachable`); `None` for compiled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<String>,
}

/// How the dashboard renders a module's content area.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MountKind {
    /// Legacy: a page component compiled into the dashboard bundle, keyed by module id. UNUSED — the
    /// dashboard no longer bundles any module page, so no shipped module reports this and the dashboard
    /// renders no route for it. Retained only as the `new()` default until a manifest calls
    /// `with_runtime_ui` / `with_iframe`. Prefer `Runtime`.
    Bundled,
    /// A UI bundle imported at runtime from the manifest's `ui_url` (native React, shared with the
    /// shell). The module serves its own bundle (in-process modules via their `Router` seam). This is
    /// how every first-party in-process module (entry/movement/search/verticals) ships its UI.
    Runtime,
    /// An iframe to `/m/{id}/`, which the kernel reverse-proxies to the sidecar (imported modules).
    Iframe,
    /// No UI — a headless compute plugin (e.g. a sandboxed Wasm DetectionConsumer). Contributes no nav
    /// route; the store lists it with a "compute" treatment and no Open affordance.
    Headless,
}

impl ModuleManifest {
    /// Convenience builder for a single-nav-entry in-process module. Defaults to `mount: Bundled` with
    /// no UI; call `with_runtime_ui` to point the dashboard at the module's runtime-loaded bundle.
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
            ui_url: None,
            health: None,
        }
    }

    /// Mark this in-process module as runtime-UI: the dashboard imports its bundle from `ui_url` at
    /// runtime (native React, shared with the shell) instead of a page baked into the dashboard bundle.
    pub fn with_runtime_ui(mut self, ui_url: &str) -> Self {
        self.mount = MountKind::Runtime;
        self.ui_url = Some(ui_url.to_string());
        self
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
#[derive(Clone, Debug, Deserialize, utoipa::ToSchema)]
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
            ui_url: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_manifest_serializes_mount_and_ui_url() {
        let m = ModuleManifest::new(
            "search",
            "Search",
            "0.1.0",
            "Heldar",
            ModuleKind::Core,
            "desc",
            vec![NavEntry::new("/search", "Search", "search")],
        )
        .with_runtime_ui("/api/v1/modules/search/ui/index.js");
        assert_eq!(m.mount, MountKind::Runtime);
        let j = serde_json::to_value(&m).unwrap();
        assert_eq!(j["mount"], "runtime");
        assert_eq!(j["ui_url"], "/api/v1/modules/search/ui/index.js");
    }

    #[test]
    fn bundled_manifest_omits_ui_url() {
        let m = ModuleManifest::new("x", "X", "0.1.0", "Heldar", ModuleKind::Core, "d", vec![]);
        assert_eq!(m.mount, MountKind::Bundled);
        let j = serde_json::to_value(&m).unwrap();
        assert!(j.get("ui_url").is_none(), "ui_url is skipped when None");
    }
}
