#!/usr/bin/env bash
# Boots a full Heldar stack for the Playwright UI e2e: MediaMTX + N synthetic RTSP cameras + the core
# (serving the built dashboard) with one camera AI-enabled. Isolated DB/data under /tmp so it never
# touches the dev DB. Stays in the foreground (Playwright's `webServer` keeps it alive); on TERM/EXIT
# it tears down every child. No real cameras, no credentials.
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MTX="$ROOT/infra/mediamtx/mediamtx"
CORE="$ROOT/target/debug/heldar-core"
PORT="${E2E_PORT:-8011}"          # dedicated port so the test stack never collides with a dev core on :8000
API="http://127.0.0.1:$PORT"
NCAMS="${E2E_CAMS:-6}"            # 6 cameras → 2x2 paginates (2 pages), 3x3 is one page
DATA="/tmp/heldar-e2e"
LOG="$DATA/logs"; mkdir -p "$LOG"
rm -f "$DATA/heldar.db"* 2>/dev/null; rm -rf "$DATA/recordings" "$DATA/frames" 2>/dev/null

PIDS=()
cleanup() {
  for p in "${PIDS[@]:-}"; do kill "$p" 2>/dev/null; done
  pkill -f 'rtsp://127.0.0.1:8554/cam_e2e_' 2>/dev/null
  sleep 1
}
trap cleanup EXIT INT TERM

# Free the ports + kill leftovers from a previous run that may have been killed before its trap ran,
# so every boot starts clean (the core port + MediaMTX's RTSP/API ports, and the synthetic publishers).
fuser -k "${PORT}/tcp" 8554/tcp 9997/tcp 2>/dev/null || true
pkill -9 -f 'rtsp://127.0.0.1:8554/cam_e2e_' 2>/dev/null || true
sleep 1

echo "[e2e_stack] MediaMTX"
"$MTX" "$ROOT/infra/mediamtx/mediamtx.yml" >"$LOG/mediamtx.log" 2>&1 & PIDS+=($!)
sleep 2

echo "[e2e_stack] $NCAMS synthetic cameras"
for i in $(seq 1 "$NCAMS"); do
  # testsrc has a built-in moving pattern + frame counter (motion for the AI task); no drawtext, since
  # that filter needs libfreetype which isn't in every ffmpeg build.
  ffmpeg -nostdin -hide_banner -loglevel error -re \
    -f lavfi -i "testsrc=size=640x360:rate=10" \
    -c:v libx264 -preset ultrafast -tune zerolatency -g 20 -pix_fmt yuv420p \
    -f rtsp -rtsp_transport tcp "rtsp://127.0.0.1:8554/cam_e2e_${i}" >"$LOG/cam_${i}.log" 2>&1 & PIDS+=($!)
done
sleep 3

echo "[e2e_stack] core (isolated DB under $DATA, port $PORT)"
HELDAR_DATABASE_URL="sqlite://$DATA/heldar.db" \
HELDAR_DATA_DIR="$DATA" \
HELDAR_API_PORT="$PORT" \
HELDAR_WEB_DIR="$ROOT/apps/web/dist" \
HELDAR_DEFAULT_SEGMENT_SECONDS=5 \
HELDAR_INDEXER_INTERVAL_S=3 \
HELDAR_HEALTH_INTERVAL_S=5 \
HELDAR_AI_ENABLED=true HELDAR_DEFAULT_AI_FPS=2 \
"$CORE" >"$LOG/core.log" 2>&1 & PIDS+=($!)

# wait for the API
for _ in $(seq 1 40); do curl -fsS "$API/healthz" >/dev/null 2>&1 && break; sleep 1; done
curl -fsS "$API/healthz" >/dev/null 2>&1 || { echo "[e2e_stack] core did not start"; tail -20 "$LOG/core.log"; exit 1; }

echo "[e2e_stack] registering $NCAMS cameras"
for i in $(seq 1 "$NCAMS"); do
  curl -fsS -X POST "$API/api/v1/cameras" -H 'content-type: application/json' -d "{
    \"id\":\"cam_e2e_${i}\",\"name\":\"E2E Camera ${i}\",\"vendor\":\"generic\",
    \"main_stream_url\":\"rtsp://127.0.0.1:8554/cam_e2e_${i}\",\"record_stream\":\"main\",
    \"segment_seconds\":5,\"retention_hours\":1
  }" >/dev/null || true
done
# one camera gets a motion AI task so the AI page + perception path have data
curl -fsS -X POST "$API/api/v1/cameras/cam_e2e_1/ai-tasks" -H 'content-type: application/json' \
  -d '{"task_type":"motion","fps":2,"width":480,"enabled":true,"config":{"threshold":0.0008,"pixel_delta":6}}' >/dev/null 2>&1 || true

echo "[e2e_stack] ready: $NCAMS cameras on $API (dashboard served). Waiting…"
wait "${PIDS[-1]}"
