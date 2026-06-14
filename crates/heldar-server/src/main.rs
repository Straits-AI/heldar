//! Heldar composing server — links the open kernel with the open generic apps (access control,
//! movement, search), plus optional proprietary verticals (e.g. bakery) behind the `verticals`
//! feature.
//!
//! Boots the SQLite store + kernel migrations, applies each app's schema, registers their perception
//! consumers and routers, starts the recorder/sampler supervisors + background services (indexer,
//! health, retention, app retention, notifier), and serves the HTTP API + recorded media. The OPEN
//! reference build (`--no-default-features`) composes only kernel + the Apache-2.0 generic apps; a
//! different deployment links a different set of app crates here.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::http::HeaderValue;
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

// The composing binary links the kernel library and (later) the app crates.
use heldar_kernel::config::Config;
use heldar_kernel::services::recorder::RecorderManager;
use heldar_kernel::services::sampler::SamplerManager;
use heldar_kernel::state::AppState;
use heldar_kernel::{auth, db, routes, services};

// Proprietary vertical composition is isolated behind this seam. In the OPEN repo `verticals.rs` is a
// no-op stub; the private workspace ships the real module (BakerySense, …). Keeps main.rs identical
// and free of any proprietary-crate reference in the open build.
mod verticals;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();

    let cfg = Arc::new(Config::from_env());
    // Fail fast if the media toolchain is missing — recording/clip/snapshot/sampling all need it.
    heldar_kernel::util::check_media_binaries(&cfg).context("media-binary preflight")?;
    for dir in [
        &cfg.data_dir,
        &cfg.recordings_dir,
        &cfg.clips_dir,
        &cfg.snapshots_dir,
        &cfg.frames_dir,
    ] {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }

    let pool = db::init_pool(&cfg).await.context("init database pool")?;
    db::run_migrations(&pool).await.context("run migrations")?;
    // Release any transient segment read-locks left by a crash mid clip/snapshot export.
    db::clear_segment_read_locks(&pool)
        .await
        .context("clear stale segment read-locks")?;
    // Composed apps apply their own schema (idempotently) against the shared pool.
    heldar_entry::schema::init(&pool)
        .await
        .context("entry schema init")?;
    heldar_movement::schema::init(&pool)
        .await
        .context("movement schema init")?;
    heldar_search::schema::init(&pool)
        .await
        .context("search schema init")?;
    // Proprietary verticals (isolated behind the `verticals` seam; a no-op in the open build).
    verticals::init_schema(&pool)
        .await
        .context("verticals schema init")?;
    auth::ensure_bootstrap(&pool, &cfg)
        .await
        .context("auth bootstrap")?;

    let recorder = RecorderManager::new(pool.clone(), cfg.clone());
    let sampler = SamplerManager::new(pool.clone(), cfg.clone());
    // Register the perception consumers (zones = kernel-open spatial primitive; ANPR = the open
    // access-control app). The kernel ingest path fans batches out to these without naming them —
    // adding an app is a push here, not an edit to the ingest handler.
    // Each app loads its own config from the environment (the kernel carries none).
    let entry_cfg = Arc::new(heldar_entry::config::EntryConfig::from_env());
    let movement_cfg = Arc::new(heldar_movement::config::MovementConfig::from_env());
    let search_cfg = Arc::new(heldar_search::config::SearchConfig::from_env());
    use services::consumer::DetectionConsumer;
    let consumers: Arc<Vec<Arc<dyn DetectionConsumer>>> = Arc::new(vec![
        // Zone engine = kernel-open spatial primitive.
        services::zones::ZoneEngine::new(pool.clone(), cfg.clone()),
        // Access-control ANPR engine (open generic app), registered as a consumer over the seam.
        heldar_entry::anpr::AnprEngine::new(pool.clone(), cfg.clone(), entry_cfg.clone()),
    ]);
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("building http client")?;
    let state = AppState {
        pool: pool.clone(),
        cfg: cfg.clone(),
        recorder: recorder.clone(),
        sampler: sampler.clone(),
        consumers,
        http,
        started_at: chrono::Utc::now(),
    };

    recorder.start_all().await.context("starting recorders")?;
    sampler.start_all().await;
    // Supervise the background services: if one panics, it is respawned (production resilience).
    {
        let (p, c) = (pool.clone(), cfg.clone());
        spawn_supervised("indexer", move || {
            services::indexer::run(p.clone(), c.clone())
        });
        let (p, c) = (pool.clone(), cfg.clone());
        spawn_supervised("health", move || {
            services::health::run(p.clone(), c.clone())
        });
        let (p, c) = (pool.clone(), cfg.clone());
        spawn_supervised("retention", move || {
            services::retention::run(p.clone(), c.clone())
        });
        // Bundled domain apps run their own retention loops (they own their data lifecycle).
        let (p, c, e) = (pool.clone(), cfg.clone(), entry_cfg.clone());
        spawn_supervised("entry_retention", move || {
            heldar_entry::retention::run(p.clone(), c.clone(), e.clone())
        });
        // Proprietary verticals start their own background loops (no-op in the open build).
        verticals::spawn_loops(&pool);
        // Movement: the ReID candidate proposer + the red-zone breach rule engine.
        let (p, m) = (pool.clone(), movement_cfg.clone());
        spawn_supervised("movement_reid", move || {
            heldar_movement::reid::run(p.clone(), m.clone())
        });
        let (p, m) = (pool.clone(), movement_cfg.clone());
        spawn_supervised("movement_breach", move || {
            heldar_movement::breach::run(p.clone(), m.clone())
        });
        // Only supervise the notifier when a webhook is configured — otherwise run() returns
        // immediately and the supervisor would respawn it in a tight loop.
        if cfg.alert_webhook_url.is_some() {
            let (p, c) = (pool.clone(), cfg.clone());
            spawn_supervised("notifier", move || {
                services::notifier::run(p.clone(), c.clone())
            });
        }
    }

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

    // Open generic apps merge their routers here; the kernel router is unaware of them.
    let app = Router::new()
        .merge(routes::api_router())
        .merge(routes::metrics::router())
        .merge(heldar_entry::routes::router())
        .merge(heldar_movement::routes::router(movement_cfg.clone()))
        .merge(heldar_search::routes::router(search_cfg.clone()));
    // Proprietary verticals merge their routers via the seam (a no-op in the open build).
    let app = verticals::merge_routes(app)
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
    tracing::info!("Heldar Core listening on http://{addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(recorder.clone(), sampler.clone()))
        .await
        .context("server error")?;

    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_env("HELDAR_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,heldar_core=debug"));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
}

/// Spawn a long-lived background task that is respawned (after a short delay) if it ever returns or
/// panics. The service `run` loops are not expected to return, so this is a resilience backstop.
fn spawn_supervised<F, Fut>(name: &'static str, make: F)
where
    F: Fn() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        loop {
            let handle = tokio::spawn(make());
            match handle.await {
                Ok(()) => {
                    tracing::error!(
                        task = name,
                        "background task returned unexpectedly; respawning in 5s"
                    )
                }
                Err(e) if e.is_panic() => {
                    tracing::error!(task = name, "background task panicked; respawning in 5s")
                }
                Err(_) => {
                    tracing::info!(task = name, "background task cancelled");
                    break;
                }
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

async fn shutdown_signal(recorder: Arc<RecorderManager>, sampler: Arc<SamplerManager>) {
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
    tracing::info!("shutdown signal received; stopping recorders + samplers");
    recorder.shutdown().await;
    sampler.shutdown().await;
}
