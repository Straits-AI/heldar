use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::config::Config;
use crate::modules::ModuleManifest;
use crate::services::consumer::DetectionConsumer;
use crate::services::live_publisher::LivePublisherManager;
use crate::services::mirror::MirrorRecorderManager;
use crate::services::recorder::RecorderManager;
use crate::services::sampler::SamplerManager;

/// Shared application state, cloned cheaply into every handler and background task.
///
/// Note the kernel holds NO concrete domain engine: perception interpreters (zones, ANPR/entry, and
/// future apps) are registered as [`DetectionConsumer`]s in `consumers`, so the ingest path and this
/// struct stay domain-agnostic. After the crate split the composing binary decides which app crates
/// populate the registry.
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub cfg: Arc<Config>,
    pub recorder: Arc<RecorderManager>,
    /// Dual/mirror recorder, present only when `HELDAR_MIRROR_RECORDINGS_DIR` is configured.
    pub mirror: Option<Arc<MirrorRecorderManager>>,
    pub sampler: Arc<SamplerManager>,
    /// Kernel-owned live preview publishers (the HEVC→H.264 transcode ffmpegs feeding MediaMTX).
    pub live: Arc<LivePublisherManager>,
    /// Registered perception consumers, fanned out to from detection ingest.
    pub consumers: Arc<Vec<Arc<dyn DetectionConsumer>>>,
    /// Loaded module manifests (composed by the binary), served at `GET /api/v1/modules` so the
    /// dashboard renders nav + routes from live truth. The kernel names no module — it only carries
    /// whatever the composing server populated.
    pub modules: Arc<Vec<ModuleManifest>>,
    /// The plugin store's catalog engine (bundled + signed remote registries).
    pub catalog: Arc<crate::services::registry::CatalogService>,
    pub http: reqwest::Client,
    pub started_at: DateTime<Utc>,
}

impl AppState {
    /// Load a camera ON BEHALF OF a caller: camera scope first, then existence.
    ///
    /// This is the single choke point for camera scoping on the request path. The order matters — an
    /// out-of-scope id answers 403 whether or not the camera exists, so the scope boundary cannot be
    /// used as an existence oracle for the rest of the fleet.
    ///
    /// Background services (recorder, sampler, live publisher, mirror, ANR) deliberately do NOT go
    /// through here: they never hold a `Principal`, and recording must never acquire a new failure mode
    /// from the auth layer. They keep the raw query.
    pub async fn camera_for(
        &self,
        principal: &crate::auth::Principal,
        id: &str,
    ) -> crate::error::AppResult<crate::models::Camera> {
        principal.require_camera(id, "access this camera")?;
        crate::routes::cameras::load_camera(&self.pool, id).await
    }

    /// Assert camera scope without loading the row — for handlers that only need the id (they 404 via
    /// their own query, or address a per-camera resource rather than the camera itself).
    pub fn camera_scope_check(
        &self,
        principal: &crate::auth::Principal,
        id: &str,
    ) -> crate::error::AppResult<()> {
        principal.require_camera(id, "access this camera")
    }
}

/// SQL predicate + bind values restricting a camera-keyed list query to a principal's scope.
///
/// Returns `None` when the caller is unrestricted (the overwhelming default), so unscoped callers pay
/// no predicate at all. Otherwise returns `(" AND camera_id IN (?,?,…)", ids)` — an EMPTY allowlist
/// yields `IN ()`-equivalent `AND 0`, i.e. no rows, which is the fail-closed answer.
pub fn camera_scope_filter(
    principal: &crate::auth::Principal,
    column: &str,
) -> Option<(String, Vec<String>)> {
    let ids = principal.camera_scope()?;
    if ids.is_empty() {
        return Some((" AND 0".to_string(), Vec::new()));
    }
    let mut sorted: Vec<String> = ids.iter().cloned().collect();
    sorted.sort();
    let placeholders = vec!["?"; sorted.len()].join(",");
    Some((format!(" AND {column} IN ({placeholders})"), sorted))
}
