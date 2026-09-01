//! This crate's fragment of the API contract (#120).
//!
//! The kernel cannot name these handlers — this crate depends on it, not the other way — so
//! the composed document is assembled in `heldar-server`, which depends on both. Serving a
//! partial document from the kernel would be worse than serving none: an integrator would
//! reasonably read a missing route as a route that does not exist.

use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(paths(
    crate::routes::plan_only,
    crate::routes::search_events,
    crate::routes::search_nl,
    crate::routes::search_semantic_scoped,
    crate::routes::serve_ui,
))]
pub struct ApiDoc;

/// This crate's routes, for the composer.
pub fn fragment() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}
