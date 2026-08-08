#!/usr/bin/env bash
# End-to-end validation of the Heldar Core media kernel against a synthetic RTSP camera.
# Starts MediaMTX + a synthetic camera + the core server, exercises every Stage 0 capability,
# writes a report to data/validate_report.txt, and tears everything down.
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DATA="${HELDAR_DATA_DIR:-$ROOT/data}"
MTX="$ROOT/infra/mediamtx/mediamtx"
CORE="$ROOT/target/debug/heldar-core"
API=http://127.0.0.1:8000
REPORT="$DATA/validate_report.txt"
LOGDIR="$DATA/validate_logs"
mkdir -p "$LOGDIR" "$DATA"
: > "$REPORT"

log(){ echo "$@" | tee -a "$REPORT"; }
hr(){ log ""; log "----- $* -----"; }

# Assertion tracking so the script exits non-zero on a real validation failure (CI gate). Backward
# compatible for local use: local runs still print the full report and tear down; the only added
# behaviour is a non-zero exit + a "FAIL:" line when a core Stage 0 guarantee is not met.
FAILS=0
fail(){ FAILS=$((FAILS+1)); log "FAIL: $*"; }
check(){ # check <condition-desc> <actual> <expected>
  if [ "$2" = "$3" ]; then log "PASS: $1 ($2)"; else fail "$1 (got '$2', want '$3')"; fi
}

MTX_PID=""; SYNTH_PID=""; CORE_PID=""
cleanup(){
  hr "cleanup"
  [ -n "$CORE_PID" ] && kill "$CORE_PID" 2>/dev/null
  sleep 3   # allow graceful shutdown to kill recorder ffmpeg children
  [ -n "$SYNTH_PID" ] && kill "$SYNTH_PID" 2>/dev/null
  [ -n "$MTX_PID" ] && kill "$MTX_PID" 2>/dev/null
  pkill -f 'rtsp://127.0.0.1:8554/cam_test' 2>/dev/null
  sleep 1
  log "done."
}
trap cleanup EXIT

# Clean prior validation artifacts
rm -rf "$DATA/recordings/synth_cam" "$DATA/heldar.db"* 2>/dev/null
rm -f "$DATA/clips/"*.mp4 2>/dev/null

hr "start MediaMTX"
"$MTX" "$ROOT/infra/mediamtx/mediamtx.yml" >"$LOGDIR/mediamtx.log" 2>&1 &
MTX_PID=$!
sleep 2

hr "start Heldar Core (segment=5s, indexer=3s)"
HELDAR_DEFAULT_SEGMENT_SECONDS=5 \
HELDAR_DATA_DIR="$DATA" \
HELDAR_INDEXER_INTERVAL_S=3 \
HELDAR_HEALTH_INTERVAL_S=10 \
HELDAR_RETENTION_INTERVAL_S=60 \
HELDAR_LOG="info,heldar_core=debug" \
"$CORE" >"$LOGDIR/core.log" 2>&1 &
CORE_PID=$!

# wait for API
UP=0
for _ in $(seq 1 30); do
  if curl -fsS "$API/healthz" >/dev/null 2>&1; then UP=1; break; fi
  sleep 1
done
log "API up: $UP"
[ "$UP" = 1 ] || { log "API DID NOT START — core.log tail:"; tail -n 30 "$LOGDIR/core.log" | tee -a "$REPORT"; exit 1; }

# The synthetic publisher must start AFTER the core: MediaMTX delegates publish authorization to the
# kernel (`authMethod: http` -> /internal/mediamtx-auth), so publishing before the core is up gets a
# 401 and ffmpeg exits immediately.
hr "start synthetic camera (testsrc -> rtsp://127.0.0.1:8554/cam_test)"
ffmpeg -nostdin -hide_banner -loglevel warning -re \
  -f lavfi -i "testsrc=size=1280x720:rate=15" \
  -c:v libx264 -preset ultrafast -tune zerolatency -g 30 -pix_fmt yuv420p \
  -f rtsp -rtsp_transport tcp rtsp://127.0.0.1:8554/cam_test >"$LOGDIR/synth.log" 2>&1 &
SYNTH_PID=$!
sleep 3
kill -0 "$SYNTH_PID" 2>/dev/null \
  || { log "SYNTHETIC CAMERA DIED — synth.log tail:"; tail -n 10 "$LOGDIR/synth.log" | tee -a "$REPORT"; exit 1; }

hr "healthz"; curl -fsS "$API/healthz"; echo | tee -a "$REPORT"
hr "system (initial)"; curl -fsS "$API/api/v1/system" | tee -a "$REPORT"; echo | tee -a "$REPORT"

hr "create camera synth_cam"
curl -fsS -X POST "$API/api/v1/cameras" -H 'content-type: application/json' -d '{
  "id":"synth_cam","name":"Synthetic Test Camera","vendor":"generic",
  "main_stream_url":"rtsp://127.0.0.1:8554/cam_test","record_stream":"main",
  "segment_seconds":5,"retention_hours":24
}' | tee -a "$REPORT"; echo | tee -a "$REPORT"

hr "test camera connectivity"
curl -fsS -X POST "$API/api/v1/cameras/synth_cam/test" | tee -a "$REPORT"; echo | tee -a "$REPORT"

hr "recording for 22s"
sleep 22

hr "camera health"
curl -fsS "$API/api/v1/cameras/synth_cam/health" | tee -a "$REPORT"; echo | tee -a "$REPORT"

hr "segments"
SEGS=$(curl -fsS "$API/api/v1/cameras/synth_cam/segments")
echo "$SEGS" | tee -a "$REPORT"; echo | tee -a "$REPORT"

hr "timeline"
curl -fsS "$API/api/v1/cameras/synth_cam/timeline" | tee -a "$REPORT"; echo | tee -a "$REPORT"

RANGE=$(python3 - "$SEGS" <<'PY'
import sys, json
try:
    segs = json.loads(sys.argv[1])
except Exception:
    segs = []
if segs:
    print(segs[0]["start_time"] + " " + segs[-1]["end_time"])
PY
)
FROM=$(echo "$RANGE" | awk '{print $1}')
TO=$(echo "$RANGE" | awk '{print $2}')
SEG_COUNT=$(echo "$SEGS" | python3 -c 'import sys,json; print(len(json.load(sys.stdin)))' 2>/dev/null || echo 0)
log "recorded range: '$FROM' .. '$TO' (segment_count=$SEG_COUNT)"
# Core Stage 0 guarantee: the recorder produced at least one indexed segment from the synthetic camera.
if [ "${SEG_COUNT:-0}" -gt 0 ] 2>/dev/null; then log "PASS: recorder produced $SEG_COUNT segment(s)"; else fail "no recorded segments (recorder/indexer produced nothing)"; fi

if [ -n "$FROM" ]; then
  hr "snapshot at $FROM (recorded)"
  curl -sS "$API/api/v1/cameras/synth_cam/snapshot?at=$FROM" -o "$DATA/snap_recorded.jpg" \
    -w "http=%{http_code} bytes=%{size_download}\n" | tee -a "$REPORT"
  file "$DATA/snap_recorded.jpg" 2>/dev/null | tee -a "$REPORT"

  hr "clip export $FROM .. $TO"
  curl -sS -X POST "$API/api/v1/cameras/synth_cam/clip" -H 'content-type: application/json' \
    -d "{\"from\":\"$FROM\",\"to\":\"$TO\"}" -w "\n[http=%{http_code}]\n" | tee -a "$REPORT"
fi

hr "live snapshot (grab from stream now)"
SNAP_CODE=$(curl -sS "$API/api/v1/cameras/synth_cam/snapshot" -o "$DATA/snap_live.jpg" -w '%{http_code}')
SNAP_BYTES=$(wc -c <"$DATA/snap_live.jpg" 2>/dev/null | tr -d ' ')
log "http=$SNAP_CODE bytes=$SNAP_BYTES"
file "$DATA/snap_live.jpg" 2>/dev/null | tee -a "$REPORT"
# Core Stage 0 guarantee: a live JPEG snapshot can be grabbed from the running stream.
if [ "$SNAP_CODE" = "200" ] && file "$DATA/snap_live.jpg" 2>/dev/null | grep -qi 'JPEG'; then
  log "PASS: live snapshot is a JPEG"
else
  fail "live snapshot not a valid JPEG (http=$SNAP_CODE)"
fi

hr "liveview (register MediaMTX path)"
curl -sS "$API/api/v1/cameras/synth_cam/liveview" -w "\n[http=%{http_code}]\n" | tee -a "$REPORT"
hr "HLS playlist (head)"
HLS_CODE=000
for _ in $(seq 1 10); do
  HLS_CODE=$(curl -sSL -o /tmp/hls.m3u8 -w '%{http_code}' "http://127.0.0.1:8888/cam_synth_cam/index.m3u8")
  [ "$HLS_CODE" = "200" ] && break
  sleep 1
done
log "HLS http=$HLS_CODE"
head -c 400 /tmp/hls.m3u8 2>/dev/null | tee -a "$REPORT"; echo | tee -a "$REPORT"
# Core Stage 0 guarantee: the live-view HLS playlist is served for the registered camera.
check "HLS playlist served" "$HLS_CODE" "200"

hr "events (recent)"
curl -fsS "$API/api/v1/events?limit=20" | tee -a "$REPORT"; echo | tee -a "$REPORT"

hr "reconnect test: kill synthetic camera, expect recorder to notice"
kill "$SYNTH_PID" 2>/dev/null; SYNTH_PID=""
sleep 12
hr "camera health after stream loss"
curl -fsS "$API/api/v1/cameras/synth_cam/health" | tee -a "$REPORT"; echo | tee -a "$REPORT"

hr "system (final)"
curl -fsS "$API/api/v1/system" | tee -a "$REPORT"; echo | tee -a "$REPORT"

hr "on-disk artifacts"
log "recordings/synth_cam:"; ls -la "$DATA/recordings/synth_cam" 2>/dev/null | tee -a "$REPORT"
log "clips:"; ls -la "$DATA/clips" 2>/dev/null | grep -E '\.mp4$' | tee -a "$REPORT"

hr "VALIDATION COMPLETE"
# Exit non-zero if any core Stage 0 guarantee failed, so CI actually fails on a bad build. The EXIT
# trap (cleanup) still runs and preserves this status. Local runs are unaffected on success.
if [ "$FAILS" -gt 0 ]; then log "VALIDATION FAILED: $FAILS check(s) failed"; exit 1; fi
log "VALIDATION PASSED: all checks green"
