#!/usr/bin/env bash
# Lightweight full-stack smoke: MediaMTX + synthetic camera + core + Vite dev server.
# Confirms the dashboard serves the SPA and proxies /api to the control plane.
set -u
ROOT=/home/soh/cctv
MTX="$ROOT/infra/mediamtx/mediamtx"
CORE="$ROOT/target/debug/heldar-core"
REPORT="$ROOT/data/web_smoke.txt"
LOG="$ROOT/data/web_logs"; mkdir -p "$LOG"
: > "$REPORT"
log(){ echo "$@" | tee -a "$REPORT"; }

MTX_PID=""; SYNTH_PID=""; CORE_PID=""; VITE_PID=""
cleanup(){
  log "--- cleanup ---"
  [ -n "$VITE_PID" ] && kill "$VITE_PID" 2>/dev/null
  pkill -f 'vite' 2>/dev/null
  [ -n "$CORE_PID" ] && kill "$CORE_PID" 2>/dev/null; sleep 2
  [ -n "$SYNTH_PID" ] && kill "$SYNTH_PID" 2>/dev/null
  [ -n "$MTX_PID" ] && kill "$MTX_PID" 2>/dev/null
  pkill -f 'rtsp://127.0.0.1:8554/cam_test' 2>/dev/null
}
trap cleanup EXIT
rm -rf "$ROOT/data/recordings/synth_cam" "$ROOT/data/heldar.db"* 2>/dev/null

"$MTX" "$ROOT/infra/mediamtx/mediamtx.yml" >"$LOG/mtx.log" 2>&1 & MTX_PID=$!
sleep 2
ffmpeg -nostdin -hide_banner -loglevel warning -re -f lavfi -i "testsrc=size=1280x720:rate=15" \
  -c:v libx264 -preset ultrafast -tune zerolatency -g 30 -pix_fmt yuv420p \
  -f rtsp -rtsp_transport tcp rtsp://127.0.0.1:8554/cam_test >"$LOG/synth.log" 2>&1 & SYNTH_PID=$!
sleep 2
HELDAR_DEFAULT_SEGMENT_SECONDS=10 HELDAR_DATA_DIR="$ROOT/data" "$CORE" >"$LOG/core.log" 2>&1 & CORE_PID=$!
for _ in $(seq 1 30); do curl -fsS localhost:8000/healthz >/dev/null 2>&1 && break; sleep 1; done
curl -fsS -X POST localhost:8000/api/v1/cameras -H 'content-type: application/json' \
  -d '{"id":"synth_cam","name":"Synthetic Test Camera","main_stream_url":"rtsp://127.0.0.1:8554/cam_test","segment_seconds":10}' >/dev/null 2>&1

( cd "$ROOT/apps/web" && npm run dev >"$LOG/vite.log" 2>&1 ) & VITE_PID=$!
VOK=0; for _ in $(seq 1 40); do curl -fsS localhost:5173/ >/dev/null 2>&1 && { VOK=1; break; }; sleep 1; done
log "vite dev up: $VOK"
log "## index.html (head)"; curl -fsS localhost:5173/ 2>&1 | head -c 500 | tee -a "$REPORT"; log ""
log "## /api/v1/system via vite proxy"; curl -fsS localhost:5173/api/v1/system 2>&1 | tee -a "$REPORT"; log ""
log "## /api/v1/cameras via vite proxy"; curl -fsS localhost:5173/api/v1/cameras 2>&1 | tee -a "$REPORT"; log ""
log "## vite dev log tail"; tail -n 8 "$LOG/vite.log" | tee -a "$REPORT"
log "DONE"
