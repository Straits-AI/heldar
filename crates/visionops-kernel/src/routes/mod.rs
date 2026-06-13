use axum::Router;

use crate::state::AppState;

pub mod ai;
pub mod auth;
pub mod cameras;
pub mod discovery;
pub mod entry;
pub mod health;
pub mod liveview;
pub mod metrics;
pub mod playback;
pub mod recordings;
pub mod system;
pub mod zones;

/// Assemble the full API router (absolute paths, mounted at root by `main`).
pub fn api_router() -> Router<AppState> {
    Router::new()
        .merge(system::router())
        .merge(cameras::router())
        .merge(recordings::router())
        .merge(playback::router())
        .merge(liveview::router())
        .merge(health::router())
        .merge(discovery::router())
        .merge(ai::router())
        .merge(zones::router())
        .merge(auth::router())
        .merge(entry::router())
}
