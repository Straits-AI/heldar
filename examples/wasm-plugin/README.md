# wasm-plugin — a reference Heldar Wasm plugin

An example sandboxed Wasm guest (`heldar-occupancy-plugin`): it emits an `occupancy.high` event when a
detection batch contains many people. Build it with
`cargo build --release --target wasm32-unknown-unknown` (standalone crate, not part of the workspace),
then load the `.wasm` into a server built with `--features wasm`. Full build/load instructions and the
guest ABI: `website/docs/develop/wasm-plugins.md`.
