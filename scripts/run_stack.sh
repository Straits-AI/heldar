#!/usr/bin/env bash
# Run the full Heldar stack (MediaMTX + core + Vite dashboard) for interactive/browser testing.
# Stays up for up to 30 minutes, then auto-stops. Conservative recording limits for the dev host.
set -u
ROOT=/home/soh/cctv
LOG="$ROOT/data/stack_logs"; mkdir -p "$LOG"
MTX_PID=""; CORE_PID=""; VITE_PID=""
cleanup() {
  [ -n "$VITE_PID" ] && kill "$VITE_PID" 2>/dev/null
  pkill -f 'node_modules/.bin/vite' 2>/dev/null
  [ -n "$CORE_PID" ] && kill "$CORE_PID" 2>/dev/null
  sleep 2
  [ -n "$MTX_PID" ] && kill "$MTX_PID" 2>/dev/null
}
trap cleanup EXIT TERM INT

"$ROOT/infra/mediamtx/mediamtx" "$ROOT/infra/mediamtx/mediamtx.yml" >"$LOG/mediamtx.log" 2>&1 &
MTX_PID=$!
sleep 2

HELDAR_DATA_DIR="$ROOT/data" \
HELDAR_MAX_RECORDINGS_GB=3 \
HELDAR_DEFAULT_RETENTION_HOURS=2 \
HELDAR_LOG="info,heldar_core=info" \
"$ROOT/target/debug/heldar-core" >"$LOG/core.log" 2>&1 &
CORE_PID=$!

( cd "$ROOT/apps/web" && npm run dev >"$LOG/vite.log" 2>&1 ) &
VITE_PID=$!

echo "stack up: mediamtx=$MTX_PID core=$CORE_PID vite=$VITE_PID (auto-stop in 1800s)"
sleep 1800
