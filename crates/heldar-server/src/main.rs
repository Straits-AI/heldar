//! Heldar composing server — links the open kernel with the open generic apps (access control,
//! movement, search), plus optional proprietary verticals (e.g. bakery) behind the `verticals`
//! feature.
//!
//! Boots the SQLite store + kernel migrations, applies each app's schema, registers their perception
//! consumers and routers, starts the recorder/sampler supervisors + background services (indexer,
//! health, retention, app retention, webhooks), and serves the HTTP API + recorded media. The OPEN
//! reference build (`--no-default-features`) composes only kernel + the Apache-2.0 generic apps; a
//! different deployment links a different set of app crates here.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

// The composing binary links the kernel library and (later) the app crates.
use heldar_kernel::config::Config;
use heldar_kernel::services::recorder::RecorderManager;
use heldar_kernel::services::sampler::SamplerManager;
use heldar_kernel::state::AppState;
use heldar_kernel::{auth, db, routes, services};

// Proprietary vertical composition is isolated behind this seam. In the OPEN repo `verticals.rs` is a
// no-op stub; the private workspace ships the real module (the proprietary verticals). Keeps main.rs identical
// and free of any proprietary-crate reference in the open build.
mod verticals;
// Sandboxed Wasm plugin composition, isolated behind the `wasm` feature seam (no-op stub when off).
mod wasm_plugins;

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
        &cfg.playback_dir,
        &cfg.archive_dir,
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
    // Dual/mirror recorder: present only when HELDAR_MIRROR_RECORDINGS_DIR is configured.
    let mirror = match &cfg.mirror_recordings_dir {
        Some(dir) => {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("creating mirror recordings dir {}", dir.display()))?;
            Some(services::mirror::MirrorRecorderManager::new(
                pool.clone(),
                cfg.clone(),
                dir.clone(),
            ))
        }
        None => None,
    };
    let sampler = SamplerManager::new(pool.clone(), cfg.clone());
    // Register the perception consumers (zones = kernel-open spatial primitive; ANPR = the open
    // access-control app). The kernel ingest path fans batches out to these without naming them —
    // adding an app is a push here, not an edit to the ingest handler.
    // Each app loads its own config from the environment (the kernel carries none).
    let entry_cfg = Arc::new(heldar_entry::config::EntryConfig::from_env());
    let movement_cfg = Arc::new(heldar_movement::config::MovementConfig::from_env());
    let search_cfg = Arc::new(heldar_search::config::SearchConfig::from_env());
    use services::consumer::DetectionConsumer;
    let mut consumers: Vec<Arc<dyn DetectionConsumer>> = vec![
        // Zone engine = kernel-open spatial primitive. Holds the recorder so a committed zone event
        // triggers event-mode recording (recorder is created above, before its consumers).
        services::zones::ZoneEngine::new(pool.clone(), cfg.clone(), recorder.clone()),
        // Access-control ANPR engine (open generic app), registered as a consumer over the seam.
        heldar_entry::anpr::AnprEngine::new(pool.clone(), cfg.clone(), entry_cfg.clone()),
    ];
    // Module manifests, composed here so GET /api/v1/modules reflects exactly what this binary links.
    // The open generic apps register first; proprietary verticals add theirs via the seam (empty in the
    // open build). The dashboard renders its nav + routes from this list.
    let mut modules = vec![
        heldar_entry::manifest(),
        heldar_movement::manifest(),
        heldar_search::manifest(),
    ];
    modules.extend(verticals::manifests());
    // Sandboxed Wasm plugins register as additional consumers + headless modules via the seam (a no-op
    // when the `wasm` feature is off). Pass the already-composed ids so a plugin can't collide with a
    // compiled/vertical module id (which would duplicate it in GET /api/v1/modules).
    let reserved_ids: Vec<String> = modules.iter().map(|m| m.id.clone()).collect();
    let (wasm_consumers, wasm_modules) = wasm_plugins::load(&pool, &cfg.data_dir, &reserved_ids);
    consumers.extend(wasm_consumers);
    modules.extend(wasm_modules);
    let consumers: Arc<Vec<Arc<dyn DetectionConsumer>>> = Arc::new(consumers);
    let modules = Arc::new(modules);
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("building http client")?;
    // Plugin store catalog engine (bundled + signed remote registries).
    let catalog = Arc::new(services::registry::CatalogService::new(&cfg));
    // Kept for the durable fan-out drainer (consumers is moved into AppState below).
    let drain_consumers = Arc::clone(&consumers);
    let state = AppState {
        pool: pool.clone(),
        cfg: cfg.clone(),
        recorder: recorder.clone(),
        mirror: mirror.clone(),
        sampler: sampler.clone(),
        consumers,
        modules,
        catalog: catalog.clone(),
        http,
        started_at: chrono::Utc::now(),
    };

    recorder.start_all().await.context("starting recorders")?;
    if let Some(m) = &mirror {
        m.start_all().await.context("starting mirror recorders")?;
    }
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
        // Durable perception fan-out: replays detection batches whose consumer fan-out didn't
        // complete before a crash (idempotent via consumer_fanout).
        let (p, cons) = (pool.clone(), drain_consumers.clone());
        spawn_supervised("fanout_drain", move || {
            services::fanout::run(p.clone(), cons.clone())
        });
        // Bundled domain apps run their own retention loops (they own their data lifecycle).
        let (p, c, e) = (pool.clone(), cfg.clone(), entry_cfg.clone());
        spawn_supervised("entry_retention", move || {
            heldar_entry::retention::run(p.clone(), c.clone(), e.clone())
        });
        // Forensic-search prunes its own query log.
        let (p, c, s) = (pool.clone(), cfg.clone(), search_cfg.clone());
        spawn_supervised("search_retention", move || {
            heldar_search::retention::run(p.clone(), c.clone(), s.clone())
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
        // Scheduled interval snapshots (kernel platform feature): supervise only when enabled.
        if cfg.snapshot_scheduler_enabled {
            let st = state.clone();
            spawn_supervised("snapshot_scheduler", move || {
                services::snapshot_scheduler::run(st.clone())
            });
        }
        // ANR edge re-fill: re-fetch missed footage from camera onboard storage to fill recording
        // gaps. Only supervise when enabled — run() returns immediately otherwise (avoids respawn
        // churn, mirroring the backup-scheduler guard).
        if cfg.anr_enabled {
            let (p, c) = (pool.clone(), cfg.clone());
            spawn_supervised("anr", move || services::anr::run(p.clone(), c.clone()));
        }
        // Recording-schedule watcher (opens/closes time-of-day windows for scheduled cameras).
        // Only meaningful when the recorder is enabled; the watcher itself also self-guards.
        if cfg.recorder_enabled {
            let st = state.clone();
            spawn_supervised("schedule_watcher", move || {
                services::schedule_watcher::run(st.clone())
            });
        }
        // Playback-session cleanup: removes expired HLS playback dirs and releases their read-locks.
        {
            let st = state.clone();
            spawn_supervised("playback_session_cleanup", move || {
                services::playback_session::run(st.clone())
            });
        }
        // Backup scheduler (scheduled policy jobs). Only supervise when enabled — run() returns
        // immediately otherwise, which would respawn it in a tight loop.
        if cfg.backup_enabled {
            let st = state.clone();
            spawn_supervised("backup_scheduler", move || {
                services::backup::run(st.clone())
            });
        }
        // The webhook delivery engine is spawned UNCONDITIONALLY: it is the single deliverer of events
        // to external systems (superseding the old single-URL notifier). Subscriptions are managed at
        // runtime via the API and re-read every cycle, so it self-idles when none are enabled and never
        // returns — there is no tight-loop respawn concern.
        {
            let (p, c) = (pool.clone(), cfg.clone());
            spawn_supervised("webhooks", move || {
                services::webhooks::run(p.clone(), c.clone())
            });
        }
        // Sidecar module health: probe each registered plugin's /heldar/health so the dashboard can
        // badge healthy/unreachable. Self-idles when none are registered; never returns.
        {
            let p = pool.clone();
            spawn_supervised("module_health", move || services::modules::run(p.clone()));
        }
        // Plugin registry refresh: re-fetch + verify remote signed catalogs on a cadence. Parks when
        // the registry is disabled or no URLs are configured (the bundled catalog needs no refresh).
        {
            let c = catalog.clone();
            spawn_supervised("registry_refresh", move || {
                services::registry::run(c.clone())
            });
        }
        // Edge-side fleet self-registration: POST this node's identity to the control plane on boot +
        // heartbeat so it joins the fleet without static config. Parks unless HELDAR_CP_URL +
        // HELDAR_SITE_ID + HELDAR_PUBLIC_BASE_URL are all set (the fleet is opt-in).
        {
            let c = cfg.clone();
            spawn_supervised("fleet_register", move || {
                services::fleet_register::run(c.clone())
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
    // Recorded media (/media/*) is the same sensitive footage the API gates — so guard it with the
    // SAME auth when enabled. The browser sends the session cookie with <img>/<video> requests, so the
    // dashboard keeps working; an unauthenticated client gets 401. No-op when auth is disabled.
    let media = Router::new()
        .nest_service("/media/recordings", ServeDir::new(&cfg.recordings_dir))
        .nest_service("/media/clips", ServeDir::new(&cfg.clips_dir))
        .nest_service("/media/snapshots", ServeDir::new(&cfg.snapshots_dir))
        .nest_service("/media/playback", ServeDir::new(&cfg.playback_dir))
        .nest_service("/media/archives", ServeDir::new(&cfg.archive_dir))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            media_guard,
        ));
    let app = verticals::merge_routes(app).merge(media);

    // Serve the built dashboard so the whole product is ONE binary at ONE URL. The /api/*, /media/*,
    // /healthz, /readyz and /metrics routes above are explicit and take precedence; the SPA is only
    // a fallback for everything else. Unknown (client-routed) paths fall back to index.html so deep
    // links work. We use ServeDir::fallback (not not_found_service) so those deep links return 200 —
    // not_found_service wraps the file in SetStatus(404), which would mark every valid client route
    // as a 404 (breaking caching, prefetch and uptime checks). When no web_dir exists: API-only.
    let app = match &cfg.web_dir {
        Some(dir) => {
            tracing::info!("serving dashboard from {}", dir.display());
            app.fallback_service(
                ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html"))),
            )
        }
        None => {
            tracing::info!("no web_dir; dashboard not served");
            app
        }
    };

    let app = app
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state);

    let addr = format!("{}:{}", cfg.api_host, cfg.api_port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!("Heldar Core listening on http://{addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(
            recorder.clone(),
            mirror.clone(),
            sampler.clone(),
        ))
        .await
        .context("server error")?;

    Ok(())
}

/// Auth guard for the recorded-media plane. When auth is enabled, requires a valid principal (resolved
/// exactly like the API via the `Principal` extractor — cookie / Bearer / API key); 401 otherwise. A
/// no-op pass-through when auth is disabled (the LAN-appliance default).
async fn media_guard(State(st): State<AppState>, req: Request, next: Next) -> Response {
    if !st.cfg.auth_enabled {
        return next.run(req).await;
    }
    let (mut parts, body) = req.into_parts();
    match auth::Principal::from_request_parts(&mut parts, &st).await {
        Ok(_) => next.run(Request::from_parts(parts, body)).await,
        Err(_) => StatusCode::UNAUTHORIZED.into_response(),
    }
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

async fn shutdown_signal(
    recorder: Arc<RecorderManager>,
    mirror: Option<Arc<heldar_kernel::services::mirror::MirrorRecorderManager>>,
    sampler: Arc<SamplerManager>,
) {
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
    if let Some(m) = &mirror {
        m.shutdown().await;
    }
    sampler.shutdown().await;
}
