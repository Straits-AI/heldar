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
    /// Ceiling on concurrent interactive media jobs, so exports cannot starve the recorder.
    pub media_jobs: crate::services::media_jobs::MediaJobGovernor,
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

    /// Resolve the camera OWNING a resource addressed by its own primary key, refusing an
    /// out-of-scope resource BEFORE its existence is disclosed.
    ///
    /// This is the resource-id twin of [`AppState::camera_for`]. A handler that takes `{zone_id}`,
    /// `{schedule_id}`, `{task_id}` or a bare segment id cannot use `camera_for` (it has no camera id
    /// yet) and must not reach for `require_camera(&row.camera_id, …)` either: that answers 404 for a
    /// missing row and 403 for someone else's, which turns the boundary into an id-space oracle, and
    /// its message embeds the owning camera id, which hands a scoped caller the fleet roster one probe
    /// at a time.
    ///
    /// - `Scope::All` (every human role, and every key minted without a camera list): behaviour is
    ///   identical to today — the row is looked up and a missing row is a 404 naming the resource.
    /// - `Scope::Cameras`: "owned by a camera you do not hold" and "does not exist" produce the SAME
    ///   [`AppError`] value, byte for byte, so the id space cannot be enumerated.
    ///
    /// Background services never call this: they hold no `Principal` and keep their raw queries.
    pub async fn resource_camera(
        &self,
        principal: &crate::auth::Principal,
        kind: CameraOwned,
        resource_id: &str,
        action: &str,
    ) -> crate::error::AppResult<String> {
        // `kind.table()` is a compile-time constant from a closed enum, so this `format!` carries no
        // injection surface; the resource id itself is always bound.
        let owner: Option<String> = sqlx::query_scalar(&format!(
            "SELECT camera_id FROM {} WHERE id = ?",
            kind.table()
        ))
        .bind(resource_id)
        .fetch_optional(&self.pool)
        .await?;
        match owner {
            Some(cam) if principal.camera_allowed(&cam) => Ok(cam),
            // Scoped: missing and out-of-scope are indistinguishable, deliberately.
            _ if principal.camera_scope().is_some() => Err(scope_denied(kind, action)),
            None => Err(crate::error::AppError::NotFound(format!(
                "{} {resource_id} not found",
                kind.noun()
            ))),
            // Unreachable in practice: `Scope::All` allows every camera, so an existing row always
            // matched the first arm. Kept total rather than panicking on a request path.
            Some(_) => Err(scope_denied(kind, action)),
        }
    }
}

/// Camera-owned tables addressable by their OWN primary key, for [`AppState::resource_camera`].
///
/// A closed enum, so the table name below is a compile-time constant rather than caller input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraOwned {
    Zone,
    Schedule,
    SnapshotSchedule,
    AiTask,
    Segment,
    Detection,
}

impl CameraOwned {
    /// The table holding the resource. Every one of these has a NOT NULL `camera_id`.
    pub const fn table(self) -> &'static str {
        match self {
            CameraOwned::Zone => "zones",
            CameraOwned::Schedule => "camera_schedules",
            CameraOwned::SnapshotSchedule => "snapshot_schedules",
            CameraOwned::AiTask => "ai_tasks",
            CameraOwned::Segment => "segments",
            CameraOwned::Detection => "detections",
        }
    }

    /// How the resource is named in an error message.
    pub const fn noun(self) -> &'static str {
        match self {
            CameraOwned::Zone => "zone",
            CameraOwned::Schedule => "schedule",
            CameraOwned::SnapshotSchedule => "snapshot schedule",
            CameraOwned::AiTask => "ai task",
            CameraOwned::Segment => "segment",
            CameraOwned::Detection => "detection",
        }
    }
}

/// The ONE 403 the resource-id path may produce.
///
/// It names neither the resource id nor the owning camera: on this path both would be new
/// information for the caller, and the whole point of the resource-id loader is that a refusal
/// carries no bits about what exists.
pub fn scope_denied(kind: CameraOwned, action: &str) -> crate::error::AppError {
    scope_denied_owner(kind.noun(), action)
}

/// [`scope_denied`] for a resource that is not a row in a `CameraOwned` table — a playback session on
/// disk, a media artifact. Same wording, so the two refusals stay indistinguishable.
pub fn scope_denied_owner(noun: &str, action: &str) -> crate::error::AppError {
    crate::error::AppError::Forbidden(format!(
        "credential is not scoped to the camera owning this {noun} (cannot {action})"
    ))
}

/// Confine a CALLER-SUPPLIED camera list to the principal's scope.
///
/// - Unscoped (`Scope::All`): returned verbatim, including the empty list — an empty list still means
///   "all cameras" for an unscoped caller, exactly as today.
/// - Scoped: an EMPTY list expands to the principal's own scope and never to "all"; a non-empty list
///   must be a SUBSET of the scope, else 403. It is never silently narrowed, because a caller that
///   asked for a camera it does not hold has made an error worth reporting.
pub fn confine_camera_ids(
    principal: &crate::auth::Principal,
    requested: &[String],
) -> crate::error::AppResult<Vec<String>> {
    let Some(scope) = principal.camera_scope() else {
        return Ok(requested.to_vec());
    };
    if requested.is_empty() {
        let mut ids: Vec<String> = scope.iter().cloned().collect();
        ids.sort();
        return Ok(ids);
    }
    if requested.iter().all(|c| scope.contains(c)) {
        Ok(requested.to_vec())
    } else {
        // Names no camera: the refusal must not confirm which requested ids exist on the box.
        Err(crate::error::AppError::Forbidden(
            "credential is not scoped to every camera named in this request".to_string(),
        ))
    }
}

/// Coerce a raw JSON `camera_ids` field into a list of camera ids.
///
/// Rejects a non-array, and rejects non-string ELEMENTS rather than stringifying them: a stored
/// `[1, {"a": 2}]` would degrade downstream into an unfiltered selection, which is the same
/// "empty means everything" failure this module exists to make unrepresentable.
pub fn camera_ids_from_json(v: &serde_json::Value) -> crate::error::AppResult<Vec<String>> {
    let items = v.as_array().ok_or_else(|| {
        crate::error::AppError::BadRequest(
            "camera_ids must be an array of camera id strings".to_string(),
        )
    })?;
    items
        .iter()
        .map(|i| {
            i.as_str().map(str::to_string).ok_or_else(|| {
                crate::error::AppError::BadRequest(
                    "camera_ids must be an array of camera id strings".to_string(),
                )
            })
        })
        .collect()
}

/// Which cameras a fleet-wide job (backup policy, archive export) covers.
///
/// Exists so that "empty list" can no longer mean "every camera on the box". A `Vec<String>` that
/// happens to be empty is a bug waiting to be reintroduced by a caller that forgets to confine it;
/// [`CameraSelection::Only`] with an empty vector selects NOTHING, and [`CameraSelection::All`] can
/// only be produced from an unscoped principal via [`camera_selection`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CameraSelection {
    /// Every camera on the box. Reachable only for an unscoped principal.
    All,
    /// Exactly these cameras — an empty vector selects nothing.
    Only(Vec<String>),
}

impl CameraSelection {
    pub fn is_all(&self) -> bool {
        matches!(self, CameraSelection::All)
    }

    /// The explicit id list, or `None` for [`CameraSelection::All`].
    pub fn ids(&self) -> Option<&[String]> {
        match self {
            CameraSelection::All => None,
            CameraSelection::Only(v) => Some(v),
        }
    }
}

/// Build a [`CameraSelection`] from a caller-supplied list, confined to the principal's scope.
///
/// `All` is produced only when the principal is unscoped AND asked for everything. A scoped caller
/// can never reach `All`, so no code path downstream can widen back to the fleet.
pub fn camera_selection(
    principal: &crate::auth::Principal,
    requested: &[String],
) -> crate::error::AppResult<CameraSelection> {
    let confined = confine_camera_ids(principal, requested)?;
    if principal.camera_scope().is_none() && requested.is_empty() {
        Ok(CameraSelection::All)
    } else {
        Ok(CameraSelection::Only(confined))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{Principal, Scope};
    use crate::error::AppError;
    use std::collections::HashSet;

    fn scoped(cameras: &[&str]) -> Principal {
        let set: HashSet<String> = cameras.iter().map(|c| c.to_string()).collect();
        Principal {
            scope: Scope::Cameras(std::sync::Arc::new(set)),
            ..Principal::system_admin()
        }
    }

    async fn test_state() -> AppState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        let cfg = Arc::new(crate::config::Config::from_env());
        AppState {
            recorder: crate::services::recorder::RecorderManager::new(pool.clone(), cfg.clone()),
            sampler: crate::services::sampler::SamplerManager::new(pool.clone(), cfg.clone()),
            live: crate::services::live_publisher::LivePublisherManager::new(
                pool.clone(),
                cfg.clone(),
                reqwest::Client::new(),
            ),
            mirror: None,
            consumers: Arc::new(Vec::new()),
            modules: Arc::new(Vec::new()),
            catalog: Arc::new(crate::services::registry::CatalogService::new(&cfg)),
            http: reqwest::Client::new(),
            media_jobs: crate::services::media_jobs::MediaJobGovernor::new(2),
            started_at: Utc::now(),
            pool,
            cfg,
        }
    }

    async fn insert_camera_and_zone(pool: &sqlx::SqlitePool, camera_id: &str, zone_id: &str) {
        let now = Utc::now();
        sqlx::query("INSERT INTO cameras (id, name, created_at, updated_at) VALUES (?, ?, ?, ?)")
            .bind(camera_id)
            .bind(camera_id)
            .bind(now)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO zones (id, camera_id, name, polygon, created_at, updated_at)
             VALUES (?, ?, 'z', '[]', ?, ?)",
        )
        .bind(zone_id)
        .bind(camera_id)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
    }

    fn body_of(e: &AppError) -> String {
        e.to_string()
    }

    #[tokio::test]
    async fn resource_camera_is_unchanged_for_an_unscoped_principal() {
        let st = test_state().await;
        insert_camera_and_zone(&st.pool, "cam_a", "zone_a").await;
        let admin = Principal::system_admin();
        // Present: resolves the owner, exactly as a raw load would.
        let owner = st
            .resource_camera(&admin, CameraOwned::Zone, "zone_a", "edit zones")
            .await
            .unwrap();
        assert_eq!(owner, "cam_a");
        // Missing: still the pre-existing 404, not a new refusal.
        let err = st
            .resource_camera(&admin, CameraOwned::Zone, "zone_missing", "edit zones")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn resource_camera_never_becomes_an_existence_oracle() {
        let st = test_state().await;
        insert_camera_and_zone(&st.pool, "cam_a", "zone_a").await;
        insert_camera_and_zone(&st.pool, "cam_b", "zone_b").await;
        let p = scoped(&["cam_a"]);

        // In scope: allowed.
        assert_eq!(
            st.resource_camera(&p, CameraOwned::Zone, "zone_a", "edit zones")
                .await
                .unwrap(),
            "cam_a"
        );

        // Another camera's zone, and a zone that does not exist, must be INDISTINGUISHABLE.
        let out_of_scope = st
            .resource_camera(&p, CameraOwned::Zone, "zone_b", "edit zones")
            .await
            .unwrap_err();
        let nonexistent = st
            .resource_camera(&p, CameraOwned::Zone, "zone_zzz", "edit zones")
            .await
            .unwrap_err();
        assert!(matches!(out_of_scope, AppError::Forbidden(_)));
        assert!(matches!(nonexistent, AppError::Forbidden(_)));
        assert_eq!(body_of(&out_of_scope), body_of(&nonexistent));
        // And the refusal leaks neither the owning camera nor the probed id.
        let msg = body_of(&out_of_scope);
        assert!(!msg.contains("cam_b"), "{msg}");
        assert!(!msg.contains("zone_b"), "{msg}");
    }

    #[test]
    fn confine_camera_ids_is_identity_for_an_unscoped_principal() {
        let admin = Principal::system_admin();
        // Empty still means "all" — the request is passed through untouched.
        assert_eq!(
            confine_camera_ids(&admin, &[]).unwrap(),
            Vec::<String>::new()
        );
        let asked = vec!["cam_a".to_string(), "cam_b".to_string()];
        assert_eq!(confine_camera_ids(&admin, &asked).unwrap(), asked);
    }

    #[test]
    fn confine_camera_ids_expands_empty_to_the_scope_and_refuses_a_superset() {
        let p = scoped(&["cam_a", "cam_c"]);
        assert_eq!(
            confine_camera_ids(&p, &[]).unwrap(),
            vec!["cam_a".to_string(), "cam_c".to_string()]
        );
        assert_eq!(
            confine_camera_ids(&p, &["cam_c".to_string()]).unwrap(),
            vec!["cam_c".to_string()]
        );
        let err = confine_camera_ids(&p, &["cam_a".to_string(), "cam_b".to_string()]).unwrap_err();
        assert!(matches!(err, AppError::Forbidden(_)), "got {err:?}");
        assert!(!body_of(&err).contains("cam_b"));
    }

    #[test]
    fn camera_selection_reaches_all_only_for_an_unscoped_principal() {
        let admin = Principal::system_admin();
        assert_eq!(camera_selection(&admin, &[]).unwrap(), CameraSelection::All);
        assert_eq!(
            camera_selection(&admin, &["cam_a".to_string()]).unwrap(),
            CameraSelection::Only(vec!["cam_a".to_string()])
        );
        // A scoped principal can never widen back to the fleet, whatever it asks for.
        let p = scoped(&["cam_a"]);
        assert_eq!(
            camera_selection(&p, &[]).unwrap(),
            CameraSelection::Only(vec!["cam_a".to_string()])
        );
        // And an empty scope selects nothing rather than everything.
        let none = scoped(&[]);
        assert_eq!(
            camera_selection(&none, &[]).unwrap(),
            CameraSelection::Only(Vec::new())
        );
        assert!(!camera_selection(&none, &[]).unwrap().is_all());
    }

    #[test]
    fn camera_ids_from_json_rejects_non_string_elements() {
        let ok = serde_json::json!(["cam_a", "cam_b"]);
        assert_eq!(
            camera_ids_from_json(&ok).unwrap(),
            vec!["cam_a".to_string(), "cam_b".to_string()]
        );
        for bad in [
            serde_json::json!([1, { "a": 2 }]),
            serde_json::json!(["cam_a", 7]),
            serde_json::json!({}),
            serde_json::json!(null),
            serde_json::json!("cam_a"),
        ] {
            let err = camera_ids_from_json(&bad).unwrap_err();
            assert!(matches!(err, AppError::BadRequest(_)), "{bad} -> {err:?}");
        }
    }

    #[test]
    fn camera_scope_filter_is_unchanged() {
        assert!(camera_scope_filter(&Principal::system_admin(), "camera_id").is_none());
        let (sql, binds) =
            camera_scope_filter(&scoped(&["cam_b", "cam_a"]), "t.camera_id").unwrap();
        assert_eq!(sql, " AND t.camera_id IN (?,?)");
        assert_eq!(binds, vec!["cam_a".to_string(), "cam_b".to_string()]);
        // Empty allowlist: fail closed, and ZERO binds — call sites must bind from the returned vec.
        let (sql, binds) = camera_scope_filter(&scoped(&[]), "camera_id").unwrap();
        assert_eq!(sql, " AND 0");
        assert!(binds.is_empty());
    }
}
