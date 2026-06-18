//! Sandboxed Wasm plugin composition seam — isolated from the server core, mirroring `verticals.rs`.
//!
//! `main.rs` calls these functions unconditionally; all `heldar-wasm`/wasmi references live HERE,
//! behind the `wasm` Cargo feature. When the feature is off (the default lean appliance) both functions
//! are no-ops returning empty vecs and reference no Wasm runtime — so `main.rs` is identical and the
//! binary never links wasmi.

use std::sync::Arc;

use heldar_kernel::modules::ModuleManifest;
use heldar_kernel::services::consumer::DetectionConsumer;
use sqlx::SqlitePool;

/// Load Wasm plugins from `HELDAR_WASM_PLUGINS_DIR` (default `<data_dir>/wasm-plugins`), returning the
/// consumers to register + their headless manifests. No-op when the `wasm` feature is off, or when
/// `HELDAR_WASM_ENABLED` is false.
#[cfg(feature = "wasm")]
pub fn load(
    pool: &SqlitePool,
    data_dir: &std::path::Path,
    reserved: &[String],
) -> (Vec<Arc<dyn DetectionConsumer>>, Vec<ModuleManifest>) {
    let enabled = std::env::var("HELDAR_WASM_ENABLED")
        .map(|v| v != "false" && v != "0")
        .unwrap_or(true);
    if !enabled {
        return (Vec::new(), Vec::new());
    }
    let dir = std::env::var("HELDAR_WASM_PLUGINS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| data_dir.join("wasm-plugins"));
    heldar_wasm::load_dir(&dir, pool.clone(), reserved)
}

#[cfg(not(feature = "wasm"))]
pub fn load(
    _pool: &SqlitePool,
    _data_dir: &std::path::Path,
    _reserved: &[String],
) -> (Vec<Arc<dyn DetectionConsumer>>, Vec<ModuleManifest>) {
    (Vec::new(), Vec::new())
}
