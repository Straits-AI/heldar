# heldar-movement

*Generic cross-camera movement intelligence: multi-signal ReID candidates, movement trails, and red-zone breach alerts, built on heldar-kernel.*

[![crates.io](https://img.shields.io/crates/v/heldar-movement.svg)](https://crates.io/crates/heldar-movement)
[![docs.rs](https://docs.rs/heldar-movement/badge.svg)](https://docs.rs/heldar-movement)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/Straits-AI/heldar/blob/main/LICENSE)

`heldar-movement` is a domain-neutral correlation layer for the **Heldar** open-core visual
event-intelligence platform. It links the per-camera observations already written by the
[`heldar-kernel`](https://crates.io/crates/heldar-kernel) media/perception platform into cross-camera
journeys and flags restricted-zone breaches, under strict privacy gates. It is an analytics layer over
stored kernel data (`entry_events`, `detections`, `zone_events`, `zones`); it adds no ingest path and no
frame decode, and the kernel has no dependency on it.

## What it does

- **Vehicle ReID candidate proposer.** A background loop finds the same normalized plate on two
  operator-linked cameras within a plausible transit window, then fuses plate-exactness, transit-time
  plausibility, and vehicle attribute agreement (color/type) into a 0..1 score. Anchored on the plate,
  never a pure visual embedding.
- **Candidate matching, not identity.** Every cross-camera link is a scored *candidate* with per-signal
  evidence and a `pending` status. A human confirms or rejects it; nothing is asserted as legal identity.
- **Movement trails.** Resolves all appearances of a plate across cameras, time-ordered, bounded to the
  most recent appearances to stay memory-safe on heavily travelled plates.
- **Low-confidence person search.** Person ReID has no plate and no appearance embedding, so it is offered
  only as an on-demand candidate search over camera topology plus transit time, deliberately weak and for
  human triage.
- **Red-zone breach engine.** A second loop sweeps zone-enter events on operator-designated
  red/restricted zones, records each as a tracked incident (`open` to `acknowledged` to `resolved`), and
  best-effort correlates the subject's track id to a vehicle plate when one is available.
- **Operator-defined camera topology.** List, create, and delete edges in a directed camera-adjacency
  graph (with transit seconds and an optional bidirectional flag) that scopes all cross-camera matching.
- **Audited.** Every identity-like query (plate search, person search, plate-anchored candidate filter)
  writes a kernel audit-log entry. Reads require view, reviews require operate-gate, topology edits require
  manage.

## Where it fits

This is a **library** crate, not a runnable binary. The composing server (the un-published `heldar-server`
crate, which builds the `heldar-core` binary) applies its schema, loads its config, spawns its background
loops, and merges its router. The crate's public surface:

- `config::MovementConfig` and `MovementConfig::from_env()` — engine interval, scan window, minimum
  candidate score, red-zone kinds, retention days (all read from `HELDAR_MOVEMENT_*` env vars).
- `schema::init(&pool)` — installs its three tables (`camera_links`, `movement_candidates`,
  `breach_alerts`) idempotently against the shared kernel pool.
- `reid::run` / `reid::run_once` and `breach::run` / `breach::run_once` — the supervised proposer and
  breach-sweep loops, plus `reid::trail_for_plate`.
- `routes::router(cfg)` — an `axum::Router<AppState>` mounting the `/api/v1/movement/*` surface (topology,
  candidate review, breach workflow, audited search).
- `models::{CameraLink, CameraLinkCreate, MovementCandidate, BreachAlert}` — the serialized data models.

## Usage

```bash
cargo add heldar-movement
```

It is composed alongside the kernel like this (the real `heldar-server` wraps the two loops in a
panic-restarting supervisor):

```rust
use std::sync::Arc;
use heldar_movement::config::MovementConfig;

// 1. Install the schema against the shared kernel pool.
heldar_movement::schema::init(&pool).await?;

// 2. Load config from the environment (HELDAR_MOVEMENT_*).
let movement_cfg = Arc::new(MovementConfig::from_env());

// 3. Spawn the supervised background engines.
tokio::spawn(heldar_movement::reid::run(pool.clone(), movement_cfg.clone()));
tokio::spawn(heldar_movement::breach::run(pool.clone(), movement_cfg.clone()));

// 4. Merge the router into the kernel's Router<AppState>.
let app = app.merge(heldar_movement::routes::router(movement_cfg.clone()));
```

See `crates/heldar-server/src/main.rs` in the repository for the full composition, and
[`docs/MOVEMENT.md`](https://github.com/Straits-AI/heldar/blob/main/docs/MOVEMENT.md) for the operator and
integrator guide.

## Documentation

- Repository: <https://github.com/Straits-AI/heldar>
- Guide: [`docs/MOVEMENT.md`](https://github.com/Straits-AI/heldar/blob/main/docs/MOVEMENT.md)
- API docs: <https://docs.rs/heldar-movement>

## License

Apache-2.0. See [LICENSE](https://github.com/Straits-AI/heldar/blob/main/LICENSE).
