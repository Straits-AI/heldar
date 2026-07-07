//! Example Heldar Wasm plugin — a sandboxed, headless `DetectionConsumer`.
//!
//! It receives each persisted detection batch as JSON and emits an `occupancy.high` event when the
//! batch contains more than `threshold` person detections. Build it to `wasm32-unknown-unknown` and
//! drop the `.wasm` into the host's `HELDAR_WASM_PLUGINS_DIR`:
//!
//! ```bash
//! cargo build --release --target wasm32-unknown-unknown
//! cp target/wasm32-unknown-unknown/release/heldar_occupancy_plugin.wasm \
//!    <data>/wasm-plugins/occupancy.wasm
//! ```
//!
//! The whole file is the template: the `// ---- ABI ----` block is the ~50-line boilerplate every
//! plugin shares; the `rule()` function is the only part you change. No SDK crate, no WASI, no host
//! access beyond the two `heldar` imports — the guest is pure compute.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/* ----------------------------- the rule ----------------------------- */

/// The plugin's identity (returned to the host via `heldar_describe`).
fn manifest() -> Value {
    json!({
        "id": "occupancy",
        "name": "Occupancy Rule",
        "version": "1.0.0",
        "publisher": "Heldar",
        "description": "Emits occupancy.high when a batch has more than `threshold` person detections.",
        "interested_in": ["detection"]
    })
}

/// The only function a plugin author writes: inspect the batch, emit events.
fn rule(input: &Input) {
    let threshold = input
        .config
        .get("threshold")
        .and_then(|v| v.as_u64())
        .unwrap_or(3) as usize;
    let persons = input
        .detections
        .iter()
        .filter(|d| d.label.as_deref() == Some("person"))
        .count();
    if persons > threshold {
        emit(&Event {
            event_type: "occupancy.high".into(),
            severity: "warning".into(),
            payload: json!({ "persons": persons, "threshold": threshold }),
        });
    }
}

#[derive(Deserialize)]
struct Input {
    #[allow(dead_code)]
    camera_id: String,
    #[allow(dead_code)]
    task_type: String,
    detections: Vec<Detection>,
    config: Value,
}

#[derive(Deserialize)]
struct Detection {
    label: Option<String>,
}

#[derive(Serialize)]
struct Event {
    event_type: String,
    severity: String,
    payload: Value,
}

/* ------------------------------ ABI -------------------------------- */
// Boilerplate shared by every plugin. The host (heldar-wasm) speaks ABI version 1.

#[link(wasm_import_module = "heldar")]
extern "C" {
    fn log(level: i32, ptr: i32, len: i32);
    fn emit_event(ptr: i32, len: i32) -> i32;
}

/// Emit an event to the host (it copies the bytes synchronously during this call).
fn emit(ev: &Event) {
    let bytes = serde_json::to_vec(ev).unwrap_or_default();
    unsafe { emit_event(bytes.as_ptr() as i32, bytes.len() as i32) };
}

/// Optional structured log to the host.
#[allow(dead_code)]
fn host_log(level: i32, msg: &str) {
    unsafe { log(level, msg.as_ptr() as i32, msg.len() as i32) };
}

/// Pack a heap buffer's (ptr, len) into the i64 the host unpacks (ptr high 32 bits, len low 32).
fn pack(bytes: Vec<u8>) -> i64 {
    let ptr = bytes.as_ptr() as u64;
    let len = bytes.len() as u64;
    std::mem::forget(bytes); // host reads it from our memory; the Store is dropped after the call
    ((ptr << 32) | (len & 0xffff_ffff)) as i64
}

#[no_mangle]
pub extern "C" fn heldar_abi_version() -> i32 {
    1
}

/// Host calls this to write input before `heldar_handle`. A fresh Store per call means we can leak.
#[no_mangle]
pub extern "C" fn heldar_alloc(len: i32) -> i32 {
    let mut buf = Vec::<u8>::with_capacity(len.max(0) as usize);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr as i32
}

#[no_mangle]
pub extern "C" fn heldar_describe() -> i64 {
    pack(serde_json::to_vec(&manifest()).unwrap_or_default())
}

#[no_mangle]
pub extern "C" fn heldar_handle(ptr: i32, len: i32) -> i32 {
    let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, len.max(0) as usize) };
    let input: Input = match serde_json::from_slice(bytes) {
        Ok(i) => i,
        Err(_) => return 1, // non-zero = error
    };
    rule(&input);
    0
}
