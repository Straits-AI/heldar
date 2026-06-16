---
id: architecture
title: Architecture
sidebar_label: Architecture
sidebar_position: 1
---

# Architecture

Heldar is a thin HTTP control plane (Axum routes) over a set of long-running
background services, all sharing one SQLite store and one config. The kernel
(`heldar-kernel`) is **domain-agnostic**: it manages cameras, ingests and
records RTSP, samples frames for AI, accepts detections from workers, evaluates
spatial zones, and provides auth, retention, and observability. It knows nothing
about access control, movement intelligence, or search; those are apps.

Two rules shape the whole system:

- **The kernel is the only thing that talks to cameras.** The 24/7 recorder
  copies the compressed bitstream to disk with no decode. A budgeted sampler is
  the only component that decodes, writing one current JPEG per camera. A slow
  or absent AI worker can never stall ingest or recording.
- **The kernel has no dependency on any app.** Apps depend on the kernel and are
  linked in by a composing binary. Adding an app is a push at a few composition
  points, never an edit to the kernel.

For the full per-stage design (recorder supervisor, indexer, retention sweeper,
zone engine, metrics, and more), see
[ARCHITECTURE.md](https://github.com/Straits-AI/heldar/blob/main/ARCHITECTURE.md)
in the repo.

## The four public seams

Apps plug into the kernel only through these seams. Together they let a new app
add tables, routes, perception logic, and authorization without the kernel ever
naming it.

### 1. The `DetectionConsumer` trait

After a batch of worker detections is persisted, the kernel ingest path fans it
out to a registry of consumers. A consumer self-declares which `task_type`s it
cares about, so the kernel never grows an `if task_type == "..."` branch.

```rust
pub struct DetectionBatch<'a> {
    pub camera_id: &'a str,
    pub site_id: Option<&'a str>,
    pub task_type: &'a str,
    pub detections: &'a [DetectionIngest],
    pub timestamp: DateTime<Utc>,
}

#[async_trait::async_trait]
pub trait DetectionConsumer: Send + Sync {
    fn name(&self) -> &'static str;
    fn interested_in(&self, task_type: &str) -> bool;
    async fn consume(&self, batch: &DetectionBatch<'_>);
}
```

The open zone engine (a spatial primitive) and the open access-control engine
(plate authorization) are both consumers. The zone engine returns `true` for any
task type with tracked detections; the access-control engine returns `true` only
for `anpr`.

### 2. `Router<AppState>` merge

Each app exposes its own Axum `Router<AppState>` with absolute `/api/v1/...`
paths. The composing server merges those routers next to the kernel router; the
kernel router is unaware of them. From `AppState` an app handler reaches the
shared SQLite pool, the kernel config, and the recorder/sampler/HTTP client.

### 3. Self-installed schema

Each app owns its own tables and applies its schema idempotently against the
shared pool at startup (single-tenant per deployment). The kernel does not define
domain tables. The pattern is a `schema::init(pool)` that runs an `include_str!`
of a `schema.sql` full of `CREATE TABLE IF NOT EXISTS`.

### 4. The auth primitive

The kernel provides a `Principal` extractor plus RBAC capability checks
(`can_view`, `can_manage_registry`, `can_operate_gate`, and so on) and an audit
helper. Apps reuse these for authorization and audit instead of inventing their
own, so a single `HELDAR_AUTH_ENABLED` toggle governs the whole composed surface.

## How a deployment is composed

The composing server (`heldar-server`) is where the kernel and a chosen set of
apps come together. For each app it: applies the app schema, constructs the app's
detection consumers and adds them to the consumer vector in `AppState`, merges
the app router, and spawns any background loops under a supervisor that respawns
on panic. The open reference build composes only the kernel plus the Apache-2.0
generic apps; a different deployment links a different set of crates here.

Not every app is a `DetectionConsumer`. Some apps are periodic background loops
or read-only query layers over already-stored kernel facts; they use the same
schema + router + loop composition without sitting on the ingest hot path.

See [Build a module](../develop/build-a-module.md) for a step-by-step walkthrough
of writing one, and [Open-core](./open-core.md) for the open versus proprietary
boundary.
