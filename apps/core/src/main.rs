//! VisionOps Core — Stage 0 media kernel control plane.
//!
//! Boots the SQLite store, starts a recorder supervisor per camera, runs the timeline indexer,
//! health monitor and retention sweeper, and serves the HTTP API + recorded media.

mod camera_url;
mod config;
mod db;
mod error;
mod models;
mod repo;
mod routes;
mod services;
mod state;
mod util;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::http::HeaderValue;
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::config::Config;
use crate::services::recorder::RecorderManager;
use crate::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();

    let cfg = Arc::new(Config::from_env());
    for dir in [
        &cfg.data_dir,
        &cfg.recordings_dir,
        &cfg.clips_dir,
        &cfg.snapshots_dir,
    ] {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }

    let pool = db::init_pool(&cfg).await.context("init database pool")?;
    db::run_migrations(&pool).await.context("run migrations")?;

    let recorder = RecorderManager::new(pool.clone(), cfg.clone());
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("building http client")?;
    let state = AppState {
        pool: pool.clone(),
        cfg: cfg.clone(),
        recorder: recorder.clone(),
        http,
        started_at: chrono::Utc::now(),
    };

    recorder.start_all().await.context("starting recorders")?;
    tokio::spawn(services::indexer::run(pool.clone(), cfg.clone()));
    tokio::spawn(services::health::run(pool.clone(), cfg.clone()));
    tokio::spawn(services::retention::run(pool.clone(), cfg.clone()));

    // Allow all origins if configured with "*" or left empty; otherwise restrict to the list.
    let allow_all = cfg.cors_origins.is_empty() || cfg.cors_origins.iter().any(|o| o == "*");
    let cors = if allow_all {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    } else {
        let origins: Vec<HeaderValue> = cfg
            .cors_origins
            .iter()
            .filter_map(|o| o.parse::<HeaderValue>().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods(Any)
            .allow_headers(Any)
    };

    let app = Router::new()
        .merge(routes::api_router())
        .nest_service("/media/recordings", ServeDir::new(&cfg.recordings_dir))
        .nest_service("/media/clips", ServeDir::new(&cfg.clips_dir))
        .nest_service("/media/snapshots", ServeDir::new(&cfg.snapshots_dir))
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state);

    let addr = format!("{}:{}", cfg.api_host, cfg.api_port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!("VisionOps Core listening on http://{addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(recorder.clone()))
        .await
        .context("server error")?;

    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_env("VISIONOPS_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,visionops_core=debug"));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
}

async fn shutdown_signal(recorder: Arc<RecorderManager>) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut s) = signal(SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received; stopping recorders");
    recorder.shutdown().await;
}
