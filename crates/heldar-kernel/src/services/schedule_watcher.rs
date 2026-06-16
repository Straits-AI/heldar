//! Recording-schedule watcher. On each tick it asks the recorder to reconcile the time-based
//! cameras (`scheduled` / `scheduled_event`) whose recording state must change because a window just
//! opened or closed — so a recorder starts when the window opens and stops when it closes, without
//! waiting for an external API call. Continuous cameras are untouched (their reconcile is event-driven
//! on config change), and a camera already in the correct state is never restarted mid-window.
//!
//! Schedule windows are evaluated against the SERVER's LOCAL timezone (chrono::Local); there is no
//! per-camera timezone. Spawned (supervised) from `main` only when the recorder is enabled.

use std::time::Duration;

use crate::state::AppState;

pub async fn run(state: AppState) {
    let interval_s = state.cfg.schedule_check_interval_s.max(5);
    let mut tick = tokio::time::interval(Duration::from_secs(interval_s));
    loop {
        tick.tick().await;
        state.recorder.reconcile_schedules().await;
    }
}
