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

| Kind | Process | UI (mount) | Add without… | Use for |
|---|---|---|---|---|
| **In-process** | kernel-linked Rust crate | a React ES bundle the crate serves at `/api/v1/modules/{id}/ui`, imported by the dashboard **at runtime** (`mount: runtime`) | rebuilding the dashboard | first-party apps that want the hot path + shared DB (entry, movement, search, verticals) |
| **Sidecar** | out-of-process service (any language) | a sandboxed iframe, reverse-proxied at `/m/{id}/*` (`mount: iframe`) | recompiling the kernel or dashboard | third-party / independently-deployed apps |
| **Wasm** | in-process, sandboxed (wasmi) | headless — no page (`mount: headless`) | recompiling the kernel | untrusted compute on the detection stream |

- **In-process** modules register over the kernel seams — a `DetectionConsumer`, a `Router<AppState>`
  merge, and a versioned self-installed schema (`schema::init` over `db::run_app_migrations`) — and expose a `manifest()` that carries
  `mount: runtime` + a `ui_url`. Their UI is **not** compiled into the dashboard (see below).
  See [Build a module](./build-a-module.md).
- **Sidecar** plugins register at runtime via `POST /api/v1/modules`: the kernel mints a least-privilege
  API key + a webhook subscription for the plugin and reverse-proxies its UI + API under `/m/{id}/*`.
  See [Sidecar plugins](./sidecar-plugins.md).
- **Wasm** plugins load from a directory (behind the off-by-default `wasm` feature) as sandboxed
  `DetectionConsumer`s. See [Wasm plugins](./wasm-plugins.md).

## Runtime-loaded module UIs

An in-process module's dashboard page is **not** bundled into the SPA. Each crate builds its page as a
standalone Vite **library** bundle (an ES module) and embeds it via `include_str!`; the kernel serves it
at `GET /api/v1/modules/{id}/ui/index.js` (viewer-gated). The dashboard's `ModuleHost` component reads the
manifest's `ui_url`, dynamically `import()`s that bundle, and mounts its default-exported React component.

The bundle does **not** ship its own React or UI kit. It imports `react` and the shell SDK
(**`@heldar/shell`** — the api client, auth/session, design system, and formatters) as *externals*; an
import map in the dashboard resolves them to the shell's single instances at runtime. So a module shares
the shell's React and design system instead of duplicating them — the built bundles are small
(~10–50 KB) and always match the host.

Why it matters: because no module UI is compiled into the dashboard, the SPA is **byte-identical for the
open and full builds**. There is one `heldar-web` image for both, and a proprietary vertical ships its UI
as one self-contained bundle its own repository builds and serves — nothing about it lives in, or has to
be stripped from, this tree. A module that ships no page (e.g. a headless compute plugin) simply omits
`ui_url`.

## One manifest, composed at boot + runtime

The dashboard renders its Modules nav from a single endpoint — **`GET /api/v1/modules`** — which merges
all three kinds into one list:

- **At boot**, the composing server collects the in-process modules' manifests (each carrying its
  `ui_url`) — and, in a private build, any proprietary verticals via a no-op-in-open seam — plus any
  wasm modules, and stores them in app state.
- **At runtime**, the list handler unions those with the **sidecar** registrations from the database
  (each projected to a manifest, with a live health field).

The dashboard **polls `GET /api/v1/modules` every 30 seconds**, so installing or removing a sidecar
shows up in the nav without a reload or a restart. An unknown module icon falls back to a generic glyph
— a missing icon is never an error.

## The composition seam

Adding an in-process app is a *push* in one place — the composing server — not an edit to the kernel:

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
`installed` / `unreachable` / `not-in-build`) by cross-referencing the in-process (kernel-linked) set
with the live registrations — so the store reflects what this binary actually links plus what's
installed right now.

## Sidecar isolation and the host bridge

A sidecar's UI runs in an iframe whose sandbox **omits `allow-same-origin`**, so the frame has an
**opaque origin**. It cannot read the parent DOM, cannot touch storage, and — the part that matters —
its requests carry **no console session cookie**, because a sandboxed frame has a null
site-for-cookies. A hostile or compromised plugin therefore cannot act with the operator's authority
merely by being served from the console's origin.

That isolation means a plugin cannot call anything directly, so the host mediates. The plugin posts a
message; the host validates it, performs the request with the session, and posts the result back:

```js
// inside a sidecar UI
function call(path, opts = {}) {
  const id = crypto.randomUUID();
  return new Promise((resolve, reject) => {
    function onReply(e) {
      if (e.data?.heldar !== "response" || e.data.id !== id) return;
      window.removeEventListener("message", onReply);
      e.data.ok ? resolve(e.data) : reject(new Error(e.data.error));
    }
    window.addEventListener("message", onReply);
    parent.postMessage({ heldar: "request", id, method: opts.method ?? "GET", path, body: opts.body,
                         headers: opts.headers }, "*");
  });
}

const res = await call("api/status");        // -> GET /m/<your-id>/api/status
```

**What the bridge will and will not do:**

| | |
| --- | --- |
| Paths | Resolved against **your own** `/m/{id}/` root. `../`, a leading `/`, an absolute or protocol-relative URL, or another plugin's root are all refused. |
| Methods | `GET`, `POST`, `PUT`, `PATCH`, `DELETE`. |
| Headers | Only `content-type` is forwarded. A plugin cannot set authorization or identity headers on a request the host makes with the operator's session. |
| Timeout | 20s, then the request is aborted. |

So a plugin's reach is exactly its own sidecar — not the kernel API. If your plugin needs kernel data,
have your **sidecar** fetch it server-side with its own minted key (which carries its own capabilities),
and expose it under your own path.

**This is a breaking change for plugin UIs** that previously called `/api/v1/*` directly or reached
into the parent document. Both were only ever possible because the plugin shared the console's origin,
which is exactly the property that made an imported plugin equivalent to first-party code.

## Modules over remote access

The [remote dashboard](../getting-started/remote-access.md) runs the full SPA over the relay, so modules
work remotely — with one nuance per kind:

- **In-process** modules serve their UI bundle at `/api/v1/modules/{id}/ui` and make their API calls under
  `/api/v1/*` — both ride the relay, so the dashboard loads *and* runs them remotely with no extra
  plumbing (this is how a **proprietary** module reaches a remote operator without ever shipping in the
  open image). ✅
- **Sidecar** iframes reverse-proxy at `/m/{id}/*`, which the relay forwards to the box (the kernel then
  reaches the sidecar with its own minted key — never the user's). ✅
- **Wasm** modules are headless + in-process — nothing to relay. ✅

The relay is an allowlisted pipe (`/api/v1/*`, `/media/*`, `/m/*`; path traversal and Worker-internal
paths are refused) and the box runs its **own** RBAC on every forwarded request, so remote access never
widens what a role can do.
