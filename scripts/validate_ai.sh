#!/usr/bin/env bash
# Stage 2 validation: create an AI task on a real camera, confirm the sampler produces frames, the
# worker contract endpoints work, and detection ingestion round-trips. Assumes the stack is running
# and camera cam_192_168_0_2 is registered.
set -u
API=http://127.0.0.1:8000
CAM=cam_192_168_0_2
REPORT=/home/soh/cctv/data/validate_ai.txt
: > "$REPORT"
log(){ echo "$@" | tee -a "$REPORT"; }

curl -s --retry 30 --retry-delay 1 --retry-connrefused -o /dev/null "$API/healthz" || { log "core down"; exit 1; }
log "core up"

log "## create AI task (detection, 5fps, 640px) on $CAM"
curl -s -X POST "$API/api/v1/cameras/$CAM/ai-tasks" -H 'content-type: application/json' \
  -d '{"task_type":"detection","fps":5,"width":640,"config":{"model":"yolo-demo"}}' \
  | python3 -m json.tool 2>/dev/null | tee -a "$REPORT"

log "## worker discovery: GET /api/v1/ai/tasks"
curl -s "$API/api/v1/ai/tasks" | python3 -m json.tool 2>/dev/null | tee -a "$REPORT"

log "## wait for sampler to produce a frame"
FRAME_OK=0
for i in $(seq 1 25); do
  code=$(curl -s -o /home/soh/cctv/data/ai_frame.jpg -w '%{http_code}' "$API/api/v1/cameras/$CAM/frame")
  if [ "$code" = "200" ]; then FRAME_OK=1; break; fi
  sleep 1
done
log "frame http=$code ok=$FRAME_OK"
file /home/soh/cctv/data/ai_frame.jpg 2>/dev/null | tee -a "$REPORT"
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
curl -s "$API/api/v1/cameras/$CAM/detections?limit=5" | python3 -c 'import sys,json; d=json.load(sys.stdin); print(len(d),"detections"); [print(" ",x["label"],x["confidence"],x["bbox"]) for x in d]' 2>/dev/null | tee -a "$REPORT"

log "## metrics (AI)"
curl -s "$API/metrics" | grep -E 'heldar_(ai_tasks_enabled|detections_total)' | tee -a "$REPORT"

log "DONE"
