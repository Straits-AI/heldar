//! Module manifests — the compile-time half of the plugin platform.
//!
//! A [`ModuleManifest`] describes one loaded module (an app crate today; a runtime-registered sidecar
//! plugin in a later phase). The composing binary collects every module's manifest into
//! [`crate::state::AppState::modules`], and `GET /api/v1/modules` serves the set so the dashboard
//! renders its nav + routes from live truth instead of a hardcoded list. The kernel itself ships no
//! manifest and names no module — it only carries and serves whatever the binary composes.

use serde::Serialize;

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
#[derive(Clone, Debug, Serialize)]
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
}

impl ModuleManifest {
    /// Convenience builder for a single-nav-entry compiled module.
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
