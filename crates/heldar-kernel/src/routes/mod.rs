use axum::Router;

use crate::state::AppState;

pub mod ai;
pub mod anr;
pub mod auth;
pub mod backup;
pub mod cameras;
pub mod discovery;
pub mod health;
pub mod incidents;
pub mod liveview;
pub mod metrics;
pub mod onvif;
pub mod outbox;
pub mod playback;
pub mod playback_sessions;
pub mod recording_control;
pub mod recordings;
pub mod schedules;
pub mod snapshot_schedules;
pub mod system;
pub mod zones;

/// Assemble the kernel API router (absolute paths, mounted at root by the composing server). The
/// auth admin surface stays here for now; domain apps merge their own routers in
/// the server binary.
pub fn api_router() -> Router<AppState> {
    Router::new()
        .merge(system::router())
        .merge(cameras::router())
        .merge(recordings::router())
        .merge(anr::router())
        .merge(recording_control::router())
        .merge(playback::router())
        .merge(playback_sessions::router())
        .merge(liveview::router())
        .merge(health::router())
        .merge(discovery::router())
        .merge(ai::router())
        .merge(zones::router())
        .merge(schedules::router())
        .merge(snapshot_schedules::router())
        .merge(incidents::router())
        .merge(backup::router())
        .merge(onvif::router())
        .merge(outbox::router())
        .merge(auth::router())
}
