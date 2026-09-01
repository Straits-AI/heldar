//! This crate's fragment of the API contract (#120).
//!
//! The kernel cannot name these handlers — this crate depends on it, not the other way — so
//! the composed document is assembled in `heldar-server`, which depends on both. Serving a
//! partial document from the kernel would be worse than serving none: an integrator would
//! reasonably read a missing route as a route that does not exist.

use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(paths(
    crate::routes::ack_breach,
    crate::routes::confirm_candidate,
    crate::routes::create_link,
    crate::routes::delete_link,
    crate::routes::list_breaches,
    crate::routes::list_candidates,
    crate::routes::list_links,
    crate::routes::reject_candidate,
    crate::routes::resolve_breach,
    crate::routes::search_person,
    crate::routes::search_plate,
    crate::routes::serve_ui,
    crate::routes::trigger_run,
))]
pub struct ApiDoc;

/// This crate's routes, for the composer.
pub fn fragment() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}
