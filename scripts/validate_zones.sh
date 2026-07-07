#!/usr/bin/env bash
# Stage 3 zone-engine re-validation: debounce (2 confirming frames), server-time dwell, exit, and
# input validation. Assumes the stack is running and cam_192_168_0_2 is registered.
set -u
API=http://127.0.0.1:8000
CAM=cam_192_168_0_2
REPORT=/home/soh/cctv/data/validate_zones.txt
: > "$REPORT"
log(){ echo "$@" | tee -a "$REPORT"; }
post(){ curl -s -o /dev/null -w "%{http_code} " -X POST "$API/api/v1/ai/events" -H 'content-type: application/json' -d "$1"; }
det(){ # bbox bottom-center; $1=x $2=y $3=w $4=h
  echo "{\"camera_id\":\"$CAM\",\"task_type\":\"detection\",\"detections\":[{\"label\":\"person\",\"track_id\":\"tz\",\"bbox\":[$1,$2,$3,$4]}]}"
}

curl -s --retry 40 --retry-delay 1 --retry-connrefused -o /dev/null "$API/healthz" || { log "core down"; exit 1; }
log "core up"

log "## validation: 2-point polygon -> 400"
log "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$API/api/v1/cameras/$CAM/zones" -H 'content-type: application/json' -d '{"name":"bad","polygon":[[0.1,0.1],[0.2,0.2]]}')"
log "## validation: coord >1 -> 400"
log "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$API/api/v1/cameras/$CAM/zones" -H 'content-type: application/json' -d '{"name":"bad","polygon":[[0.1,0.1],[1.5,0.2],[0.3,0.4]]}')"
log "## validation: labels not strings -> 400"
log "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$API/api/v1/cameras/$CAM/zones" -H 'content-type: application/json' -d '{"name":"bad","polygon":[[0.5,0],[1,0],[1,1],[0.5,1]],"labels":["",123]}')"

log "## create valid zone (right half, dwell 1s, labels=[person], default confirm=2)"
ZID=$(curl -s -X POST "$API/api/v1/cameras/$CAM/zones" -H 'content-type: application/json' \
  -d '{"name":"Restricted-2","kind":"restricted","polygon":[[0.5,0.0],[1.0,0.0],[1.0,1.0],[0.5,1.0]],"dwell_seconds":1,"labels":["person"],"severity":"warning"}' \
  | python3 -c 'import sys,json;print(json.load(sys.stdin)["id"])')
log "zone: $ZID"

log "## debounce: 1 inside frame should NOT enter yet"
post "$(det 0.6 0.4 0.1 0.2)"   # inside (bottom-center 0.65,0.6)
log ""
n1=$(curl -s "$API/api/v1/cameras/$CAM/zone-events?zone_id=$ZID" | python3 -c 'import sys,json;print(len(json.load(sys.stdin)))')
log "events after 1 inside frame: $n1 (expect 0 — debounced)"

log "## 2nd inside frame -> ENTER"
post "$(det 0.6 0.4 0.1 0.2)"; log ""
sleep 1.5
log "## inside frame after 1.5s -> DWELL (server-time)"
post "$(det 0.6 0.4 0.1 0.2)"; log ""
log "## 2 outside frames -> EXIT"
post "$(det 0.1 0.1 0.1 0.1)"; post "$(det 0.1 0.1 0.1 0.1)"; log ""

log "## zone-events for this zone:"
curl -s "$API/api/v1/cameras/$CAM/zone-events?zone_id=$ZID" | python3 -c 'import sys,json; d=json.load(sys.stdin); [print(" ",e["event_type"],"dwell",e.get("dwell_seconds"),"evidence",bool(e.get("evidence_path"))) for e in d]'
log "DONE"
