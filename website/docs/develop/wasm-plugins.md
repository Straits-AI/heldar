---
id: wasm-plugins
title: Wasm Plugins
sidebar_label: Wasm Plugins
sidebar_position: 4
---

# Wasm plugins

A **Wasm plugin** is a sandboxed, headless [`DetectionConsumer`](./build-a-module.md): after the kernel
persists a detection batch it hands it to the plugin (as JSON), which runs in a WebAssembly sandbox
with **zero ambient authority** — no filesystem, network, clock, or randomness — and emits derived
events back. Emitted events are namespaced, camera-scoped, capped, and persisted through the kernel's
normal event path, so they flow to webhooks + sidecars for free.

It is the specialist tool for **lightweight, strongly-sandboxed, in-process rule/transform logic** on
the ingest hot path. For anything with a UI, multiple languages, or heavy/stateful work, use a
[sidecar plugin](./sidecar-plugins.md) instead — sidecars get a network + a scoped key; a Wasm guest
gets neither.

## How it fits

| | Sidecar (Phase B) | Wasm plugin (Phase D) |
| --- | --- | --- |
| Process | separate (any language) | in-process (sandbox) |
| UI | yes (iframe at `/m/{id}/`) | none (headless) |
| Capability | scoped API key + network | **none** — pure compute |
| Best for | apps, UIs, integrations | rules, filters, derived events |

The runtime ([wasmi](https://github.com/wasmi-labs/wasmi), a pure-Rust interpreter) ships behind an
**off-by-default `wasm` cargo feature** — the default appliance binary never links it. Build the server
with `--features wasm` to enable plugin loading.

## The plugin

A guest is a `wasm32-unknown-unknown` core module. It exports a tiny ABI and may import exactly two host
functions (`heldar.log`, `heldar.emit_event`) — importing anything else (e.g. WASI) fails to load, so
the sandbox is closed by construction. The complete, copy-pasteable template is
[`examples/wasm-plugin`](https://github.com/Straits-AI/heldar/tree/main/examples/wasm-plugin); the only
part you change is the `rule()` function:

```rust
fn rule(input: &Input) {
    let threshold = input.config.get("threshold").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
    let persons = input.detections.iter().filter(|d| d.label.as_deref() == Some("person")).count();
    if persons > threshold {
        emit(&Event {
            event_type: "occupancy.high".into(),
            severity: "warning".into(),
            payload: json!({ "persons": persons }),
        });
    }
}
```

The host calls `heldar_describe()` once at load to read `{ id, name, version, publisher, description,
interested_in }`, then `heldar_handle(ptr, len)` per batch (JSON input written to guest memory). Events
the guest passes to `emit_event` are buffered and persisted after the call as
`wasm.{plugin_id}.{event_type}`, **always scoped to the batch's camera** (a guest cannot forge events
for another camera) with the severity clamped to `info`/`warning`/`critical`.

## Build + load

```bash
# 1. build the guest to wasm32 (the example)
cd examples/wasm-plugin
cargo build --release --target wasm32-unknown-unknown

# 2. drop it into the plugins directory
cp target/wasm32-unknown-unknown/release/heldar_occupancy_plugin.wasm \
   <data>/wasm-plugins/occupancy.wasm

# 3. run the server with the wasm feature
cargo run -p heldar-server --features wasm
```

Loaded plugins appear in `GET /api/v1/modules` (mount `headless`, no nav route) and on the **Plugins**
store with a *sandboxed compute* treatment. v1 loads at boot; changing plugins means a restart.

## Sandbox + limits

Every guest runs with hard bounds, configured via env (read by the plugin host):

| Env | Default | Bounds |
| --- | --- | --- |
| `HELDAR_WASM_ENABLED` | `true` | master switch (with the `wasm` feature on) |
| `HELDAR_WASM_PLUGINS_DIR` | `<data>/wasm-plugins` | where `*.wasm` are loaded from |
| `HELDAR_WASM_FUEL` | `50000000` | per-call instruction budget (CPU DoS bound — an infinite loop traps) |
| `HELDAR_WASM_MAX_MEMORY_MB` | `64` | per-call linear-memory cap |
| `HELDAR_WASM_MAX_TABLE_ELEMENTS` | `100000` | per-call table-element cap (tables are host RAM, not covered by the memory cap) |
| `HELDAR_WASM_MAX_EVENTS` | `64` | events a guest may emit per call |
| `HELDAR_WASM_MAX_EVENT_BYTES` | `16384` | per-event byte cap |
| `HELDAR_WASM_MAX_LOG_CALLS` | `256` | `heldar.log` calls per batch (bounds a log flood) |
| `HELDAR_WASM_MAX_FAILURES` | `5` | consecutive failures before the plugin is auto-disabled |

A guest trap, fuel exhaustion, OOM, or panic is isolated — it is logged and never crashes the kernel,
and a repeatedly-failing plugin is circuit-broken (disabled + a `wasm_plugin_disabled` event). Guests
run on `spawn_blocking` so wasm CPU never blocks the async reactor.

## Trust + scope

v1 loads plugins from a **local, operator-controlled directory** (operator-trusted). The kernel never
downloads or executes remote `.wasm`, and there is no per-artifact signing yet — those, the
[Component Model](https://component-model.bytecodealliance.org/), WASI, host-provided state, and
multi-language SDKs are deliberate non-goals for v1. If you later run untrusted third-party Wasm, the
upgrade path is the [wasmtime](https://wasmtime.dev/) runtime (epoch interruption + a more hardened
sandbox) behind the same seam.
