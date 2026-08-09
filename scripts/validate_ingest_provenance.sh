#!/usr/bin/env bash
# Stage B validation: task leases, server-issued frame tickets, and unforgeable ingest provenance.
#
# Self-contained in the same style as validate_rbac.sh: it boots its OWN auth-enabled cores on
# throwaway ports with temp databases (no MediaMTX, no camera, recorder + AI sampler off) and prints
# PASS/FAIL lines. It never touches the main stack or its database.
#
# Two cores are started, because the staging design makes the two tiers behave DIFFERENTLY and both
# behaviours are contracts:
#   :8011  HELDAR_INGEST_PROVENANCE=enforce  — the hardened posture
#   :8012  HELDAR_INGEST_PROVENANCE=warn     — today's behaviour, the default, must still work
#
# There is no real camera, so the sampled frame a ticket is minted over is written directly into
# HELDAR_FRAMES_DIR — exactly what the sampler would have written. Nothing else is faked.
#
# Style note: every assertion captures the status/value into a variable on its own line before
# comparing. Inlining `$(...)` into the assertion call is what this script did first, and a body that
# failed to parse silently reported the wrong thing — the two-step form makes the observed value
# visible and un-shiftable.
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DATA="${HELDAR_DATA_DIR:-$ROOT/data}"
mkdir -p "$DATA"
REPORT="$DATA/validate_ingest_provenance.txt"
: > "$REPORT"

log(){ echo "$@" | tee -a "$REPORT"; }
jqget(){ python3 -c "import sys,json;d=json.load(sys.stdin);print($1)" 2>/dev/null; }
code(){ curl -s -o /dev/null -w '%{http_code}' "$@"; }
# $1 observed, $2 expected, $3 description.
want(){ if [ "$1" = "$2" ]; then log "  PASS $3 [$1]"; else log "  FAIL $3 (got '$1', want '$2')"; fi; }

TMP=$(mktemp -d)
PIDS=()
cleanup(){ for p in "${PIDS[@]:-}"; do kill "$p" 2>/dev/null; done; rm -rf "$TMP"; }
trap cleanup EXIT

cd "$ROOT"

# A 1x1 JPEG. Content is irrelevant — nothing decodes it, and the ticket is minted over the file's
# mtime. Re-running it bumps mtime, which is how a DISTINCT frame (and so a distinct server-derived
# frame_id) is simulated without a sampler.
write_frame(){ # $1 = frames dir, $2 = camera id
  mkdir -p "$1/$2"
  python3 -c "
import base64,sys
open(sys.argv[1],'wb').write(base64.b64decode(
 '/9j/4AAQSkZJRgABAQEAYABgAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0a'
 'HBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAA'
 'AAAAAAAAAAAACf/EABQQAQAAAAAAAAAAAAAAAAAAAAD/2gAIAQEAAD8AKp//2Q=='))
" "$1/$2/latest_sub.jpg"
}

# Boot a throwaway auth-enabled core. $1 = port, $2 = ingest-provenance tier.
# NOTE: each `local` gets its own line — bash 3.2 (macOS) expands every RHS on a `local` line before
# performing any assignment, so `dir="$TMP/$port"` on a shared line would see an unset `port`.
start_core(){
  local port="$1"
  local tier="$2"
  local dir="$TMP/$port"
  mkdir -p "$dir/frames"
  HELDAR_AUTH_ENABLED=true \
  HELDAR_BOOTSTRAP_ADMIN_USER=admin \
  HELDAR_BOOTSTRAP_ADMIN_PASSWORD=supersecret123 \
  HELDAR_DATA_DIR="$dir" \
  HELDAR_DATABASE_URL="sqlite://$dir/prov.db" \
  HELDAR_FRAMES_DIR="$dir/frames" \
  HELDAR_API_PORT="$port" \
  HELDAR_RECORDER_ENABLED=false \
  HELDAR_AI_ENABLED=false \
  HELDAR_INGEST_PROVENANCE="$tier" \
  HELDAR_MACHINE_AUTH=enforce \
  HELDAR_MEDIAMTX_API_URL=http://127.0.0.1:65599 \
  ./target/debug/heldar-core >"$dir/core.log" 2>&1 &
  PIDS+=($!)
  local i
  for i in $(seq 1 40); do
    [ "$(code "http://127.0.0.1:$port/healthz")" = "200" ] && { log "core($tier) up on :$port after ${i}s"; return 0; }
    sleep 1
  done
  log "  FAIL core($tier) on :$port never became healthy"
  return 1
}

# Provision camera + AI task + a least-privilege AI key. Echoes "TOKEN KEY TASKID".
provision(){
  local port="$1"
  local api="http://127.0.0.1:$port/api/v1"
  local tok key task
  tok=$(curl -s -X POST "$api/auth/login" -H 'content-type: application/json' \
        -d '{"username":"admin","password":"supersecret123"}' | jqget 'd["token"]')
  curl -s -X POST "$api/cameras" -H "Authorization: Bearer $tok" -H 'content-type: application/json' \
    -d '{"id":"cam1","name":"Lane 1","address":"127.0.0.1"}' >/dev/null
  curl -s -X POST "$api/cameras" -H "Authorization: Bearer $tok" -H 'content-type: application/json' \
    -d '{"id":"cam2","name":"Lane 2","address":"127.0.0.2"}' >/dev/null
  task=$(curl -s -X POST "$api/cameras/cam1/ai-tasks" -H "Authorization: Bearer $tok" \
         -H 'content-type: application/json' -d '{"task_type":"anpr","stream_profile":"sub"}' | jqget 'd["id"]')
  key=$(curl -s -X POST "$api/api-keys" -H "Authorization: Bearer $tok" -H 'content-type: application/json' \
        -d '{"name":"aiworker","capabilities":["ai:tasks","ai:frames","ai:ingest","camera:read","events:read"]}' \
        | jqget 'd["key"]')
  echo "$tok $key $task"
}

# Acquire/renew the lease, then pull the frame with ?task= and echo the x-frame-ticket.
get_ticket(){ # $1 port, $2 key, $3 task id, $4 worker id, $5 camera
  local api="http://127.0.0.1:$1/api/v1"
  curl -s -o /dev/null -X POST "$api/ai/leases" -H "X-API-Key: $2" \
    -H 'content-type: application/json' -d "{\"worker_id\":\"$4\"}"
  curl -s -D - -o /dev/null "$api/cameras/$5/frame?profile=sub&task=$3" -H "X-API-Key: $2" \
    | tr -d '\r' | awk 'tolower($1)=="x-frame-ticket:"{print $2}'
}

# ===================================================================================================
log "== ENFORCE tier (:8011) =="
start_core 8011 enforce || { log "DONE"; exit 0; }
API=http://127.0.0.1:8011/api/v1
read -r ADMIN_TOK KEY TASK <<<"$(provision 8011)"
write_frame "$TMP/8011/frames" cam1
log "  camera cam1, ai task $TASK, key ${KEY:0:12}..."

log ""
log "## a ticketless batch is refused"
ST=$(code -X POST "$API/ai/events" -H "X-API-Key: $KEY" -H 'content-type: application/json' \
     -d '{"camera_id":"cam1","task_type":"anpr","detections":[]}')
want "$ST" 401 "ticketless ingest under enforce -> 401 frame_ticket_required"

log ""
log "## a lease yields a frame ticket"
TICKET=$(get_ticket 8011 "$KEY" "$TASK" w1 cam1)
[ -n "$TICKET" ] && want present present "frame pull with ?task= emits x-frame-ticket" \
                 || want absent present "frame pull with ?task= emits x-frame-ticket"
NOTICKET=$(curl -s -D - -o /dev/null "$API/cameras/cam1/frame?profile=sub" -H "X-API-Key: $KEY" \
           | tr -d '\r' | awk 'tolower($1)=="x-frame-ticket:"{print $2}')
want "${NOTICKET:-<none>}" "<none>" "frame pull WITHOUT ?task= emits no ticket (dashboard untouched)"

log ""
log "## THE NEGATIVE CONTROL: camera_native cannot be asserted over the API"
BODY="{\"frame_ticket\":\"$TICKET\",\"camera_id\":\"cam1\",\"task_type\":\"anpr\",\"detections\":[{\"label\":\"vehicle\",\"confidence\":0.9,\"attributes\":{\"plate\":\"ABC1234\",\"source\":\"camera_native\",\"_prov\":{\"producer\":\"native_anpr\"}}}]}"
ST=$(code -X POST "$API/ai/events" -H "X-API-Key: $KEY" -H 'content-type: application/json' -d "$BODY")
want "$ST" 200 "a ticketed batch is accepted -> 200"
DET=$(curl -s "$API/cameras/cam1/detections?limit=1" -H "Authorization: Bearer $ADMIN_TOK")
SRC=$(echo "$DET" | jqget 'd[0]["attributes"]["source"]')
want "$SRC" worker "forged attributes.source=camera_native is persisted as 'worker'"
PRODUCER=$(echo "$DET" | jqget 'd[0]["attributes"]["_prov"].get("producer","<none>")')
want "$PRODUCER" "<none>" "forged _prov.producer is stripped, not merged"

log ""
log "## server-derived frame_id (a client cannot name the dedup key)"
FID=$(echo "$DET" | jqget 'd[0]["frame_id"]')
CAPMS=$(echo "$TICKET" | cut -d. -f3)
want "$FID" "$TASK:$CAPMS" "persisted frame_id is server-derived as {task_id}:{captured_ms}"

log ""
log "## ticket binding"
KEY_B=$(curl -s -X POST "$API/api-keys" -H "Authorization: Bearer $ADMIN_TOK" -H 'content-type: application/json' \
        -d '{"name":"other","capabilities":["ai:tasks","ai:frames","ai:ingest","camera:read"]}' | jqget 'd["key"]')
ST=$(code -X POST "$API/ai/events" -H "X-API-Key: $KEY_B" -H 'content-type: application/json' \
     -d "{\"frame_ticket\":\"$TICKET\",\"camera_id\":\"cam1\",\"task_type\":\"anpr\",\"detections\":[]}")
want "$ST" 401 "a ticket issued to key A is inert for key B -> 401"
ST=$(code -X POST "$API/ai/events" -H "X-API-Key: $KEY" -H 'content-type: application/json' \
     -d "{\"frame_ticket\":\"$TICKET\",\"camera_id\":\"cam2\",\"task_type\":\"anpr\",\"detections\":[]}")
want "$ST" 409 "body camera_id contradicting the ticket -> 409"
ST=$(code -X POST "$API/ai/events" -H "X-API-Key: $KEY" -H 'content-type: application/json' \
     -d "{\"frame_ticket\":\"$TICKET\",\"camera_id\":\"cam1\",\"task_type\":\"face\",\"detections\":[]}")
want "$ST" 409 "body task_type contradicting the ticket -> 409"
ST=$(code -X POST "$API/ai/events" -H "X-API-Key: $KEY" -H 'content-type: application/json' \
     -d '{"frame_ticket":"f1.nope.1.9999999999.AAAA","camera_id":"cam1","task_type":"anpr","detections":[]}')
want "$ST" 401 "a ticket for a task we hold no lease on -> 401"
ST=$(code -X POST "$API/ai/events" -H "X-API-Key: $KEY" -H 'content-type: application/json' \
     -d '{"frame_ticket":"garbage","camera_id":"cam1","task_type":"anpr","detections":[]}')
want "$ST" 401 "a malformed ticket -> 401"

log ""
log "## THE LEASE NEGATIVE CONTROL: ingest for a task/camera the lease does not cover"
# cam2 is covered by no lease and has no AI task, so no ticket for it can exist and a ticketed batch
# cannot be re-pointed at it (the 409 above). Ticketless is refused outright.
ST=$(code -X POST "$API/ai/events" -H "X-API-Key: $KEY" -H 'content-type: application/json' \
     -d '{"camera_id":"cam2","task_type":"anpr","detections":[]}')
want "$ST" 401 "ingest for an unleased camera -> 401 (no lease, no ticket, no ingest)"
# Disabling the task revokes its outstanding tickets AT ONCE, not at lease expiry.
curl -s -o /dev/null -X PATCH "$API/ai-tasks/$TASK" -H "Authorization: Bearer $ADMIN_TOK" \
  -H 'content-type: application/json' -d '{"enabled":false}'
ST=$(code -X POST "$API/ai/events" -H "X-API-Key: $KEY" -H 'content-type: application/json' \
     -d "{\"frame_ticket\":\"$TICKET\",\"camera_id\":\"cam1\",\"task_type\":\"anpr\",\"detections\":[]}")
want "$ST" 403 "a ticket for a task disabled since issue -> 403"
curl -s -o /dev/null -X PATCH "$API/ai-tasks/$TASK" -H "Authorization: Bearer $ADMIN_TOK" \
  -H 'content-type: application/json' -d '{"enabled":true}'

log ""
log "## reserved kernel-domain event types cannot be forged"
write_frame "$TMP/8011/frames" cam1     # fresh mtime -> a distinct frame, distinct frame_id
TICKET2=$(get_ticket 8011 "$KEY" "$TASK" w1 cam1)
ST=$(code -X POST "$API/ai/events" -H "X-API-Key: $KEY" -H 'content-type: application/json' \
     -d "{\"frame_ticket\":\"$TICKET2\",\"camera_id\":\"cam1\",\"task_type\":\"anpr\",\"detections\":[],\"event\":{\"event_type\":\"gate_opened\",\"severity\":\"critical\"}}")
want "$ST" 400 "worker raising gate_opened -> 400"
ST=$(code -X POST "$API/ai/events" -H "X-API-Key: $KEY" -H 'content-type: application/json' \
     -d "{\"frame_ticket\":\"$TICKET2\",\"camera_id\":\"cam1\",\"task_type\":\"anpr\",\"detections\":[],\"event\":{\"event_type\":\"zone_foo\"}}")
want "$ST" 400 "worker raising zone_foo -> 400"
ST=$(code -X POST "$API/ai/events" -H "X-API-Key: $KEY" -H 'content-type: application/json' \
     -d "{\"frame_ticket\":\"$TICKET2\",\"camera_id\":\"cam1\",\"task_type\":\"anpr\",\"detections\":[],\"event\":{\"event_type\":\"my_custom_thing\",\"severity\":\"critical\"}}")
want "$ST" 200 "worker raising its own event type -> 200"
SEV=$(curl -s "$API/events?limit=100" -H "Authorization: Bearer $ADMIN_TOK" | python3 -c "
import sys,json
rows=json.load(sys.stdin)
rows=rows if isinstance(rows,list) else rows.get('items',[])
print(next((r['severity'] for r in rows if r.get('event_type')=='my_custom_thing'),'<none>'))" 2>/dev/null)
want "$SEV" warning "a worker cannot self-escalate severity to critical (clamped)"

log ""
log "## SUPPRESSION CONTROL (enforce): an attacker cannot pre-claim the worker's frame_id"
write_frame "$TMP/8011/frames" cam1
TICKET3=$(get_ticket 8011 "$KEY" "$TASK" w1 cam1)
CAPMS3=$(echo "$TICKET3" | cut -d. -f3)
ST=$(code -X POST "$API/ai/events" -H "X-API-Key: $KEY_B" -H 'content-type: application/json' \
     -d "{\"camera_id\":\"cam1\",\"task_type\":\"anpr\",\"frame_id\":\"$TASK:$CAPMS3\",\"detections\":[]}")
want "$ST" 401 "attacker pre-claiming the frame_id ticketlessly -> 401"
DUP=$(curl -s -X POST "$API/ai/events" -H "X-API-Key: $KEY" -H 'content-type: application/json' \
      -d "{\"frame_ticket\":\"$TICKET3\",\"camera_id\":\"cam1\",\"task_type\":\"anpr\",\"detections\":[{\"label\":\"vehicle\",\"confidence\":0.8}]}" \
      | jqget 'd.get("duplicate",False)')
want "$DUP" False "the real worker's ticketed post is NOT suppressed (duplicate=false)"

log ""
log "## lease exclusivity"
N_A=$(curl -s -X POST "$API/ai/leases" -H "X-API-Key: $KEY" -H 'content-type: application/json' \
      -d '{"worker_id":"w1"}' | jqget 'len(d["tasks"])')
want "$N_A" 1 "the holder renews and keeps its task"
N_B=$(curl -s -X POST "$API/ai/leases" -H "X-API-Key: $KEY_B" -H 'content-type: application/json' \
      -d '{"worker_id":"w2"}' | jqget 'len(d["tasks"])')
want "$N_B" 0 "a second credential gets nothing while the lease is live"
ST=$(code "$API/ai/tasks" -H "X-API-Key: $KEY_B")
want "$ST" 200 "GET /ai/tasks is unchanged for a worker that never leases (old-worker compat)"
LEASE_ID=$(curl -s -X POST "$API/ai/leases" -H "X-API-Key: $KEY" -H 'content-type: application/json' \
           -d '{"worker_id":"w1"}' | jqget 'd["lease_id"]')
curl -s -o /dev/null -X DELETE "$API/ai/leases/$LEASE_ID" -H "X-API-Key: $KEY_B"
N_B2=$(curl -s -X POST "$API/ai/leases" -H "X-API-Key: $KEY_B" -H 'content-type: application/json' \
       -d '{"worker_id":"w2"}' | jqget 'len(d["tasks"])')
want "$N_B2" 0 "another credential cannot release a lease it does not hold"
curl -s -o /dev/null -X DELETE "$API/ai/leases/$LEASE_ID" -H "X-API-Key: $KEY"
N_B3=$(curl -s -X POST "$API/ai/leases" -H "X-API-Key: $KEY_B" -H 'content-type: application/json' \
       -d '{"worker_id":"w2"}' | jqget 'len(d["tasks"])')
want "$N_B3" 1 "the holder's own release frees the task immediately"

log ""
log "## least-privilege AI credential (hole (a)): the worker key reaches nothing else"
ST=$(code "$API/vehicles" -H "X-API-Key: $KEY");   want "$ST" 403 "AI key -> GET /vehicles"
ST=$(code "$API/watchlist" -H "X-API-Key: $KEY");  want "$ST" 403 "AI key -> GET /watchlist"
ST=$(code -X POST "$API/discover" -H "X-API-Key: $KEY" -H 'content-type: application/json' \
     -d '{"targets":"127.0.0.1"}'); want "$ST" 403 "AI key -> POST /discover"
ST=$(code "$API/system" -H "X-API-Key: $KEY");     want "$ST" 403 "AI key -> GET /system"
ST=$(code "$API/cameras/cam1/liveview" -H "X-API-Key: $KEY"); want "$ST" 403 "AI key -> GET /cameras/{id}/liveview"

# ===================================================================================================
log ""
log "== WARN tier (:8012) — the DEFAULT, must behave exactly as today =="
start_core 8012 warn || { log "DONE"; exit 0; }
API2=http://127.0.0.1:8012/api/v1
read -r ADMIN2 KEY2 TASK2 <<<"$(provision 8012)"
write_frame "$TMP/8012/frames" cam1

ST=$(code -X POST "$API2/ai/events" -H "X-API-Key: $KEY2" -H 'content-type: application/json' \
     -d '{"camera_id":"cam1","task_type":"anpr","detections":[{"label":"vehicle"}]}')
want "$ST" 200 "ticketless ingest under warn -> 200 (today's behaviour, unchanged)"
# The rewrite is UNCONDITIONAL — provenance is not a tiered behaviour.
curl -s -o /dev/null -X POST "$API2/ai/events" -H "X-API-Key: $KEY2" -H 'content-type: application/json' \
  -d '{"camera_id":"cam1","task_type":"anpr","frame_id":"warn-1","detections":[{"label":"vehicle","attributes":{"source":"camera_native"}}]}'
SRC2=$(curl -s "$API2/cameras/cam1/detections?limit=1" -H "Authorization: Bearer $ADMIN2" \
       | jqget 'd[0]["attributes"]["source"]')
want "$SRC2" worker "forged camera_native is rewritten under warn TOO (the rewrite is unconditional)"

log ""
log "## SUPPRESSION CONTROL (warn): the documented residual"
# Under warn a client still names its own frame_id, so first-writer-wins remains reachable. This is a
# PASSING test of a known limitation rather than a comment; flipping to enforce is what closes it.
D1=$(curl -s -X POST "$API2/ai/events" -H "X-API-Key: $KEY2" -H 'content-type: application/json' \
     -d '{"camera_id":"cam1","task_type":"anpr","frame_id":"race-me","detections":[]}' | jqget 'd.get("duplicate",False)')
want "$D1" False "first writer of a client-named frame_id wins"
D2=$(curl -s -X POST "$API2/ai/events" -H "X-API-Key: $KEY2" -H 'content-type: application/json' \
     -d '{"camera_id":"cam1","task_type":"anpr","frame_id":"race-me","detections":[]}' | jqget 'd.get("duplicate",False)')
want "$D2" True "under warn a pre-claimed frame_id still suppresses (residual, closed by enforce)"

log ""
log "DONE"
