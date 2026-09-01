#!/usr/bin/env bash
# Boot ONE synthetic stack and run the zone, entry, movement and search validations against it.
#
# Those four scripts were written against a running dev box: they assume a stack on :8000 and a real
# camera already registered, which is exactly why CI skipped them (see the note in ci.yml). Their
# event data was always synthetic — only the harness assumption was not. They already take the camera
# id from `$CAM`, so they need no changes; they need a stack.
#
# This is that stack, booted once and shared, because booting five is five times the flake surface for
# no extra coverage. It deliberately mirrors validate.sh's sequence (MediaMTX, then core, then the
# publisher — MediaMTX delegates publish auth to the kernel, so a publisher started first gets 401 and
# ffmpeg exits).
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DATA="${HELDAR_DATA_DIR:-$ROOT/data}"
MTX="$ROOT/infra/mediamtx/mediamtx"
CORE="$ROOT/target/debug/heldar-core"
API=http://127.0.0.1:8000
LOGDIR="$DATA"
REPORT="$DATA/validate_subsystems.txt"
CAM=synth_cam

mkdir -p "$DATA"
: >"$REPORT"
log(){ echo "$*" | tee -a "$REPORT"; }
hr(){ log ""; log "== $* =="; }

cleanup(){
  hr cleanup
  for pid in "${ANALYZER_PID:-}" "${SYNTH_PID:-}" "${SYNTH2_PID:-}" "${CORE_PID:-}" "${MTX_PID:-}"; do
    [ -n "$pid" ] && kill "$pid" 2>/dev/null
  done
  sleep 3   # let the recorder's ffmpeg children go with it
}
trap cleanup EXIT

rm -rf "$DATA/recordings/$CAM" "$DATA/heldar.db"* 2>/dev/null

hr "start MediaMTX"
"$MTX" "$ROOT/infra/mediamtx/mediamtx.yml" >"$LOGDIR/subsys-mediamtx.log" 2>&1 &
MTX_PID=$!
sleep 2

hr "start Heldar Core"
HELDAR_DEFAULT_SEGMENT_SECONDS=5 \
HELDAR_DATA_DIR="$DATA" \
HELDAR_INDEXER_INTERVAL_S=3 \
HELDAR_HEALTH_INTERVAL_S=10 \
HELDAR_RETENTION_INTERVAL_S=60 \
HELDAR_LOG="info,heldar_core=debug" \
"$CORE" >"$LOGDIR/subsys-core.log" 2>&1 &
CORE_PID=$!

UP=0
for _ in $(seq 1 30); do
  curl -fsS "$API/healthz" >/dev/null 2>&1 && { UP=1; break; }
  sleep 1
done
[ "$UP" = 1 ] || { log "API DID NOT START"; tail -n 30 "$LOGDIR/subsys-core.log" | tee -a "$REPORT"; exit 1; }

hr "start synthetic camera"
ffmpeg -nostdin -hide_banner -loglevel warning -re \
  -f lavfi -i "testsrc=size=640x360:rate=10" \
  -c:v libx264 -preset ultrafast -tune zerolatency -g 20 -pix_fmt yuv420p \
  -f rtsp -rtsp_transport tcp rtsp://127.0.0.1:8554/cam_test >"$LOGDIR/subsys-synth.log" 2>&1 &
SYNTH_PID=$!
sleep 3
kill -0 "$SYNTH_PID" 2>/dev/null || { log "SYNTHETIC CAMERA DIED"; tail -n 10 "$LOGDIR/subsys-synth.log" | tee -a "$REPORT"; exit 1; }

# A SECOND streaming camera (#113 criterion 1). Cross-camera movement was proven over two cameras
# of which only one existed as a stream — the other was a row created through the API with no
# publisher behind it. A transit between a real camera and a row is not a transit.
hr "start second synthetic camera"
ffmpeg -nostdin -hide_banner -loglevel warning -re \
  -f lavfi -i "testsrc=size=640x360:rate=10" \
  -c:v libx264 -preset ultrafast -tune zerolatency -g 20 -pix_fmt yuv420p \
  -f rtsp -rtsp_transport tcp rtsp://127.0.0.1:8554/cam_test_b >"$LOGDIR/subsys-synth-b.log" 2>&1 &
SYNTH2_PID=$!
sleep 2
kill -0 "$SYNTH2_PID" 2>/dev/null || { log "SECOND SYNTHETIC CAMERA DIED"; tail -n 10 "$LOGDIR/subsys-synth-b.log" | tee -a "$REPORT"; exit 1; }

hr "register $CAM"
curl -fsS -X POST "$API/api/v1/cameras" -H 'content-type: application/json' -d "{
  \"id\":\"$CAM\",\"name\":\"Synthetic Subsystem Camera\",\"vendor\":\"generic\",
  \"main_stream_url\":\"rtsp://127.0.0.1:8554/cam_test\",\"record_stream\":\"main\",
  \"segment_seconds\":5,\"retention_hours\":24
}" | tee -a "$REPORT"; echo | tee -a "$REPORT"

hr "register ${CAM}_b"
curl -fsS -X POST "$API/api/v1/cameras" -H 'content-type: application/json' -d "{
  \"id\":\"${CAM}_b\",\"name\":\"Synthetic Subsystem Camera B\",\"vendor\":\"generic\",
  \"main_stream_url\":\"rtsp://127.0.0.1:8554/cam_test_b\",\"record_stream\":\"main\",
  \"segment_seconds\":5,\"retention_hours\":24
}" | tee -a "$REPORT"; echo | tee -a "$REPORT"

# A WORKER PROCESS in the stack (#113 criterion 1), not a validator pretending to be one.
#
# Deterministic by construction — a fixed detection, no model — so a failure here means the pipeline
# changed rather than that a detector saw something different. It leases, pulls ticketed frames and
# posts attributable detections for the whole run, which is what makes the stack COMPLETE rather than
# a recorder with an API in front of it.
# Its own AI task, on its own camera. A lease is EXCLUSIVE, so a worker left unconfined takes every
# eligible task and starves `validate_ai.sh`'s chain assertions — which is how this first went red for
# a reason that had nothing to do with the product.
curl -fsS -X POST "$API/api/v1/cameras/${CAM}_b/ai-tasks" -H 'content-type: application/json' \
  -d '{"task_type":"motion","fps":2,"width":320,"enabled":true,"config":{"model":"mock"}}' \
  | tee -a "$REPORT"; echo | tee -a "$REPORT"

hr "start mock AI worker"
# `motion`, so the lease filter keeps it off the `detection` task validate_ai.sh creates and leases
# for its own chain assertions. Confining a worker by camera is not possible — the lease API filters
# by task type, and filtering the RESULT leaves the tasks leased-but-unworked, which is worse.
python3 "$ROOT/scripts/mock_analyzer.py" "$API" --task-types motion --interval 3 \
  >"$LOGDIR/subsys-analyzer.log" 2>&1 &
ANALYZER_PID=$!
sleep 2
kill -0 "$ANALYZER_PID" 2>/dev/null || { log "MOCK ANALYZER DIED"; tail -n 10 "$LOGDIR/subsys-analyzer.log" | tee -a "$REPORT"; exit 1; }

# Each script owns its own assertions; this only reports which subsystem failed and keeps going, so
# one broken subsystem does not hide the state of the other three.
rc=0
# `ai` is in the loop as of #113: it was excluded on the grounds that it needed a real camera, and
# this stack publishes a live RTSP stream for the whole run — which is precisely what its frame check
# needs.
#
# LAST, not first. These validators share one camera, and validate_ai.sh ingests detections with a
# track id; running it ahead of validate_zones.sh left a track already established, and the zone
# debounce assertion ("one inside frame does not enter") saw 3 events instead of 0. The sampler needs
# no head start — it has been running since the stack came up.
for s in zones entry movement search ai; do
  hr "validate_$s.sh (CAM=$CAM)"
  if CAM="$CAM" "$ROOT/scripts/validate_$s.sh" >>"$REPORT" 2>&1; then
    log "OK   validate_$s.sh"
  else
    log "FAIL validate_$s.sh (exit $?)"
    rc=1
  fi
  # A script that ASSERTS NOTHING exits 0 no matter what the API said, so "it ran" must not be
  # reported as "it passed". Every one of these was a transcript before this change: validate_zones
  # had been sending a `kind` the API stopped accepting, creating no zone at all, and reporting
  # success the whole time. Demand the marker that only a real assertion run produces.
  if ! grep -q '^RESULT: PASS' "$DATA/validate_$s.txt" 2>/dev/null; then
    log "FAIL validate_$s.sh produced no 'RESULT: PASS' — it asserted nothing, so its OK means nothing"
    rc=1
  fi
  # These scripts report assertion failures as FAIL markers rather than a non-zero exit, so a green
  # exit code alone would not mean the subsystem passed.
  if grep -Eq '(^|[[:space:]])FAIL ' "$DATA/validate_$s.txt" 2>/dev/null; then
    log "FAIL validate_$s.sh reported FAIL assertions:"
    grep -E '(^|[[:space:]])FAIL ' "$DATA/validate_$s.txt" | tee -a "$REPORT"
    rc=1
  fi
done

# THE WORKER MUST HAVE WORKED. Starting a process and never checking it did anything is how a stack
# grows a component that has been dead for months — the same "it ran, so it passed" this gate refuses
# from the validators.
#
# `ticketed=True` is what is demanded, not merely a 200: under the default `warn` tier an unticketed
# ingest is still accepted, so a worker whose frame pulls stopped carrying tickets would keep posting
# happily and prove nothing about provenance.
hr "mock AI worker"
if kill -0 "$ANALYZER_PID" 2>/dev/null; then
  log "OK   the worker is still running"
else
  log "FAIL the mock analyzer exited during the run"
  tail -n 20 "$LOGDIR/subsys-analyzer.log" | tee -a "$REPORT"
  rc=1
fi
TICKETED=$(grep -c 'ticketed=True' "$LOGDIR/subsys-analyzer.log" 2>/dev/null || echo 0)
log "worker ticketed ingests: $TICKETED"
if [ "$TICKETED" -lt 1 ] 2>/dev/null; then
  log "FAIL the worker never completed a ticketed ingest — the lease -> frame -> ticket -> ingest \
path is broken, or no AI task existed for it to lease"
  tail -n 20 "$LOGDIR/subsys-analyzer.log" | tee -a "$REPORT"
  rc=1
fi

hr "result"
log "subsystem gate rc=$rc"
exit "$rc"
