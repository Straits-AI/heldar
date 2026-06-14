#!/usr/bin/env bash
# Stage 4 Campus Entry validation: registry → simulated ANPR reads → temporal voting → authorization
# resolution (matched / exception / blocked / unmatched / pass) → guard workflow → reports → audit.
# Runs against the stack with AUTH disabled (default); see validate_rbac.sh for the RBAC path.
set -u
API=http://127.0.0.1:8000/api/v1
CAM=cam_192_168_0_2
REPORT=/home/soh/cctv/data/validate_entry.txt
: > "$REPORT"
log(){ echo "$@" | tee -a "$REPORT"; }
jqget(){ python3 -c "import sys,json;d=json.load(sys.stdin);print($1)"; }

# Post one ANPR read for a track (plate/color/type via attributes). $1=track $2=plate $3=color $4=vtype
read_anpr(){
  curl -s -o /dev/null -w '%{http_code} ' -X POST "$API/ai/events" -H 'content-type: application/json' -d "{
    \"camera_id\":\"$CAM\",\"task_type\":\"anpr\",
    \"detections\":[{\"label\":\"$4\",\"confidence\":0.9,\"track_id\":\"$1\",\"bbox\":[0.4,0.4,0.2,0.3],
      \"attributes\":{\"plate\":\"$2\",\"plate_confidence\":0.93,\"color\":\"$3\",\"vehicle_type\":\"$4\",\"direction\":\"inbound\"}}]}"
}
# Drive a track to commit (min_votes default 3 → send 4 reads).
commit_track(){ for i in 1 2 3 4; do read_anpr "$@"; done; echo; }

curl -s --retry 30 --retry-delay 1 --retry-connrefused -o /dev/null "$API/../healthz" || { log "core down"; exit 1; }
log "== core up =="

log "## registry setup"
VID=$(curl -s -X POST "$API/vehicles" -H 'content-type: application/json' \
  -d '{"plate":"ABC 1234","owner_name":"Tan Ali","owner_type":"staff","vehicle_type":"car","color":"white","make":"Perodua","model":"Myvi"}' | jqget 'd["id"]')
log "registered vehicle ABC1234 (white car): $VID"
curl -s -X POST "$API/watchlist" -H 'content-type: application/json' -d '{"plate":"BAD9999","kind":"block","reason":"stolen","severity":"critical"}' >/dev/null
log "watchlist BLOCK BAD9999 added"
PID=$(curl -s -X POST "$API/passes" -H 'content-type: application/json' \
  -d '{"visitor_name":"Siti Visitor","phone":"0123","host":"Admin Office","purpose":"meeting","plate":"VIS1234"}' | jqget 'd["id"]')
log "visitor pass for VIS1234: $PID"

log "## ANPR scenarios (4 reads each → temporal voting commit)"
log "matched (registered, attrs agree):    $(commit_track t_match  ABC1234 white car)"
log "exception (registered, color/type mismatch): $(commit_track t_exc BLKABC1234X black truck)"  # placeholder, fixed below
log "blocked (watchlist):                  $(commit_track t_block  BAD9999 silver car)"
log "unmatched (unknown plate):            $(commit_track t_unk    ZZZ0000 blue car)"
log "pass match (visitor pass):            $(commit_track t_pass   VIS1234 gray car)"

# Exception scenario done properly: same registered plate ABC1234 but detected black truck.
log "exception (ABC1234 seen as black truck): $(commit_track t_exc2 ABC1234 black truck)"

sleep 1
# Display helper (avoids fragile inline f-strings under bash quoting).
cat > /tmp/_ve_show.py <<'PYEOF'
import sys, json
mode = sys.argv[1]
d = json.load(sys.stdin)
if mode == "events":
    for e in d:
        a = e["authorization"]
        extra = a.get("mismatches") or a.get("reason") or a.get("note") or ""
        ok = "OK" if e["auth_status"] == a.get("status") else "MISMATCH"
        print("  %-14s plate=%-11s auth=%-10s wf=%-8s src=%-18s [%s] %s" % (
            e["event_type"], str(e.get("plate")), e["auth_status"],
            e["workflow_status"], str(a.get("source")), ok, extra))
elif mode == "audit":
    for a in d:
        print("  %-18s %s/%s by %s(%s)" % (
            a["action"], a.get("target_type"), a.get("target_id"),
            a.get("actor_name"), a.get("role")))
PYEOF

log ""
log "## entry events (newest first; [OK] = denormalized auth_status matches authorization JSON):"
curl -s "$API/entry-events?limit=20" | python3 /tmp/_ve_show.py events | tee -a "$REPORT"

log ""
log "## auth_status tally (today entry-log report):"
curl -s "$API/reports/entry-log" | jqget 'd["by_auth_status"]'

log "## exceptions report count:"
curl -s "$API/reports/exceptions" | jqget 'd["total"]'

log "## pass auto-checked-in by ANPR match? -> $(curl -s "$API/passes/$PID" | jqget 'd["status"]')"

log ""
log "## guard workflow: confirm the first pending event"
EVID=$(curl -s "$API/entry-events?workflow_status=pending&limit=1" | jqget '(d[0]["id"] if d else "")')
if [ -n "$EVID" ]; then
  log "confirm $EVID -> $(curl -s -o /dev/null -w '%{http_code}' -X POST "$API/entry-events/$EVID/confirm" -H 'content-type: application/json' -d '{"note":"verified at booth"}')"
  log "workflow_status now: $(curl -s "$API/entry-events/$EVID" | jqget 'd["workflow_status"]')"
fi

log ""
log "## audit log (recent actions):"
curl -s "$API/audit?limit=8" | python3 /tmp/_ve_show.py audit | tee -a "$REPORT"
log "DONE"
