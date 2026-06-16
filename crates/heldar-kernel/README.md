# heldar-kernel

*The open, domain-agnostic Heldar platform: media/DVR control plane, perception ingest + sampler, zone engine, auth/RBAC, and the worker SDK contract.*

[![crates.io](https://img.shields.io/crates/v/heldar-kernel.svg)](https://crates.io/crates/heldar-kernel)
[![docs.rs](https://docs.rs/heldar-kernel/badge.svg)](https://docs.rs/heldar-kernel)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/Straits-AI/heldar/blob/main/LICENSE)

`heldar-kernel` is the open (Apache-2.0) foundation of the **Heldar** open-core platform. It owns the media kernel (camera registry, RTSP ingest, recording, timeline, playback, live view) and the seams that turn camera streams into structured events, so domain apps never touch codecs or the ingest path. Every other Heldar crate links this one and plugs in as a route module and/or a perception consumer; the kernel itself has no dependency on any app.

## What it does

- **Media / DVR control plane.** Camera registry (`/api/v1/cameras`), RTSP segment recording via `RecorderManager` (FFmpeg stream-copy), a timeline indexer, clip and snapshot export, and recorded-media serving. Camera RTSP URLs are built from vendor templates (`camera_url`) so onboarding supplies only address + credentials.
- **Live view brokering.** Streams are proxied through MediaMTX (`services::mediamtx`, `/api/v1/cameras/{id}/liveview`) so camera credentials never reach the browser.
- **Perception ingest + sampler.** A bounded, isolated frame sampler (`SamplerManager`, global fps budget) feeds a pluggable worker; workers post detections back over the SDK contract (`/api/v1/ai/events`, `models::DetectionIngest`). Per-camera AI tasks are managed under `/api/v1/cameras/{id}/ai-tasks` (and `/api/v1/ai-tasks/{task_id}`); worker task discovery and sampler status are exposed at `/api/v1/ai/tasks` and `/api/v1/ai/samplers`.
- **DetectionConsumer seam.** Committed, persisted detection batches fan out to a registry of `DetectionConsumer`s that self-select by `task_type`. Apps are *added* to the registry; the ingest handler never grows an `if task_type == ...` branch.
- **Zone engine.** Polygon zones (`/api/v1/cameras/{id}/zones`) and enter/exit/dwell events (`/api/v1/cameras/{id}/zone-events`), shipped as the kernel-open `ZoneEngine` consumer.
- **Auth / RBAC.** Opt-in via `HELDAR_AUTH_ENABLED`; argon2id password hashing, 256-bit tokens stored only as SHA-256, an HttpOnly `SameSite=Strict` session cookie plus `X-API-Key`, five roles, a `Principal` request extractor, and an immutable audit log.
- **Observability.** Prometheus `/metrics`, `/healthz` + `/readyz`, per-camera health (`/api/v1/health/cameras`), a generic event stream (`/api/v1/events`), recording-gap tracking (`/api/v1/cameras/{id}/gaps`), and an optional alert webhook notifier.
- **Retention.** Size, age, and free-disk pruning of recording segments plus detection/audit row retention.
- **Remote-access awareness.** Probes the configured WireGuard-overlay interface and reports remote-access health (`OverlayStatus`) via `/api/v1/system`; the kernel observes but does not manage the overlay.
- **Self-installing store.** SQLite with WAL and embedded migrations (`db::run_migrations`), plus crash-safe segment read-lock recovery on startup.

## Where it fits

`heldar-kernel` is a **library** crate, not a runnable binary. The `heldar-core` server binary that composes the kernel with the generic apps, registers consumers, starts the background supervisors, and serves the HTTP API lives in the un-published `heldar-server` crate (`crates/heldar-server/src/main.rs`).

Key public surface (see `src/lib.rs`):

- `state::AppState`: shared, cheaply-cloned handler state (pool, config, recorder, sampler, consumers, http client).
- `routes::api_router() -> Router<AppState>` and `routes::metrics::router()`: the kernel API, mounted at root; apps merge their own `Router<AppState>` alongside.
- `services::consumer::{DetectionConsumer, DetectionBatch}`: the perception-consumer seam.
- `services::recorder::RecorderManager`, `services::sampler::SamplerManager`, `services::zones::ZoneEngine`.
- `config::Config` (loaded from `HELDAR_*` env), `db` (`init_pool`, `run_migrations`), and `auth` (`Principal`, `Role`, `ensure_bootstrap`, `audit`).

## Usage

```bash
cargo add heldar-kernel
```

A composing server wires the kernel together. Condensed from `heldar-server/src/main.rs`:

```rust
use std::sync::Arc;
use heldar_kernel::services::consumer::DetectionConsumer;
use heldar_kernel::services::recorder::RecorderManager;
use heldar_kernel::services::sampler::SamplerManager;
use heldar_kernel::{config::Config, db, routes, services, state::AppState};

let cfg = Arc::new(Config::from_env());
let pool = db::init_pool(&cfg).await?;
db::run_migrations(&pool).await?;

let recorder = RecorderManager::new(pool.clone(), cfg.clone());
let sampler = SamplerManager::new(pool.clone(), cfg.clone());

// Register perception consumers over the kernel's DetectionConsumer seam.
let consumers: Arc<Vec<Arc<dyn DetectionConsumer>>> =
    Arc::new(vec![services::zones::ZoneEngine::new(pool.clone(), cfg.clone())]);

let state = AppState {
    pool: pool.clone(),
    cfg: cfg.clone(),
    recorder,
    sampler,
    consumers,
    // Module manifests, served at GET /api/v1/modules so the dashboard builds nav + routes
    // from live truth. The composing binary pushes each linked app's manifest() here.
    modules: Arc::new(vec![]),
    // Plugin store catalog engine (bundled + signed remote registries), GET /api/v1/registry.
    catalog: Arc::new(services::registry::CatalogService::new(&cfg)),
    http: reqwest::Client::new(),
    started_at: chrono::Utc::now(),
};

// Mount the kernel API; domain apps merge their own Router<AppState> here.
let app = routes::api_router()
    .merge(routes::metrics::router())
    .with_state(state);
```

The full binary also creates data directories, starts the recorder/sampler supervisors and background services, and serves recorded media. See `crates/heldar-server/src/main.rs` for the complete composition.

## Documentation

- Repository: <https://github.com/Straits-AI/heldar>
- Architecture and the kernel seams: [`ARCHITECTURE.md`](https://github.com/Straits-AI/heldar/blob/main/ARCHITECTURE.md)
- Operations: [`docs/OBSERVABILITY.md`](https://github.com/Straits-AI/heldar/blob/main/docs/OBSERVABILITY.md) and [`docs/REMOTE-ACCESS.md`](https://github.com/Straits-AI/heldar/blob/main/docs/REMOTE-ACCESS.md)
- API docs: <https://docs.rs/heldar-kernel>

## License

Apache-2.0.
