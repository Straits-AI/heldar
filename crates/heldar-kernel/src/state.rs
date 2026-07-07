use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::config::Config;
use crate::modules::ModuleManifest;
use crate::services::consumer::DetectionConsumer;
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
