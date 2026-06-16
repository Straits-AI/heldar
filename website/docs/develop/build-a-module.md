---
id: build-a-module
title: Build a Module
sidebar_label: Build a Module
sidebar_position: 1
---

# Build a module

A module extends Heldar with its own tables, routes, UI, and perception logic. There are **two ways
to build one**, and you pick by how deeply you integrate and what language you work in:

- **Compiled-in app crate (this guide).** A Rust crate that depends on `heldar-kernel` and is linked
  by a composing binary. It shares the kernel's SQLite pool and rides the `DetectionConsumer` ingest
  seam — the tightest integration, for first-party apps. The open `heldar-entry`/`movement`/`search`
  crates are built this way.
- **Out-of-process sidecar plugin** ([separate guide](./sidecar-plugins.md)). Any HTTP service in any
  language that Heldar reverse-proxies and feeds events to, **installed at runtime** with no rebuild.
  Process/container-isolated, least-privilege. This is the path for third-party and self-made plugins,
  and what the **Plugins** page installs.

The rest of this page is the compiled-in path. An app adds tables, routes, and perception logic
without the kernel ever knowing it exists. You depend on `heldar-kernel`; a composing binary links
you in.

Throughout, the worked reference is the open access-control app,
[`heldar-entry`](https://github.com/Straits-AI/heldar/tree/main/crates/heldar-entry).
It is a real, compiling example of every step below. A compiled-in app also declares a
`manifest()` so it shows up in the dashboard nav (see the sidecar guide's "Manifest" section — the
shape is the same; compiled modules just return it from code instead of registering it at runtime).

## The mental model

The kernel has **no dependency on your crate**. Dependencies point one way: your
crate depends on `heldar-kernel`, and the composing server
([`heldar-server`](https://github.com/Straits-AI/heldar/blob/main/crates/heldar-server/src/main.rs))
depends on both and wires them together. You plug in through four public seams:
a `DetectionConsumer`, a `Router<AppState>`, a self-installed schema, and the
auth primitive. Adding your app is a push at a few composition points in the
server, never an edit to the kernel ingest handler or router.

## 1. A new crate depending on the kernel

Create a library crate and depend on `heldar-kernel`:

```toml
# crates/heldar-dwell/Cargo.toml
[package]
name = "heldar-dwell"
version = "0.1.0"
edition = "2021"

[dependencies]
heldar-kernel = "0.1"          # or a path/git dep during local development
axum = "0.8"
sqlx = { version = "0.8", features = ["sqlite", "runtime-tokio"] }
async-trait = "0.1"
chrono = { version = "0.4", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
```

Your `lib.rs` exposes the modules the server will reach for: a consumer, a
schema initializer, a router, and an optional config. Mirror
[`heldar-entry/src/lib.rs`](https://github.com/Straits-AI/heldar/blob/main/crates/heldar-entry/src/lib.rs).

## 2. Implement a `DetectionConsumer`

After a batch of worker detections is persisted, the kernel fans it out to every
registered consumer whose `interested_in(task_type)` returns `true`. This is the
trait, from `heldar_kernel::services::consumer`:

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

A minimal consumer that counts detections per camera:

```rust
use std::sync::Arc;
use sqlx::SqlitePool;
use heldar_kernel::services::consumer::{DetectionBatch, DetectionConsumer};

pub struct DwellCounter {
    pool: SqlitePool,
}

impl DwellCounter {
    pub fn new(pool: SqlitePool) -> Arc<Self> {
        Arc::new(Self { pool })
    }
}

#[async_trait::async_trait]
impl DetectionConsumer for DwellCounter {
    fn name(&self) -> &'static str {
        "dwell_counter"
    }

    // Self-select on task_type. Return true for all types only if you are
    // genuinely task-agnostic. heldar-entry returns true only for "anpr".
    fn interested_in(&self, task_type: &str) -> bool {
        task_type.eq_ignore_ascii_case("detection")
    }

    async fn consume(&self, batch: &DetectionBatch<'_>) {
        // batch.camera_id / .site_id / .task_type / .timestamp are the context;
        // each det carries label, confidence, bbox ([x,y,w,h] 0..1), track_id,
        // and a free-form attributes blob. Write to YOUR tables on self.pool.
        for det in batch.detections {
            let _ = sqlx::query(
                "INSERT INTO dwell_counts (camera_id, label, ts)
                 VALUES (?, ?, ?)",
            )
            .bind(batch.camera_id)
            .bind(det.label.as_deref())
            .bind(batch.timestamp)
            .execute(&self.pool)
            .await;
        }
    }
}
```

`consume` must not panic; errors are yours to log or swallow. The kernel calls it
synchronously after the batch is committed, so keep it cheap, or push work onto a
queue or your own background loop. For a full worked consumer see the ANPR engine
in
[`heldar-entry/src/anpr.rs`](https://github.com/Straits-AI/heldar/blob/main/crates/heldar-entry/src/anpr.rs)
(`impl DetectionConsumer for AnprEngine`).

## 3. Self-install your schema idempotently

Your app owns its tables. Apply them against the shared pool on startup, exactly
like
[`heldar-entry/src/schema.rs`](https://github.com/Straits-AI/heldar/blob/main/crates/heldar-entry/src/schema.rs):

```rust
// src/schema.rs
use sqlx::SqlitePool;

pub async fn init(pool: &SqlitePool) -> sqlx::Result<()> {
    sqlx::raw_sql(include_str!("schema.sql")).execute(pool).await?;
    Ok(())
}
```

```sql
-- src/schema.sql
CREATE TABLE IF NOT EXISTS dwell_counts (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    camera_id  TEXT NOT NULL,
    label      TEXT,
    ts         TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_dwell_cam_ts ON dwell_counts (camera_id, ts);
```

Use `CREATE TABLE IF NOT EXISTS` so `init` is idempotent (single-tenant per
deployment). The kernel never defines your domain tables; you share its
`SqlitePool` but own your schema.

## 4. Expose a `Router<AppState>` and merge it

Your handlers run against the kernel's `AppState`, which gives you the shared
pool (`st.pool`), the kernel config (`st.cfg`), and the recorder/sampler/HTTP
client. Use absolute `/api/v1/...` paths; the server mounts your router at root.
Reuse the auth primitive for authorization and audit, like
[`heldar-entry/src/routes.rs`](https://github.com/Straits-AI/heldar/blob/main/crates/heldar-entry/src/routes.rs):

```rust
// src/routes.rs
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};
use heldar_kernel::auth::Principal;
use heldar_kernel::error::AppResult;
use heldar_kernel::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/dwell/summary", get(summary))
}

async fn summary(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<Value>> {
    // Capability check + audit come from the kernel auth primitive. When
    // HELDAR_AUTH_ENABLED is false this is a no-op (open appliance).
    principal.require(principal.can_view(), "view dwell summary")?;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dwell_counts")
        .fetch_one(&st.pool)
        .await?;
    Ok(Json(json!({ "total": total })))
}
```

## 5. Register your consumer and spawn loops in the server

Everything comes together in the composing server. In
[`heldar-server/src/main.rs`](https://github.com/Straits-AI/heldar/blob/main/crates/heldar-server/src/main.rs)
you make four small additions:

```rust
// (a) apply your schema, after the kernel migrations have run
heldar_dwell::schema::init(&pool).await.context("dwell schema init")?;

// (b) add your consumer to the consumer vector that goes into AppState
let consumers: Arc<Vec<Arc<dyn DetectionConsumer>>> = Arc::new(vec![
    services::zones::ZoneEngine::new(pool.clone(), cfg.clone(), recorder.clone()),
    heldar_entry::anpr::AnprEngine::new(pool.clone(), cfg.clone(), entry_cfg.clone()),
    heldar_dwell::DwellCounter::new(pool.clone()),   // <- your consumer
]);

// (c) merge your router next to the kernel and the other apps
let app = Router::new()
    .merge(routes::api_router())
    .merge(heldar_entry::routes::router())
    .merge(heldar_dwell::routes::router());          // <- your router

// (d) if you have a background loop, supervise it (respawns on panic)
let p = pool.clone();
spawn_supervised("dwell_rollup", move || heldar_dwell::rollup::run(p.clone()));
```

That is the whole integration. The consumer vector is fanned out to by the kernel
ingest path without naming any consumer; the router merge is invisible to the
kernel router; the schema is your own; and `spawn_supervised` gives a panicking
loop a 5-second respawn.

## The kernel has no dependency on your crate

This is the point of the design. The dependency edges are: `heldar-dwell` to
`heldar-kernel`, and `heldar-server` to both. The kernel never imports your crate
and gains no branch for your `task_type`. That is what lets the open kernel and
the generic apps ship as separate crates, and what lets proprietary verticals
compose the same way in a private build without touching the public code.

## Not every app is a consumer

`heldar-movement` and `heldar-search` are built on the same seams but are not on
the ingest hot path: movement is two supervised background loops (a ReID
candidate proposer and a breach rule engine), and search is a read-only query
layer over already-stored kernel facts. Both still own a schema, expose a router,
and (for movement) spawn loops, composed in the server exactly as above, minus
the consumer registration. Pick the pattern that fits: a consumer for per-batch
interpretation, a loop for periodic rollups, or a plain router for read-only
queries.
