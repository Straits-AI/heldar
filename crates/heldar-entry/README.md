# heldar-entry

*Generic access control for gated-entry deployments: ANPR authorization, a vehicle/visitor/watchlist registry, a guard workflow, and entry reports.*

[![crates.io](https://img.shields.io/crates/v/heldar-entry.svg)](https://crates.io/crates/heldar-entry)
[![docs.rs](https://docs.rs/heldar-entry/badge.svg)](https://docs.rs/heldar-entry)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/Straits-AI/heldar/blob/main/LICENSE)

`heldar-entry` is the open access-control app of the **Heldar** open-core platform. It turns ANPR
plate reads from the AI worker into authoritative entry events, resolves each plate against a
registry, and gives guards a confirm/reject workflow plus reports. It is domain-neutral: any
gated-entry site (residential, corporate, industrial) uses it as-is. It is built on the
`heldar-kernel` crate and plugs in purely through the kernel's public seams, so the kernel has no
dependency on this crate.

## What it does

- **ANPR temporal-voting engine** (`AnprEngine`) consolidates per-frame plate reads per
  `(camera, track)` into one entry/exit event once the winning plate reaches a vote threshold;
  plausible plates are preferred over noisy ones, and stale tracks commit on TTL prune.
- **Identity resolution** classifies each plate against the registry with fixed precedence: active
  block watchlist (fails closed on DB error), registered vehicle, active visitor pass, VIP
  watchlist, then unmatched. Output is `matched` / `exception` / `unmatched` / `blocked`.
- **Attribute checks as review, not rejection**: a color or vehicle-type mismatch against a
  registered plate raises an exception for guard review and never auto-rejects (make/model is
  assistive metadata only).
- **Registry CRUD** over vehicles, visitor passes (with `checkin` / `checkout`, auto check-in on a
  matched inbound pass, and issued pass codes), and a watchlist of `block` / `vip` / `alert` kinds.
- **Canonical entry-event feed** with a guard **confirm / reject** workflow; manual visitor
  check-in/out is also written into the feed.
- **Reports**: a daily entry log with per-`auth_status` counts, an exceptions report
  (blocked / exception / unmatched / rejected), and an audit log.
- **RBAC enforcement** through the kernel `Principal`: reads need any principal, gate operations
  need guard+, registry mutations and the audit log need manager+. Every mutation is audited.
- **Self-owned data lifecycle**: a self-installed SQLite schema plus a retention loop that prunes
  old entry events and deletes their evidence frames on the configured TTL.

## Where it fits

This is a **library** crate. It is consumed by the composing server: the runnable `heldar-core`
binary lives in the unpublished `heldar-server` crate, which links the kernel and each open app and
wires them together. `heldar-entry` exposes:

- `anpr::AnprEngine` plus `normalize_plate` / `is_plausible_plate` helpers. `AnprEngine` implements
  `heldar_kernel::services::consumer::DetectionConsumer`, so the server registers it as a perception
  consumer behind the kernel ingest seam.
- `routes::router() -> axum::Router<heldar_kernel::state::AppState>`, merged into the kernel router.
- `schema::init(&pool)` to install the app's tables idempotently against the shared kernel pool.
- `config::EntryConfig::from_env()` for the vote threshold and retention window.
- `retention::run(pool, cfg, ecfg)`, a background sweep the server supervises.
- `models` (`Vehicle`, `VisitorPass`, `Watchlist`, `EntryEvent`, `AuditLog`, and their create/update
  inputs).

## Usage

```sh
cargo add heldar-entry
```

It composes into the server exactly as `heldar-server` does it:

```rust
use std::sync::Arc;
use heldar_kernel::services::consumer::DetectionConsumer;

// 1. Install this app's schema against the shared kernel pool.
heldar_entry::schema::init(&pool).await?;

// 2. Load app config and register the ANPR engine as a detection consumer.
let entry_cfg = Arc::new(heldar_entry::config::EntryConfig::from_env());
let anpr: Arc<dyn DetectionConsumer> =
    heldar_entry::anpr::AnprEngine::new(pool.clone(), cfg.clone(), entry_cfg.clone());

// 3. Merge the HTTP routes into the kernel router.
let app = kernel_router.merge(heldar_entry::routes::router());

// 4. Run the retention loop in the background.
tokio::spawn(heldar_entry::retention::run(pool, cfg, entry_cfg));
```

See `crates/heldar-server/src/main.rs` in the repo for the full wiring, and the
[Access Control guide](https://github.com/Straits-AI/heldar/blob/main/docs/ACCESS-CONTROL.md) for the
end-to-end ANPR pipeline and the HTTP API.

## Documentation

- Repository: <https://github.com/Straits-AI/heldar>
- Guide: [docs/ACCESS-CONTROL.md](https://github.com/Straits-AI/heldar/blob/main/docs/ACCESS-CONTROL.md)
- API docs: <https://docs.rs/heldar-entry>

## License

Apache-2.0.
