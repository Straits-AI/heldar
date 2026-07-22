#!/usr/bin/env bash
# Stage 6 Movement validation: topology link, vehicle ReID candidate (same plate on two linked
# cameras), red-zone breach incident, candidate/breach review workflows, and audited plate search.
# Assumes the stack is up. Auth off (default).
set -u
API=http://127.0.0.1:8000/api/v1
A=cam_192_168_0_2
Bc=cam_movement_b
REPORT=/home/soh/cctv/data/validate_movement.txt
: > "$REPORT"
log(){ echo "$@" | tee -a "$REPORT"; }
jqget(){ python3 -c "import sys,json;d=json.load(sys.stdin);print($1)"; }

anpr(){ # cam track plate color type  -> post one ANPR read
  curl -s -o /dev/null -X POST "$API/ai/events" -H 'content-type: application/json' -d "{
    \"camera_id\":\"$1\",\"task_type\":\"anpr\",
    \"detections\":[{\"label\":\"$5\",\"confidence\":0.9,\"track_id\":\"$2\",\"bbox\":[0.4,0.4,0.2,0.3],
      \"attributes\":{\"plate\":\"$3\",\"plate_confidence\":0.95,\"color\":\"$4\",\"vehicle_type\":\"$5\"}}]}"
}
commit(){ for i in 1 2 3 4; do anpr "$@"; done; }

curl -s --retry 30 --retry-delay 1 --retry-connrefused -o /dev/null "$API/../healthz" || { log down; exit 1; }
log "== core up =="
curl -s -o /dev/null -X POST "$API/cameras" -H 'content-type: application/json' -d "{\"id\":\"$A\",\"name\":\"Gate A\",\"vendor\":\"hikvision\"}"
curl -s -o /dev/null -X POST "$API/cameras" -H 'content-type: application/json' -d "{\"id\":\"$Bc\",\"name\":\"Gate B\",\"vendor\":\"hikvision\"}"

log "## topology link A->B (bidirectional, transit 60s)"
curl -s -o /dev/null -X POST "$API/movement/links" -H 'content-type: application/json' -d "{\"from_camera\":\"$A\",\"to_camera\":\"$Bc\",\"transit_seconds\":60,\"bidirectional\":true}"
log "links: $(curl -s "$API/movement/links" | jqget 'len(d)')"

log "## vehicle TRAIL123: white car at A, then at B (~within transit)"
commit "$A" vA TRAIL123 white car
sleep 3
commit "$Bc" vB TRAIL123 white car
sleep 1

log "## trigger movement engines -> $(curl -s -o /dev/null -w '%{http_code}' -X POST "$API/movement/run")"
log "## ReID candidates (vehicle, plate-anchored):"
curl -s "$API/movement/candidates" | python3 -c '
import sys,json
for c in json.load(sys.stdin):
    print("  %s anchor=%s %s->%s score=%.2f transit=%.0fs status=%s" % (c["subject_type"],c.get("anchor"),c.get("from_camera"),c.get("to_camera"),c["score"],c.get("transit_seconds") or 0,c["status"]))'

CID=$(curl -s "$API/movement/candidates?status=pending&limit=1" | jqget '(d[0]["id"] if d else "")')
if [ -n "$CID" ]; then
  log "## confirm candidate $CID -> $(curl -s -o /dev/null -w '%{http_code}' -X POST "$API/movement/candidates/$CID/confirm")"
fi

log ""
log "## red-zone breach: restricted zone on A, drive a track in"
ZID=$(curl -s -X POST "$API/cameras/$A/zones" -H 'content-type: application/json' -d '{"name":"Restricted-Dock","kind":"restricted","severity":"critical","labels":["person"],"config":{"confirm_frames":1},"polygon":[[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]]}' | jqget 'd["id"]')
curl -s -o /dev/null -X POST "$API/ai/events" -H 'content-type: application/json' -d "{\"camera_id\":\"$A\",\"task_type\":\"detection\",\"detections\":[{\"label\":\"person\",\"confidence\":0.9,\"track_id\":\"intruder1\",\"bbox\":[0.4,0.6,0.1,0.2]}]}"
sleep 1
log "## trigger -> $(curl -s -o /dev/null -w '%{http_code}' -X POST "$API/movement/run")"
log "## breach alerts:"
curl -s "$API/movement/breaches" | python3 -c '
import sys,json
for b in json.load(sys.stdin):
    print("  %s zone=%s subject=%s/%s severity=%s status=%s" % (b["rule"],b.get("zone_name"),b.get("subject_type"),b.get("subject"),b["severity"],b["status"]))'
BID=$(curl -s "$API/movement/breaches?status=open&limit=1" | jqget '(d[0]["id"] if d else "")')
if [ -n "$BID" ]; then
  log "## ack $BID -> $(curl -s -o /dev/null -w '%{http_code}' -X POST "$API/movement/breaches/$BID/ack")"
  log "## resolve $BID -> $(curl -s -o /dev/null -w '%{http_code}' -X POST "$API/movement/breaches/$BID/resolve")"
fi

log ""
log "## audited plate search TRAIL123:"
curl -s "$API/movement/search/plate/TRAIL123" | python3 -c '
import sys,json;d=json.load(sys.stdin)
print("  appearances:",len(d["appearances"]),"candidates:",len(d["candidates"]))
for a in d["appearances"]: print("   -",a["camera_id"],a["timestamp"],a["event_type"])'
log "## audit log shows the search? -> $(curl -s "$API/audit?action=movement_search_plate&limit=3" | jqget 'len(d)') entries"
log "DONE"
