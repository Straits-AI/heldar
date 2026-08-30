#!/usr/bin/env bash
# Stage 3 zone-engine re-validation: debounce (2 confirming frames), server-time dwell, exit, and
# input validation. Assumes the stack is running and cam_192_168_0_2 is registered.
set -u
API=http://127.0.0.1:8000
CAM="${CAM:-cam_192_168_0_2}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DATA="${HELDAR_DATA_DIR:-$ROOT/data}"
mkdir -p "$DATA"
REPORT="$DATA/validate_zones.txt"
: > "$REPORT"
log(){ echo "$@" | tee -a "$REPORT"; }
. "$ROOT/scripts/lib/assert.sh"
post(){ curl -s -o /dev/null -w "%{http_code} " -X POST "$API/api/v1/ai/events" -H 'content-type: application/json' -d "$1"; }
det(){ # bbox bottom-center; $1=x $2=y $3=w $4=h
  # `confidence` is required in practice: zones apply a 0.5 floor by default (DEFAULT_MIN_CONFIDENCE
  # — "noise far more often than signal"), and a detection without it sits below that floor, so every
  # frame was silently discarded and no zone event could ever fire.
  echo "{\"camera_id\":\"$CAM\",\"task_type\":\"detection\",\"detections\":[{\"label\":\"person\",\"track_id\":\"tz\",\"confidence\":0.9,\"bbox\":[$1,$2,$3,$4]}]}"
}

curl -s --retry 40 --retry-delay 1 --retry-connrefused -o /dev/null "$API/healthz" || { log "core down"; exit 1; }
log "core up"

assert_eq 400 "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$API/api/v1/cameras/$CAM/zones" -H 'content-type: application/json' -d '{"name":"bad","polygon":[[0.1,0.1],[0.2,0.2]]}')" "2-point polygon rejected"
assert_eq 400 "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$API/api/v1/cameras/$CAM/zones" -H 'content-type: application/json' -d '{"name":"bad","polygon":[[0.1,0.1],[1.5,0.2],[0.3,0.4]]}')" "out-of-range coord rejected"
assert_eq 400 "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$API/api/v1/cameras/$CAM/zones" -H 'content-type: application/json' -d '{"name":"bad","polygon":[[0.5,0],[1,0],[1,1],[0.5,1]],"labels":["",123]}')" "non-string label rejected"

log "## create valid zone (right half, dwell 1s, labels=[person], default confirm=2)"
ZID=$(curl -s -X POST "$API/api/v1/cameras/$CAM/zones" -H 'content-type: application/json' \
  -d '{"name":"Restricted-2","kind":"region","polygon":[[0.5,0.0],[1.0,0.0],[1.0,1.0],[0.5,1.0]],"dwell_seconds":1,"labels":["person"],"severity":"warning"}' \
  | python3 -c 'import sys,json;print(json.load(sys.stdin).get("id",""))')
# `kind` was "restricted" until this was asserted. The API takes region|presence|line, so creation had
# been 400ing and every later check ran against an empty zone id — silently, because nothing here
# checked anything. That is the failure mode this whole change is about.
assert_contains "$ZID" "zone_" "zone was actually created"
log "zone: $ZID"

log "## debounce: 1 inside frame should NOT enter yet"
post "$(det 0.6 0.4 0.1 0.2)"   # inside (bottom-center 0.65,0.6)
log ""
n1=$(curl -s "$API/api/v1/cameras/$CAM/zone-events?zone_id=$ZID" | python3 -c 'import sys,json;print(len(json.load(sys.stdin)))')
# The debounce is the whole point of the zone engine: one frame must not raise an event, or every
# flicker becomes an alert.
assert_eq 0 "$n1" "one inside frame does not enter (debounced)"

log "## 2nd inside frame -> ENTER"
post "$(det 0.6 0.4 0.1 0.2)"; log ""
sleep 1.5
log "## inside frame after 1.5s -> DWELL (server-time)"
post "$(det 0.6 0.4 0.1 0.2)"; log ""
log "## 2 outside frames -> EXIT"
post "$(det 0.1 0.1 0.1 0.1)"; post "$(det 0.1 0.1 0.1 0.1)"; log ""

log "## zone-events for this zone:"
EVENTS=$(curl -s "$API/api/v1/cameras/$CAM/zone-events?zone_id=$ZID")
echo "$EVENTS" | python3 -c 'import sys,json; d=json.load(sys.stdin); [print(" ",e["event_type"],"dwell",e.get("dwell_seconds"),"evidence",bool(e.get("evidence_path"))) for e in d]' | tee -a "$REPORT"
TYPES=$(echo "$EVENTS" | python3 -c 'import sys,json;print(",".join(e["event_type"] for e in json.load(sys.stdin)))')
# The engine's contract: two confirming frames enter, server-side time produces a dwell, two outside
# frames exit. A transcript showing none of these used to read exactly like one showing all three.
assert_contains "$TYPES" "enter" "ENTER raised after 2 confirming frames"
assert_contains "$TYPES" "dwell" "DWELL raised from server-side time"
assert_contains "$TYPES" "exit"  "EXIT raised after 2 outside frames"
log "DONE"
assert_summary
