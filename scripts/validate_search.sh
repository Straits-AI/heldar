#!/usr/bin/env bash
# Stage 7 Semantic search validation: structured search, NL search (rule planner → plan → execute →
# proof), plan dry-run, and the identity-query audit. Assumes the stack is up. Auth off (default).
set -u
API=http://127.0.0.1:8000/api/v1
CAM=cam_192_168_0_2
REPORT=/home/soh/cctv/data/validate_search.txt
: > "$REPORT"
log(){ echo "$@" | tee -a "$REPORT"; }
jqget(){ python3 -c "import sys,json;d=json.load(sys.stdin);print($1)"; }
anpr(){ curl -s -o /dev/null -X POST "$API/ai/events" -H 'content-type: application/json' -d "{\"camera_id\":\"$CAM\",\"task_type\":\"anpr\",\"detections\":[{\"label\":\"car\",\"confidence\":0.9,\"track_id\":\"$1\",\"bbox\":[0.4,0.4,0.2,0.3],\"attributes\":{\"plate\":\"$2\",\"plate_confidence\":0.95,\"color\":\"white\",\"vehicle_type\":\"car\"}}]}"; }

curl -s --retry 30 --retry-delay 1 --retry-connrefused -o /dev/null "$API/../healthz" || { log down; exit 1; }
log "== core up =="
curl -s -o /dev/null -X POST "$API/cameras" -H 'content-type: application/json' -d "{\"id\":\"$CAM\",\"name\":\"Gate A\",\"vendor\":\"hikvision\"}"
log "## seed: white car SEEK999 (4 ANPR reads -> entry_event)"
for i in 1 2 3 4; do anpr seekv SEEK999; done
sleep 1

log ""
log "## structured search: plate=SEEK999"
curl -s -X POST "$API/search/events" -H 'content-type: application/json' -d '{"plate":"SEEK999"}' | jqget '"hits="+str(d["count"])'
log "## structured search: color=white, sources=[entry]"
curl -s -X POST "$API/search/events" -H 'content-type: application/json' -d '{"color":"white","sources":["entry"]}' | jqget '"white-entry hits="+str(d["count"])'

log ""
log "## NL search: 'unauthorized vehicles last week'"
curl -s -X POST "$API/search/nl" -H 'content-type: application/json' -d '{"query":"unauthorized vehicles last week"}' | python3 -c '
import sys,json;d=json.load(sys.stdin)
print("  planner=%s count=%d plan.auth=%s plan.subject=%s" % (d["planner"],d["count"],d["plan"].get("auth_status"),d["plan"].get("subject_type")))
print("  proof claim levels:",[c["level"] for c in d["proof"]["claim_levels"]])'

log "## NL search: 'white cars entering after 6pm'"
curl -s -X POST "$API/search/nl" -H 'content-type: application/json' -d '{"query":"white cars entering after 6pm"}' | python3 -c '
import sys,json;d=json.load(sys.stdin);p=d["plan"]
print("  count=%d color=%s event_type=%s hour_min=%s" % (d["count"],p.get("color"),p.get("event_type"),p.get("hour_min")))'

log "## NL search: 'red zone breaches yesterday'"
curl -s -X POST "$API/search/nl" -H 'content-type: application/json' -d '{"query":"red zone breaches yesterday"}' | python3 -c '
import sys,json;d=json.load(sys.stdin);p=d["plan"]
print("  count=%d sources=%s from=%s" % (d["count"],p.get("sources"),(p.get("from") or "")[:10]))'

log ""
log "## plan dry-run: 'unknown white truck entering Gate A after 6pm yesterday'"
curl -s -X POST "$API/search/plan" -H 'content-type: application/json' -d '{"query":"unknown white truck entering Gate A after 6pm yesterday"}' | jqget 'json.dumps(d["plan"])'

log ""
log "## identity-query audit (plate search writes audit_log):"
curl -s -X POST "$API/search/nl" -H 'content-type: application/json' -d '{"query":"vehicle SEEK999"}' >/dev/null
log "  search_identity_query audit entries: $(curl -s "$API/audit?action=search_identity_query&limit=5" | jqget 'len(d)')"
log "  search_log rows: $(sqlite3 /home/soh/cctv/data/heldar.db 'SELECT count(*) FROM search_log;' 2>/dev/null)"
log "DONE"
