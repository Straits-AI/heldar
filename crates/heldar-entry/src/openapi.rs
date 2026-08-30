//! This crate's fragment of the API contract (#120).
//!
//! The kernel cannot name these handlers — this crate depends on it, not the other way — so
//! the composed document is assembled in `heldar-server`, which depends on both. Serving a
//! partial document from the kernel would be worse than serving none: an integrator would
//! reasonably read a missing route as a route that does not exist.

use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(paths(
    crate::routes::checkin_pass,
    crate::routes::checkout_pass,
    crate::routes::confirm_event,
    crate::routes::create_pass,
    crate::routes::create_vehicle,
    crate::routes::create_watch,
    crate::routes::delete_gate_policy,
    crate::routes::delete_pass,
    crate::routes::delete_vehicle,
    crate::routes::delete_watch,
    crate::routes::gate_open,
    crate::routes::get_entry_event,
    crate::routes::get_gate_state,
    crate::routes::get_pass,
    crate::routes::get_vehicle,
    crate::routes::list_audit,
    crate::routes::list_entry_events,
    crate::routes::list_passes,
    crate::routes::list_vehicles,
    crate::routes::list_watchlist,
    crate::routes::put_gate_policy,
    crate::routes::put_gate_settings,
    crate::routes::reject_event,
    crate::routes::report_entry_log,
    crate::routes::report_exceptions,
    crate::routes::serve_ui,
    crate::routes::update_pass,
    crate::routes::update_vehicle,
    crate::routes::update_watch,
))]
pub struct ApiDoc;

/// This crate's routes, for the composer.
pub fn fragment() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}
