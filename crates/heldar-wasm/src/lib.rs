//! Heldar sandboxed in-process Wasm plugin host (Phase D of the plugin platform).
//!
//! A Wasm plugin is a HEADLESS, capability-zero [`DetectionConsumer`]: after the kernel persists a
//! detection batch it hands it (as JSON) to each plugin, which runs in a [`wasmi`] sandbox with NO
//! ambient authority (no WASI ⇒ no filesystem/network/clock/random) and emits derived events back
//! through a host import. Emitted events are validated, camera-scoped, namespaced (`wasm.{id}.{type}`),
//! and capped, then persisted via the kernel's canonical [`heldar_kernel::repo::log_event`] — so they
//! flow into the events table → webhooks → sidecars with no new plumbing.
//!
//! It complements the out-of-process sidecar plugins (Phase B): sidecars are for any-language services
//! with a UI; Wasm plugins are for lightweight, strongly-sandboxed, in-process rule/transform logic on
//! the ingest path. The whole runtime is an OPTIONAL `wasm` feature on the composing server — the
//! default appliance binary never links it.
//!
//! Trust model: v1 loads `*.wasm` from a local operator-controlled directory (operator-trusted).
//! Remote download/execution + per-artifact signature verification are deliberately out of scope.
//! Resource bounds: per-call fuel (CPU), a linear-memory cap, a table-element cap (tables are host RAM
//! outside the memory cap), and per-call event/byte + log-call caps; a guest trap/fuel-exhaustion/OOM/
//! panic is isolated (never unwinds into the kernel) and a per-plugin circuit breaker disables a
//! repeatedly-failing plugin.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::SqlitePool;
use wasmi::{Caller, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};

use heldar_kernel::modules::{ModuleKind, ModuleManifest, MountKind, NavEntry};
use heldar_kernel::services::consumer::{DetectionBatch, DetectionConsumer};

/// The ABI version this host speaks. A guest must export `heldar_abi_version() -> i32` returning this.
const ABI_VERSION: i32 = 1;

/// Tunables, read from the environment by [`load_dir`] (kept out of the kernel config so the kernel
/// gains no Wasm-specific knobs).
#[derive(Clone)]
struct Limits {
    fuel: u64,
    max_memory_bytes: usize,
    /// Cap on a guest's table element count — a table is backed by host RAM (8 bytes/elem) and is NOT
    /// covered by the linear-memory cap, so an uncapped `(table N funcref)` could OOM the process.
    max_table_elements: usize,
    max_events: usize,
    max_event_bytes: usize,
    /// Cap on `heldar.log` calls per batch (host-fn calls aren't fuel-metered; bounds a log flood).
    max_log_calls: usize,
    max_failures: u32,
}

impl Limits {
    fn from_env() -> Self {
        let mb: usize = env_parse("HELDAR_WASM_MAX_MEMORY_MB", 64);
        Limits {
            fuel: env_parse("HELDAR_WASM_FUEL", 50_000_000),
            max_memory_bytes: mb.saturating_mul(1024 * 1024),
            max_table_elements: env_parse("HELDAR_WASM_MAX_TABLE_ELEMENTS", 100_000),
            max_events: env_parse("HELDAR_WASM_MAX_EVENTS", 64),
            max_event_bytes: env_parse("HELDAR_WASM_MAX_EVENT_BYTES", 16 * 1024),
            max_log_calls: env_parse("HELDAR_WASM_MAX_LOG_CALLS", 256),
            max_failures: env_parse("HELDAR_WASM_MAX_FAILURES", 5),
        }
    }
}

fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// An event a guest asked to emit (deserialized from the bytes it passed to `emit_event`).
#[derive(Debug, Deserialize)]
struct GuestEvent {
    event_type: String,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    payload: Value,
}

/// Per-call host state living in the wasmi `Store`. `emit_event` pushes into `events`; the consumer
/// drains them after the call and persists them.
struct HostState {
    events: Vec<GuestEvent>,
    max_events: usize,
    max_event_bytes: usize,
    dropped: u32,
    log_calls: usize,
    max_log_calls: usize,
    limits: StoreLimits,
}

/// What a guest's `heldar_describe()` returns.
#[derive(Deserialize)]
struct Describe {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    publisher: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    interested_in: Vec<String>,
}

/// A loaded Wasm plugin, adapted to the kernel's [`DetectionConsumer`] seam.
pub struct WasmConsumer {
    id: String,
    name_static: &'static str,
    interested: HashSet<String>,
    engine: Engine,
    module: Module,
    pool: SqlitePool,
    config: Value,
    limits: Limits,
    consecutive_failures: AtomicU32,
    disabled: AtomicBool,
}

#[async_trait]
impl DetectionConsumer for WasmConsumer {
    fn name(&self) -> &'static str {
        self.name_static
    }

    fn interested_in(&self, task_type: &str) -> bool {
        self.interested.contains("*") || self.interested.contains(task_type)
    }

    async fn consume(&self, batch: &DetectionBatch<'_>) {
        if self.disabled.load(Ordering::Relaxed) {
            return;
        }
        let input = json!({
            "camera_id": batch.camera_id,
            "site_id": batch.site_id,
            "task_type": batch.task_type,
            "timestamp": batch.timestamp,
            "detections": batch.detections,
            "config": self.config,
        });
        let Ok(input_bytes) = serde_json::to_vec(&input) else {
            return;
        };

        // Wasm is CPU-bound + the wasmi Store is not Sync; run it off the async reactor. A fresh Store
        // per call gives clean linear memory (no cross-call/tenant state bleed) and trivial recovery.
        let engine = self.engine.clone();
        let module = self.module.clone();
        let limits = self.limits.clone();
        let outcome =
            tokio::task::spawn_blocking(move || run_guest(&engine, &module, &input_bytes, &limits))
                .await;

        let (events, dropped) = match outcome {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => return self.record_failure(&e).await,
            Err(_) => return self.record_failure("guest task panicked").await,
        };
        self.consecutive_failures.store(0, Ordering::Relaxed);
        if dropped > 0 {
            // The guest emitted past max_events / oversized — surface the truncation, don't hide it.
            tracing::warn!(plugin = %self.id, dropped, "wasm: plugin emitted events were dropped (cap hit)");
        }

        // Persist emitted events: namespaced, severity-clamped, and FORCE-scoped to the batch camera
        // (a guest cannot forge events for another camera).
        for ev in events {
            let event_type = format!("wasm.{}.{}", self.id, sanitize_type(&ev.event_type));
            let severity = clamp_severity(ev.severity.as_deref());
            if let Err(e) = heldar_kernel::repo::log_event(
                &self.pool,
                Some(batch.camera_id),
                &event_type,
                severity,
                ev.payload,
            )
            .await
            {
                tracing::warn!(plugin = %self.id, error = %e, "wasm: failed to persist emitted event");
            }
        }
    }
}

impl WasmConsumer {
    async fn record_failure(&self, err: &str) {
        let n = self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::warn!(plugin = %self.id, failures = n, error = %err, "wasm: plugin call failed");
        if n >= self.limits.max_failures && !self.disabled.swap(true, Ordering::Relaxed) {
            tracing::error!(plugin = %self.id, "wasm: disabling plugin after repeated failures");
            let _ = heldar_kernel::repo::log_event(
                &self.pool,
                None,
                "wasm_plugin_disabled",
                "warning",
                json!({ "plugin": self.id, "consecutive_failures": n, "last_error": err }),
            )
            .await;
        }
    }
}

fn sanitize_type(s: &str) -> String {
    let t: String = s
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.' || *c == '-')
        .take(64)
        .collect();
    if t.is_empty() {
        "event".into()
    } else {
        t
    }
}

fn clamp_severity(s: Option<&str>) -> &'static str {
    match s {
        Some("warning") => "warning",
        Some("critical") => "critical",
        _ => "info",
    }
}

/// Build a `Linker` exposing exactly the `heldar` host namespace (`log`, `emit_event`) and nothing
/// else — so any guest importing WASI or another symbol fails instantiation (closed sandbox).
fn build_linker(engine: &Engine) -> Result<Linker<HostState>, String> {
    let mut linker = Linker::<HostState>::new(engine);
    linker
        .func_wrap(
            "heldar",
            "log",
            |mut caller: Caller<'_, HostState>, level: i32, ptr: i32, len: i32| {
                // Bound log calls per batch: host-fn bodies aren't fuel-metered, so an uncapped log in
                // a tight loop is a disk-fill/CPU DoS. Drop silently past the cap (mirrors max_events).
                if caller.data().log_calls >= caller.data().max_log_calls {
                    return;
                }
                caller.data_mut().log_calls += 1;
                if let Some(bytes) = read_caller_mem(&mut caller, ptr, len, 8 * 1024) {
                    let msg = String::from_utf8_lossy(&bytes);
                    match level {
                        0 | 1 => tracing::debug!(target: "wasm_plugin", "{msg}"),
                        2 => tracing::info!(target: "wasm_plugin", "{msg}"),
                        3 => tracing::warn!(target: "wasm_plugin", "{msg}"),
                        _ => tracing::error!(target: "wasm_plugin", "{msg}"),
                    }
                }
            },
        )
        .map_err(|e| format!("link log: {e}"))?;
    linker
        .func_wrap(
            "heldar",
            "emit_event",
            |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| -> i32 {
                let max_bytes = caller.data().max_event_bytes;
                let max_events = caller.data().max_events;
                if (len as usize) > max_bytes {
                    caller.data_mut().dropped += 1;
                    return -3;
                }
                if caller.data().events.len() >= max_events {
                    caller.data_mut().dropped += 1;
                    return -4;
                }
                let Some(bytes) = read_caller_mem(&mut caller, ptr, len, max_bytes) else {
                    return -2;
                };
                match serde_json::from_slice::<GuestEvent>(&bytes) {
                    Ok(ev) => {
                        caller.data_mut().events.push(ev);
                        0
                    }
                    Err(_) => -5,
                }
            },
        )
        .map_err(|e| format!("link emit_event: {e}"))?;
    Ok(linker)
}

/// Read `len` bytes at `ptr` from the caller's exported `memory`, bounds-checked. Returns None on a bad
/// pointer/len or oversize.
fn read_caller_mem(
    caller: &mut Caller<'_, HostState>,
    ptr: i32,
    len: i32,
    max: usize,
) -> Option<Vec<u8>> {
    if ptr < 0 || len < 0 || len as usize > max {
        return None;
    }
    let memory = caller.get_export("memory").and_then(|e| e.into_memory())?;
    let data = memory.data(&caller);
    let (off, len) = (ptr as usize, len as usize);
    let end = off.checked_add(len)?;
    if end > data.len() {
        return None;
    }
    Some(data[off..end].to_vec())
}

fn new_store(engine: &Engine, limits: &Limits) -> Result<Store<HostState>, String> {
    // Cap BOTH linear memory AND table elements: a table is host RAM not covered by memory_size, so an
    // uncapped `(table N funcref)` could OOM the process at instantiation (outside fuel metering).
    let store_limits = StoreLimitsBuilder::new()
        .memory_size(limits.max_memory_bytes)
        .memories(1)
        .tables(8)
        .table_elements(limits.max_table_elements)
        .build();
    let mut store = Store::new(
        engine,
        HostState {
            events: Vec::new(),
            max_events: limits.max_events,
            max_event_bytes: limits.max_event_bytes,
            dropped: 0,
            log_calls: 0,
            max_log_calls: limits.max_log_calls,
            limits: store_limits,
        },
    );
    store.limiter(|s| &mut s.limits);
    store
        .set_fuel(limits.fuel)
        .map_err(|e| format!("set_fuel: {e}"))?;
    Ok(store)
}

/// Run one batch through a guest: fresh Store, write the input, call `heldar_handle`, return the
/// buffered events plus the count of events the guest tried to emit past the caps (dropped). Any trap
/// / fuel-exhaustion / bad ABI is an `Err` (the caller isolates it).
fn run_guest(
    engine: &Engine,
    module: &Module,
    input: &[u8],
    limits: &Limits,
) -> Result<(Vec<GuestEvent>, u32), String> {
    let linker = build_linker(engine)?;
    let mut store = new_store(engine, limits)?;
    let instance = linker
        .instantiate_and_start(&mut store, module)
        .map_err(|e| format!("instantiate: {e}"))?;

    let abi = instance
        .get_typed_func::<(), i32>(&store, "heldar_abi_version")
        .map_err(|e| format!("missing heldar_abi_version: {e}"))?;
    let v = abi
        .call(&mut store, ())
        .map_err(|e| format!("abi trap: {e}"))?;
    if v != ABI_VERSION {
        return Err(format!("abi version {v} != {ABI_VERSION}"));
    }

    let alloc = instance
        .get_typed_func::<i32, i32>(&store, "heldar_alloc")
        .map_err(|e| format!("missing heldar_alloc: {e}"))?;
    let ptr = alloc
        .call(&mut store, input.len() as i32)
        .map_err(|e| format!("alloc trap: {e}"))?;
    let memory = instance
        .get_memory(&store, "memory")
        .ok_or("guest exports no memory")?;
    memory
        .write(&mut store, ptr as usize, input)
        .map_err(|e| format!("write input: {e}"))?;

    let handle = instance
        .get_typed_func::<(i32, i32), i32>(&store, "heldar_handle")
        .map_err(|e| format!("missing heldar_handle: {e}"))?;
    let rc = handle
        .call(&mut store, (ptr, input.len() as i32))
        .map_err(|e| format!("handle trap: {e}"))?;
    if rc != 0 {
        return Err(format!("guest returned {rc}"));
    }
    let data = store.into_data();
    Ok((data.events, data.dropped))
}

/// Instantiate a freshly-compiled module once to read its self-description.
fn describe(engine: &Engine, module: &Module, limits: &Limits) -> Result<Describe, String> {
    let linker = build_linker(engine)?;
    let mut store = new_store(engine, limits)?;
    let instance = linker
        .instantiate_and_start(&mut store, module)
        .map_err(|e| format!("instantiate: {e}"))?;
    let f = instance
        .get_typed_func::<(), i64>(&store, "heldar_describe")
        .map_err(|e| format!("missing heldar_describe: {e}"))?;
    let packed = f
        .call(&mut store, ())
        .map_err(|e| format!("describe trap: {e}"))?;
    let ptr = ((packed as u64) >> 32) as usize;
    let len = ((packed as u64) & 0xffff_ffff) as usize;
    let memory = instance
        .get_memory(&store, "memory")
        .ok_or("guest exports no memory")?;
    let data = memory.data(&store);
    let end = ptr.checked_add(len).ok_or("describe ptr+len overflow")?;
    if end > data.len() || len > 64 * 1024 {
        return Err("describe range out of bounds".into());
    }
    serde_json::from_slice::<Describe>(&data[ptr..end]).map_err(|e| format!("describe json: {e}"))
}

/// Load every `*.wasm` plugin from `dir`, returning the consumers + their manifests. A plugin that
/// fails to compile/describe is skipped with a logged error (it never aborts boot). Returns empty when
/// the directory is absent. `reserved` are module ids already composed (compiled apps + verticals); a
/// plugin colliding with one is rejected so it can't duplicate an id in `GET /api/v1/modules`.
pub fn load_dir(
    dir: &Path,
    pool: SqlitePool,
    reserved: &[String],
) -> (Vec<Arc<dyn DetectionConsumer>>, Vec<ModuleManifest>) {
    let limits = Limits::from_env();
    let mut config = wasmi::Config::default();
    config.consume_fuel(true);
    let engine = Engine::new(&config);

    let mut consumers: Vec<Arc<dyn DetectionConsumer>> = Vec::new();
    let mut manifests: Vec<ModuleManifest> = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => {
            tracing::info!(dir = %dir.display(), "wasm: plugins dir absent; no Wasm plugins loaded");
            return (consumers, manifests);
        }
    };
    // Seed with the already-composed ids so a plugin id colliding with a compiled/vertical module is
    // rejected (load_one treats a `seen` hit as a duplicate).
    let mut seen: HashSet<String> = reserved.iter().cloned().collect();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("wasm") {
            continue;
        }
        match load_one(&engine, &path, &pool, &limits, &mut seen) {
            Ok((consumer, manifest)) => {
                tracing::info!(plugin = %manifest.id, path = %path.display(), "wasm: loaded plugin");
                consumers.push(consumer);
                manifests.push(manifest);
            }
            Err(e) => {
                tracing::error!(path = %path.display(), error = %e, "wasm: failed to load plugin")
            }
        }
    }
    (consumers, manifests)
}

fn load_one(
    engine: &Engine,
    path: &Path,
    pool: &SqlitePool,
    limits: &Limits,
    seen: &mut HashSet<String>,
) -> Result<(Arc<dyn DetectionConsumer>, ModuleManifest), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read: {e}"))?;
    let module = Module::new(engine, &bytes[..]).map_err(|e| format!("compile: {e}"))?;
    let d = describe(engine, &module, limits)?;
    if d.id.trim().is_empty()
        || !d
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("invalid plugin id `{}`", d.id));
    }
    if !seen.insert(d.id.clone()) {
        return Err(format!("duplicate plugin id `{}`", d.id));
    }
    let name = d.name.clone().unwrap_or_else(|| d.id.clone());
    let interested: HashSet<String> = if d.interested_in.is_empty() {
        ["*".to_string()].into_iter().collect()
    } else {
        d.interested_in.iter().cloned().collect()
    };
    let manifest = ModuleManifest {
        id: d.id.clone(),
        name: name.clone(),
        version: d.version.clone().unwrap_or_default(),
        publisher: d.publisher.clone().unwrap_or_else(|| "local".into()),
        kind: ModuleKind::Imported,
        description: d
            .description
            .clone()
            .unwrap_or_else(|| "Sandboxed Wasm detection plugin.".into()),
        nav: Vec::<NavEntry>::new(), // headless: no route
        mount: MountKind::Headless,
        health: Some("loaded".into()),
    };
    let consumer = Arc::new(WasmConsumer {
        name_static: Box::leak(d.id.clone().into_boxed_str()),
        id: d.id,
        interested,
        engine: engine.clone(),
        module,
        pool: pool.clone(),
        config: Value::Null,
        limits: limits.clone(),
        consecutive_failures: AtomicU32::new(0),
        disabled: AtomicBool::new(false),
    });
    Ok((consumer, manifest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        let mut c = wasmi::Config::default();
        c.consume_fuel(true);
        Engine::new(&c)
    }

    fn limits() -> Limits {
        Limits {
            fuel: 2_000_000,
            max_memory_bytes: 4 * 1024 * 1024,
            max_table_elements: 10_000,
            max_events: 8,
            max_event_bytes: 4096,
            max_log_calls: 16,
            max_failures: 3,
        }
    }

    fn module(engine: &Engine, wat: &str) -> Module {
        Module::new(engine, &wat::parse_str(wat).unwrap()[..]).unwrap()
    }

    // A conforming guest: describes itself + emits one event in handle. describe JSON is 40 bytes at
    // offset 100 (packed = (100<<32)|40 = 429496729640); event JSON is 38 bytes at offset 300.
    const GOOD: &str = r#"(module
        (import "heldar" "emit_event" (func $emit (param i32 i32) (result i32)))
        (memory (export "memory") 1)
        (data (i32.const 100) "{\"id\":\"t\",\"interested_in\":[\"detection\"]}")
        (data (i32.const 300) "{\"event_type\":\"hit\",\"severity\":\"warning\"}")
        (func (export "heldar_abi_version") (result i32) (i32.const 1))
        (func (export "heldar_alloc") (param i32) (result i32) (i32.const 2048))
        (func (export "heldar_describe") (result i64) (i64.const 429496729640))
        (func (export "heldar_handle") (param i32 i32) (result i32)
            (drop (call $emit (i32.const 300) (i32.const 41)))
            (i32.const 0)))"#;

    #[test]
    fn describes_and_emits() {
        let engine = engine();
        let module = module(&engine, GOOD);
        let d = describe(&engine, &module, &limits()).unwrap();
        assert_eq!(d.id, "t");
        assert_eq!(d.interested_in, vec!["detection".to_string()]);
        let (events, dropped) = run_guest(&engine, &module, b"{}", &limits()).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(dropped, 0);
        assert_eq!(events[0].event_type, "hit");
        assert_eq!(events[0].severity.as_deref(), Some("warning"));
    }

    /// An infinite loop in the guest exhausts fuel and TRAPS — returned as Err, never a kernel panic.
    #[test]
    fn fuel_exhaustion_is_isolated() {
        let spin = r#"(module
            (memory (export "memory") 1)
            (func (export "heldar_abi_version") (result i32) (i32.const 1))
            (func (export "heldar_alloc") (param i32) (result i32) (i32.const 2048))
            (func (export "heldar_handle") (param i32 i32) (result i32)
                (loop $l (br $l)) (i32.const 0)))"#;
        let engine = engine();
        let module = module(&engine, spin);
        let err = run_guest(&engine, &module, b"{}", &limits()).unwrap_err();
        assert!(
            err.contains("trap") || err.to_lowercase().contains("fuel"),
            "got: {err}"
        );
    }

    /// A guest declaring a table larger than the element cap fails to instantiate — tables are host
    /// RAM not covered by the linear-memory cap, so this guards an OOM vector.
    #[test]
    fn oversize_table_is_rejected() {
        let big = r#"(module
            (memory (export "memory") 1)
            (table 200000 funcref)
            (func (export "heldar_abi_version") (result i32) (i32.const 1))
            (func (export "heldar_alloc") (param i32) (result i32) (i32.const 2048))
            (func (export "heldar_handle") (param i32 i32) (result i32) (i32.const 0)))"#;
        let engine = engine();
        let module = module(&engine, big);
        // test limits() caps table_elements at 10_000 < 200_000.
        assert!(run_guest(&engine, &module, b"{}", &limits()).is_err());
    }

    /// A guest importing anything outside the `heldar` namespace (e.g. WASI) fails instantiation —
    /// the sandbox is closed by construction (the Linker defines only `heldar`).
    #[test]
    fn forbidden_import_is_rejected() {
        let wasi = r#"(module
            (import "wasi_snapshot_preview1" "fd_write"
                (func (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "heldar_abi_version") (result i32) (i32.const 1))
            (func (export "heldar_alloc") (param i32) (result i32) (i32.const 2048))
            (func (export "heldar_handle") (param i32 i32) (result i32) (i32.const 0)))"#;
        let engine = engine();
        let module = module(&engine, wasi);
        assert!(run_guest(&engine, &module, b"{}", &limits()).is_err());
    }
}
