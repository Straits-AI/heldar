---
id: module-system
title: Module System
sidebar_label: Module System
sidebar_position: 1
---

# Module System

Heldar is composed, not monolithic: the kernel is domain-agnostic, and every product surface — access
control, movement, search, your own apps — is a **module** that plugs in through a small set of seams.
The dashboard's nav and routes are built **at runtime** from whatever modules the running binary
exposes, so adding one never forks the core.

This page is the map; the per-kind guides — [Build a module](./build-a-module.md),
[Sidecar plugins](./sidecar-plugins.md), [Wasm plugins](./wasm-plugins.md), [Registry](./registry.md) —
are the detail.

## The three kinds

| Kind | Process | UI | Add without… | Use for |
|---|---|---|---|---|
| **Compiled** | in-process (kernel-linked Rust crate) | a React page bundled in the dashboard | (needs a rebuild) | first-party apps that want the hot path + shared DB (entry, movement, search) |
| **Sidecar** | out-of-process service (any language) | a sandboxed iframe, reverse-proxied at `/m/{id}/*` | recompiling the kernel or dashboard | third-party / independently-deployed apps |
| **Wasm** | in-process, sandboxed (wasmi) | headless (no page) | recompiling the kernel | untrusted compute on the detection stream |

- **Compiled** modules register over the kernel seams — a `DetectionConsumer`, a `Router<AppState>`
  merge, and a self-installed schema (`schema::init`) — and expose a `manifest()`.
  See [Build a module](./build-a-module.md).
- **Sidecar** plugins register at runtime via `POST /api/v1/modules`: the kernel mints a least-privilege
  API key + a webhook subscription for the plugin and reverse-proxies its UI + API under `/m/{id}/*`.
  See [Sidecar plugins](./sidecar-plugins.md).
- **Wasm** plugins load from a directory (behind the off-by-default `wasm` feature) as sandboxed
  `DetectionConsumer`s. See [Wasm plugins](./wasm-plugins.md).

## One manifest, composed at boot + runtime

The dashboard renders its Modules nav from a single endpoint — **`GET /api/v1/modules`** — which merges
all three kinds into one list:

- **At boot**, the composing server collects the compiled modules' manifests (and, in a private build,
  any proprietary verticals via a no-op-in-open seam) plus any wasm modules, and stores them in app
  state.
- **At runtime**, the list handler unions those with the **sidecar** registrations from the database
  (each projected to a manifest, with a live health field).

The dashboard **polls `GET /api/v1/modules` every 30 seconds**, so installing or removing a sidecar
shows up in the nav without a reload or a restart. An unknown module icon falls back to a generic glyph
— a missing icon is never an error.

## The composition seam

Adding a compiled app is a *push* in one place — the composing server — not an edit to the kernel:

```rust
// crates/heldar-server/src/main.rs (sketch)
let mut modules = vec![
    heldar_entry::manifest(),
    heldar_movement::manifest(),
    heldar_search::manifest(),
];
modules.extend(verticals::manifests());          // proprietary verticals — a no-op stub in the open build
let (wasm_consumers, wasm_modules) = wasm_plugins::load(/* … */);  // no-op when the `wasm` feature is off
```

The `verticals` and `wasm_plugins` seams are how *optional* code composes without the kernel ever
referencing it: in the open build both are stubs that return nothing; a private build (or
`--features wasm`) swaps in the real thing. `main.rs` is byte-identical across the open and private
repos — see [Open-core](../concepts/open-core.md).

## Health & state

Sidecars report health at `GET /heldar/health`, which the kernel probes every 30s; the store shows each
as `healthy` / `unreachable` / `unknown`. The [registry](./registry.md) computes each catalog entry's
**shelf** (core / proprietary / community / compute) and **state** (`included` / `available` /
`installed` / `unreachable` / `not-in-build`) by cross-referencing the compiled set with the live
registrations — so the store reflects what this binary actually links plus what's installed right now.

## Modules over remote access

The [remote dashboard](../getting-started/remote-access.md) runs the full SPA over the relay, so modules
work remotely — with one nuance per kind:

- **Compiled** pages are already in the bundle, and their kernel API calls ride the relay (`/api/v1/*`). ✅
- **Sidecar** iframes reverse-proxy at `/m/{id}/*`, which the relay forwards to the box (the kernel then
  reaches the sidecar with its own minted key — never the user's). ✅
- **Wasm** modules are headless + in-process — nothing to relay. ✅

The relay is an allowlisted pipe (`/api/v1/*`, `/media/*`, `/m/*`; path traversal and Worker-internal
paths are refused) and the box runs its **own** RBAC on every forwarded request, so remote access never
widens what a role can do.
