#!/usr/bin/env bash
# Stage 2 validation: create an AI task, confirm THE SAMPLER PRODUCES A FRAME FROM A LIVE STREAM,
# that the worker contract endpoints work, and that detection ingestion round-trips.
#
# Runs against the synthetic stack (scripts/validate_subsystems.sh), which publishes a real RTSP
# stream for the whole run. CI excluded this script for years on the grounds that it "needs a REAL
# camera actively streaming ... cannot be satisfied synthetically" — that stopped being true once the
# subsystem stack existed: it publishes a live stream, `ai_enabled` defaults to true
# (config.rs), and the sampler falls back to the record URL when a camera has no sub-stream
# (services/sampler.rs), which is exactly the synthetic camera's shape.
#
# The frame check below is the one that needed a live stream, and it is the point of the script: it
# is the only place anything asserts that the SAMPLER, rather than a hand-posted event, turned a
# stream into a frame.
set -u
API=http://127.0.0.1:8000
CAM="${CAM:-cam_192_168_0_2}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DATA="${HELDAR_DATA_DIR:-$ROOT/data}"
mkdir -p "$DATA"
REPORT="$DATA/validate_ai.txt"
: > "$REPORT"
log(){ echo "$@" | tee -a "$REPORT"; }
. "$ROOT/scripts/lib/assert.sh"

curl -s --retry 30 --retry-delay 1 --retry-connrefused -o /dev/null "$API/healthz" || { log "core down"; exit 1; }
log "core up"

log "## create AI task (detection, 5fps, 640px) on $CAM"
TASK=$(curl -s -X POST "$API/api/v1/cameras/$CAM/ai-tasks" -H 'content-type: application/json' \
  -d '{"task_type":"detection","fps":5,"width":640,"config":{"model":"yolo-demo"}}')
echo "$TASK" | python3 -m json.tool 2>/dev/null | tee -a "$REPORT"
TASK_ID=$(echo "$TASK" | python3 -c 'import sys,json; print(json.load(sys.stdin).get("id",""))' 2>/dev/null)
assert_contains "$TASK_ID" "ai_" "the AI task was created and has an id"

log "## worker discovery: GET /api/v1/ai/tasks"
TASKS=$(curl -s "$API/api/v1/ai/tasks")
echo "$TASKS" | python3 -m json.tool 2>/dev/null | tee -a "$REPORT"
# A worker that cannot see the task cannot lease it, so the rest of the pipeline is unreachable.
assert_contains "$TASKS" "$CAM" "the task is visible to a worker on /api/v1/ai/tasks"

log "## wait for sampler to produce a frame"
FRAME_OK=0
for i in $(seq 1 25); do
  code=$(curl -s -o "$DATA/ai_frame.jpg" -w '%{http_code}' "$API/api/v1/cameras/$CAM/frame")
  if [ "$code" = "200" ]; then FRAME_OK=1; break; fi
  sleep 1
done
log "frame http=$code ok=$FRAME_OK"
file "$DATA/ai_frame.jpg" 2>/dev/null | tee -a "$REPORT"
# THE ASSERTION THIS SCRIPT EXISTS FOR. Everything else here can be satisfied by posting JSON at the
# API; this is the only check that something decoded a live stream into a frame. It is also the one
# the old CI comment claimed could not be satisfied synthetically.
assert_eq "1" "$FRAME_OK" "the sampler produced a frame from the live stream"
# A 200 carrying zero bytes would pass the check above. JPEG magic is what makes it a frame.
FRAME_MAGIC=$(head -c2 "$DATA/ai_frame.jpg" 2>/dev/null | od -An -tx1 | tr -d ' \n')
assert_eq "ffd8" "$FRAME_MAGIC" "the frame is a JPEG, not an empty 200"
log "frame age header:"
curl -s -D - -o /dev/null "$API/api/v1/cameras/$CAM/frame" | grep -i '^x-frame' | tee -a "$REPORT"

log "## sampler status"
curl -s "$API/api/v1/ai/samplers" | python3 -m json.tool 2>/dev/null | tee -a "$REPORT"

log "## ingest a detection + event (simulating an AI worker)"
curl -s -X POST "$API/api/v1/ai/events" -H 'content-type: application/json' -d "{
  \"camera_id\":\"$CAM\",\"task_type\":\"detection\",
  \"detections\":[{\"label\":\"person\",\"confidence\":0.91,\"bbox\":[0.1,0.2,0.25,0.5],\"track_id\":\"t1\"},
                  {\"label\":\"car\",\"confidence\":0.78,\"bbox\":[0.5,0.6,0.2,0.2]}],
  \"event\":{\"event_type\":\"ai_detection\",\"severity\":\"info\",\"payload\":{\"labels\":[\"person\",\"car\"]}}
}" | python3 -m json.tool 2>/dev/null | tee -a "$REPORT"

log "## query detections"
DETS=$(curl -s "$API/api/v1/cameras/$CAM/detections?limit=5")
echo "$DETS" | python3 -c 'import sys,json; d=json.load(sys.stdin); print(len(d),"detections"); [print(" ",x["label"],x["confidence"],x["bbox"]) for x in d]' 2>/dev/null | tee -a "$REPORT"
N_DETS=$(echo "$DETS" | python3 -c 'import sys,json; print(len(json.load(sys.stdin)))' 2>/dev/null || echo 0)
assert_ge "$N_DETS" 2 "both ingested detections round-trip through the query API"

log "## metrics (AI)"
# `heldar_detections_stored`, NOT `heldar_detections_total`: the exposition never emitted the latter,
# so this grep matched nothing and printed nothing, in a script with no assertions to notice.
AI_METRICS=$(curl -s "$API/metrics" | grep -E 'heldar_(ai_tasks_enabled|detections_stored)')
echo "$AI_METRICS" | tee -a "$REPORT"
assert_contains "$AI_METRICS" "heldar_ai_tasks_enabled" "the enabled AI task is visible in /metrics"
assert_contains "$AI_METRICS" "heldar_detections_stored" "stored detections are visible in /metrics"

log "DONE"
assert_summary
