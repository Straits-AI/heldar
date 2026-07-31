#!/usr/bin/env bash
# Run the full Heldar stack (MediaMTX + core + Vite dashboard) for interactive/browser testing.
# Stays up for up to 30 minutes, then auto-stops. Conservative recording limits for the dev host.
#
# Paths are resolved from this script's location, so it works in any clone. Override the data
# directory with HELDAR_DATA_DIR=... and the auto-stop with STACK_TTL_SECS=...
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MTX="$ROOT/infra/mediamtx/mediamtx"
CORE="$ROOT/target/debug/heldar-core"
DATA="${HELDAR_DATA_DIR:-$ROOT/data}"
TTL="${STACK_TTL_SECS:-1800}"

# Fail loudly on a missing prerequisite — a half-up stack that prints "stack up" is worse than an error.
[ -x "$MTX" ]  || { echo "MediaMTX missing at $MTX — run: scripts/setup_mediamtx.sh" >&2; exit 1; }
[ -x "$CORE" ] || { echo "core not built at $CORE — run: cargo build --workspace" >&2; exit 1; }
[ -d "$ROOT/apps/web/node_modules" ] || { echo "dashboard deps missing — run: cd apps/web && npm ci" >&2; exit 1; }

LOG="$DATA/stack_logs"; mkdir -p "$LOG"
MTX_PID=""; CORE_PID=""; VITE_PID=""
cleanup() {
  [ -n "$VITE_PID" ] && kill "$VITE_PID" 2>/dev/null
  pkill -f 'node_modules/.bin/vite' 2>/dev/null
  [ -n "$CORE_PID" ] && kill "$CORE_PID" 2>/dev/null
  sleep 2
  [ -n "$MTX_PID" ] && kill "$MTX_PID" 2>/dev/null
}
trap cleanup EXIT TERM INT

"$MTX" "$ROOT/infra/mediamtx/mediamtx.yml" >"$LOG/mediamtx.log" 2>&1 &
MTX_PID=$!
sleep 2

HELDAR_DATA_DIR="$DATA" \
HELDAR_MAX_RECORDINGS_GB=3 \
HELDAR_DEFAULT_RETENTION_HOURS=2 \
HELDAR_LOG="info,heldar_core=info" \
"$CORE" >"$LOG/core.log" 2>&1 &
CORE_PID=$!

( cd "$ROOT/apps/web" && npm run dev >"$LOG/vite.log" 2>&1 ) &
VITE_PID=$!

echo "stack up: mediamtx=$MTX_PID core=$CORE_PID vite=$VITE_PID (auto-stop in ${TTL}s)"
echo "  core:      http://localhost:8000"
echo "  dashboard: http://localhost:5173   (logs in $LOG)"
sleep "$TTL"
