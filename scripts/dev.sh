#!/usr/bin/env bash
# Run the VisionOps Core dev stack: MediaMTX + the Rust control plane.
# Build the core first with: cargo build --manifest-path apps/core/Cargo.toml
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MTX="$ROOT/infra/mediamtx/mediamtx"
CORE_BIN="$ROOT/apps/core/target/debug/visionops-core"

[ -x "$MTX" ] || { echo "MediaMTX missing; run scripts/setup_mediamtx.sh"; exit 1; }
[ -x "$CORE_BIN" ] || { echo "core not built; run: cargo build --manifest-path apps/core/Cargo.toml"; exit 1; }

cleanup() { kill "${MTX_PID:-0}" 2>/dev/null || true; }
trap cleanup EXIT

echo "Starting MediaMTX..."
"$MTX" "$ROOT/infra/mediamtx/mediamtx.yml" &
MTX_PID=$!

echo "Starting VisionOps Core..."
cd "$ROOT"
exec "$CORE_BIN"
