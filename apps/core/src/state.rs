use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::config::Config;
use crate::services::recorder::RecorderManager;

/// Shared application state, cloned cheaply into every handler and background task.
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub cfg: Arc<Config>,
    pub recorder: Arc<RecorderManager>,
    pub http: reqwest::Client,
    pub started_at: DateTime<Utc>,
}
