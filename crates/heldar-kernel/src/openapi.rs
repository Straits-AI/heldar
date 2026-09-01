//! The machine-readable API contract, generated FROM the route handlers.
//!
//! The contract already existed — spread across route definitions, request/response structs,
//! dashboard types and Markdown — which is precisely the arrangement that drifts. Anything
//! hand-maintained alongside the code eventually disagrees with it; this session already found a
//! README claiming `HELDAR_INGEST_PROVENANCE` was auto-promoted when `config.rs` says it never is.
//! So the spec is generated from `#[utoipa::path]` annotations on the handlers themselves, and the
//! drift is caught by a test rather than by an integrator.
//!
//! # This is deliberately partial, and the test says so
//!
//! 151 routes are served. Annotating all of them in one change would be unreviewable, so the routes
//! documented so far are listed in [`ApiDoc`] and the rest are named in the
//! `openapi_covers_every_route` test's allowlist. That list only shrinks: adding a route without
//! documenting it fails CI, exactly as the route census made an unguarded route fail CI.
//!
//! Served at `GET /api/v1/openapi.json`, inside the `/api/v1` auth floor — the surface map is not
//! something to hand to an unauthenticated caller. Publishing it for codegen is the release
//! artifact's job, not this endpoint's.

use axum::routing::get;
use axum::{Json, Router};
use utoipa::OpenApi;

use crate::state::AppState;

/// The error body EVERY endpoint can return.
///
/// One shape, documented once and referenced everywhere, so a client writes one error path instead
/// of guessing per route. This mirrors what `AppError::into_response` actually emits today
/// (`{"error": "...", "code": "...", "retryable": false}`) rather than an aspirational envelope — a spec that describes a body the
/// server does not send is worse than no spec.
/// An `[x, y]` pair, normalized 0..1 — the unit every geometry in this API speaks (#156).
///
/// # Why this is hand-written
///
/// The Rust types are already precise: `Vec<[f64; 2]>` cannot hold a triple. utoipa's DERIVE is what
/// cannot say so — `min_items`/`max_items` are field attributes, so there is nowhere to put them on a
/// newtype's own schema, and `[f64; 2]` reaches every generated client as a bare `number[][]`. A
/// previous attempt hit exactly that and recorded the field as blocked.
///
/// Implementing `PartialSchema` by hand sidesteps the derive entirely and says it exactly. Fields
/// keep their real Rust types and point at this one with `#[schema(value_type = Vec<Coordinate>)]`,
/// so nothing about parsing or validation changes — only what the contract can express.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Coordinate(pub [f64; 2]);

impl utoipa::PartialSchema for Coordinate {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        utoipa::openapi::schema::ArrayBuilder::new()
            .items(utoipa::openapi::schema::ObjectBuilder::new().schema_type(
                utoipa::openapi::schema::SchemaType::Type(utoipa::openapi::schema::Type::Number),
            ))
            .min_items(Some(2))
            .max_items(Some(2))
            .description(Some(
                "An [x, y] pair, normalized 0..1 against the frame's width and height.",
            ))
            .into()
    }
}

impl utoipa::ToSchema for Coordinate {
    fn name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("Coordinate")
    }
}

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct ErrorBody {
    /// Human-readable message. Not a stable identifier — do not match on it.
    #[schema(example = "camera cam_x not found")]
    pub error: String,
    /// Stable machine-readable identifier. Branch on this, not on `error`.
    ///
    /// Exactly one of: `bad_request`, `unauthorized`, `forbidden`, `not_found`, `conflict`,
    /// `payload_too_large`, `rate_limited`, `unavailable`, `internal`.
    ///
    /// This list is held to `AppError::ALL_CODES` by `codes_documented_match_codes_returned`, in
    /// both directions — a code the server can emit must appear here, and a code named here must be
    /// reachable. It previously listed one the server has never returned and omitted two it returns
    /// routinely, which is why the test exists rather than a correction. (The test also refuses the
    /// obsolete identifier by name, so this text cannot quietly reintroduce it while explaining it.)
    #[schema(example = "not_found")]
    pub code: String,
    /// Whether retrying the SAME request could plausibly succeed. True only for transient
    /// saturation; a `404` or a validation failure will fail identically forever. Retryable
    /// responses also carry `Retry-After`.
    pub retryable: bool,
}

/// The API CONTRACT's version, reported by `GET /api/v1/system` and stamped into the served spec.
///
/// Deliberately not the crate version. A patch release bumps the binary without moving a single
/// field, and a client that pinned to it would churn for nothing; a breaking API change can land in
/// any release, and a client that ignored it would break silently. This number moves only when the
/// contract does — minor for additive changes, major for anything a generated client must be
/// regenerated for.
///
/// While the contract is partial (most routes are still undocumented, see the module docs) this
/// stays `0.x`: a `1.0` would promise a completeness the document does not yet have.
pub const API_VERSION: &str = "0.1.0";

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Heldar Core API",
        version = API_VERSION,
        description = "Media kernel, perception ingest and access-control surface for a Heldar box.",
        license(name = "Apache-2.0"),
    ),
    servers((url = "/", description = "The box itself")),
    paths(
        crate::routes::cameras::list_cameras,
        crate::routes::cameras::get_camera,
        crate::routes::cameras::delete_camera,
        crate::routes::evidence::create,
        crate::routes::evidence::list,
        crate::routes::evidence::get_one,
        crate::routes::evidence::signing_key,
        crate::routes::system::get_posture,
        crate::routes::system::get_timezone,
        crate::routes::system::put_timezone,
        crate::routes::sites::list,
        crate::routes::sites::get_one,
        crate::routes::sites::create,
        crate::routes::sites::update,
        crate::routes::sites::delete_site,
        crate::routes::recordings::list_segments,
        crate::routes::recordings::timeline,
        crate::routes::recordings::gaps,
        crate::routes::playback::export_clip,
        crate::routes::playback::snapshot_handler,
        crate::routes::playback_sessions::create_session,
        crate::routes::playback_sessions::delete_session,
        crate::routes::recording_control::record_trigger,
        crate::openapi::spec,
        crate::routes::anr::list_gaps,
        crate::routes::anr::retry_gap,
        crate::routes::health::list_status,
        crate::routes::health::camera_status,
        crate::routes::health::list_events,
        crate::routes::incidents::lock_evidence,
        crate::routes::incidents::unlock_evidence,
        crate::routes::incidents::tag_incident,
        crate::routes::incidents::list_incidents,
        crate::routes::incidents::incident_segments,
        crate::routes::modules::list,
        crate::routes::modules::register,
        crate::routes::modules::detail,
        crate::routes::modules::unregister,
        crate::routes::outbox::list_outbox,
        crate::routes::outbox::site_info,
        crate::routes::registry::list,
        crate::routes::registry::refresh,
        crate::routes::snapshot_schedules::list_schedules,
        crate::routes::snapshot_schedules::create_schedule,
        crate::routes::snapshot_schedules::update_schedule,
        crate::routes::snapshot_schedules::delete_schedule,
        crate::routes::snapshot_schedules::list_snapshots,
        crate::routes::liveview::liveview,
        crate::routes::schedules::list_schedules,
        crate::routes::schedules::create_schedule,
        crate::routes::schedules::update_schedule,
        crate::routes::schedules::delete_schedule,
        crate::routes::discovery::discover_handler,
        crate::routes::cameras::test_camera,
        crate::routes::ai::acquire_lease,
        crate::routes::ai::claim_embed_queries,
        crate::routes::ai::create_task,
        crate::routes::ai::delete_task,
        crate::routes::ai::embed_query_result,
        crate::routes::ai::ingest,
        crate::routes::ai::ingest_embeddings,
        crate::routes::ai::latest_frame,
        crate::routes::ai::list_all_tasks,
        crate::routes::ai::list_camera_tasks,
        crate::routes::ai::list_detections,
        crate::routes::ai::release_lease,
        crate::routes::ai::sampler_status,
        crate::routes::ai::update_task,
        crate::routes::auth::create_api_key,
        crate::routes::auth::create_user,
        crate::routes::auth::delete_api_key,
        crate::routes::auth::delete_user,
        crate::routes::auth::list_api_keys,
        crate::routes::auth::list_users,
        crate::routes::auth::login,
        crate::routes::auth::logout,
        crate::routes::auth::me,
        crate::routes::auth::unlock_user,
        crate::routes::auth::update_api_key,
        crate::routes::auth::update_user,
        crate::routes::backup::archive_export,
        crate::routes::backup::create_destination,
        crate::routes::backup::create_policy,
        crate::routes::backup::delete_destination,
        crate::routes::backup::delete_job,
        crate::routes::backup::delete_policy,
        crate::routes::backup::get_job,
        crate::routes::backup::list_archive_exports,
        crate::routes::backup::list_destinations,
        crate::routes::backup::list_jobs,
        crate::routes::backup::list_policies,
        crate::routes::backup::test_destination,
        crate::routes::backup::trigger_policy,
        crate::routes::backup::update_destination,
        crate::routes::backup::update_policy,
        crate::routes::camera_config::bulk_config,
        crate::routes::camera_config::ensure_onvif_user,
        crate::routes::camera_config::get_device_info,
        crate::routes::camera_config::get_ntp,
        crate::routes::camera_config::get_onvif_settings,
        crate::routes::camera_config::get_osd,
        crate::routes::camera_config::get_time,
        crate::routes::camera_config::get_video,
        crate::routes::camera_config::get_video_list,
        crate::routes::camera_config::put_ntp,
        crate::routes::camera_config::put_onvif_settings,
        crate::routes::camera_config::put_osd,
        crate::routes::camera_config::put_time,
        crate::routes::camera_config::put_video,
        crate::routes::camera_config::reboot,
        crate::routes::camera_config::sync_now,
        crate::routes::camera_control::get_capabilities,
        crate::routes::camera_control::get_day_night,
        crate::routes::camera_control::get_image,
        crate::routes::camera_control::get_intrusion,
        crate::routes::camera_control::get_line_crossing,
        crate::routes::camera_control::get_motion,
        crate::routes::camera_control::list_outputs,
        crate::routes::camera_control::probe,
        crate::routes::camera_control::pulse_output,
        crate::routes::camera_control::put_day_night,
        crate::routes::camera_control::put_detection,
        crate::routes::camera_control::put_image,
        crate::routes::camera_control::put_intrusion,
        crate::routes::camera_control::put_line_crossing,
        crate::routes::camera_control::put_motion,
        crate::routes::onvif::continuous_move,
        crate::routes::onvif::discover,
        crate::routes::onvif::get_onvif,
        crate::routes::onvif::goto_preset,
        crate::routes::onvif::list_presets,
        crate::routes::onvif::probe,
        crate::routes::onvif::ptz_stop,
        crate::routes::onvif::refresh_presets,
        crate::routes::system::get_db_status,
        crate::routes::system::get_retention,
        crate::routes::system::get_transcode,
        crate::routes::system::post_db_convert,
        crate::routes::system::put_db_limit,
        crate::routes::system::put_retention,
        crate::routes::system::put_transcode,
        crate::routes::system::system_info,
        crate::routes::webhooks::create,
        crate::routes::webhooks::delete,
        crate::routes::webhooks::event_types,
        crate::routes::webhooks::list,
        crate::routes::webhooks::list_deliveries,
        crate::routes::webhooks::test,
        crate::routes::webhooks::update,
        crate::routes::zones::create_zone,
        crate::routes::zones::delete_zone,
        crate::routes::zones::list_zone_events,
        crate::routes::zones::list_zones,
        crate::routes::zones::update_zone,
        crate::routes::zones::zone_event_aggregates,
        crate::routes::zones::zone_occupancy,
    ),
    components(schemas(ErrorBody, Coordinate, crate::models::CameraView)),
    tags(
        (name = "cameras", description = "Camera registry. Every route here is camera-scoped."),
        (name = "system", description = "Box-wide operational settings. Fleet-wide by nature: a \
         camera-scoped credential cannot change them."),
        (name = "sites", description = "Sites and their timezones. A site's zone is what its \
         cameras' schedules and searches are read in, so changing it moves recording windows."),
        (name = "recordings", description = "Recorded video: segments, timeline, gaps, clips, \
         snapshots and HLS playback sessions. Every route is camera-scoped."),
        (name = "evidence", description = "Signed, offline-verifiable evidence bundles (#118). \
         Every route is camera-scoped, and gated on `video:export` except the public signing key."),
    )
)]
pub struct ApiDoc;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/openapi.json", get(spec))
}

/// This document.
///
/// Inside the `/api/v1` auth floor deliberately: the surface map is not something to hand to an
/// unauthenticated caller. Publishing it for codegen is the release artifact's job, not this
/// endpoint's — and note this serves the KERNEL's fragment, while the composed document including
/// the app crates is what `heldar_server::api_document()` builds.
#[utoipa::path(
    get, path = "/api/v1/openapi.json", tag = "system",
    operation_id = "getOpenApiDocument",
    responses((status = 200, description = "The OpenAPI 3.1 document for this build")),
)]
pub async fn spec() -> Json<serde_json::Value> {
    Json(document())
}

/// The kernel's own fragment, for a composer that adds the app crates' routes.
///
/// The app crates depend on the kernel, so the kernel cannot name their handlers — the composed
/// document has to be assembled one layer up, exactly as `/metrics` is. `document()` below returns
/// the KERNEL's routes only; `heldar_server::api_document()` is the whole surface.
pub fn kernel_fragment() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}

/// Merge extra fragments into the kernel's document and decorate the result.
pub fn document_with(extra: &[utoipa::openapi::OpenApi]) -> serde_json::Value {
    let mut doc = ApiDoc::openapi();
    for e in extra {
        doc.merge(e.clone());
    }
    let mut spec = serde_json::to_value(doc).unwrap_or_else(|_| serde_json::json!({}));
    crate::openapi_security::decorate(&mut spec);
    spec
}

/// The served document: generated from the handlers, then decorated with what each route REQUIRES.
///
/// The decoration is a post-pass over one table (`openapi_security::REQUIREMENTS`) rather than
/// attributes scattered across handlers, so a test can iterate it and drive the real router to check
/// every claim. A statement sitting beside the code agrees with it right up until someone changes
/// one and not the other — which is exactly how this module's own error-code list drifted.
pub fn document() -> serde_json::Value {
    let mut spec =
        serde_json::to_value(ApiDoc::openapi()).unwrap_or_else(|_| serde_json::json!({}));
    crate::openapi_security::decorate(&mut spec);
    spec
}
