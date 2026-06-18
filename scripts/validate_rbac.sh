#!/usr/bin/env bash
# Stage 4 RBAC validation: starts a throwaway AUTH-ENABLED core on :8001 with a temp DB, exercises
# login, role enforcement (401/403/200), and API-key ingest, then tears it down. Does not touch the
# main stack or its database.
set -u
PORT=8001
API=http://127.0.0.1:$PORT/api/v1
TMP=$(mktemp -d)
REPORT=/home/soh/cctv/data/validate_rbac.txt
: > "$REPORT"
log(){ echo "$@" | tee -a "$REPORT"; }
jqget(){ python3 -c "import sys,json;d=json.load(sys.stdin);print($1)"; }
code(){ curl -s -o /dev/null -w '%{http_code}' "$@"; }

cd /home/soh/cctv
log "== starting auth-enabled core on :$PORT (temp db $TMP) =="
HELDAR_AUTH_ENABLED=true \
HELDAR_BOOTSTRAP_ADMIN_USER=admin \
HELDAR_BOOTSTRAP_ADMIN_PASSWORD=supersecret123 \
HELDAR_DATA_DIR="$TMP" \
HELDAR_DATABASE_URL="sqlite://$TMP/rbac.db" \
HELDAR_API_PORT=$PORT \
HELDAR_RECORDER_ENABLED=false \
HELDAR_AI_ENABLED=false \
HELDAR_MEDIAMTX_API_URL=http://127.0.0.1:65599 \
./target/debug/heldar-core >"$TMP/core.log" 2>&1 &
CORE_PID=$!
trap 'kill $CORE_PID 2>/dev/null; rm -rf "$TMP"' EXIT

for i in $(seq 1 40); do
  [ "$(code http://127.0.0.1:$PORT/healthz)" = "200" ] && { log "core up after ${i}s"; break; }
  sleep 1
done

pass(){ if [ "$1" = "$2" ]; then log "  PASS $3 ($1)"; else log "  FAIL $3 (got $1, want $2)"; fi; }

log ""
log "## unauthenticated access is rejected"
pass "$(code $API/vehicles)" 401 "GET /vehicles without token -> 401"
pass "$(code -X POST $API/ai/events -H 'content-type: application/json' -d '{}')" 401 "POST /ai/events without token -> 401"

log ""
log "## admin login + use"
ADMIN_TOK=$(curl -s -X POST $API/auth/login -H 'content-type: application/json' -d '{"username":"admin","password":"supersecret123"}' | jqget 'd["token"]')
log "  admin token: ${ADMIN_TOK:0:16}..."
pass "$(code $API/vehicles -H "Authorization: Bearer $ADMIN_TOK")" 200 "GET /vehicles as admin -> 200"
pass "$(curl -s $API/auth/me -H "Authorization: Bearer $ADMIN_TOK" | jqget 'd["role"]')" admin "GET /auth/me role -> admin"
pass "$(code -X POST $API/auth/login -H 'content-type: application/json' -d '{"username":"admin","password":"wrong"}')" 401 "login wrong password -> 401"
pass "$(code -X POST $API/auth/login -H 'content-type: application/json' -d '{"username":"ghost","password":"whatever"}')" 401 "login unknown user -> 401"

log ""
log "## admin creates a guard user; guard role is enforced"
curl -s -X POST $API/users -H "Authorization: Bearer $ADMIN_TOK" -H 'content-type: application/json' \
  -d '{"username":"booth1","password":"guardpass123","role":"guard","display_name":"Booth 1"}' >/dev/null
GUARD_TOK=$(curl -s -X POST $API/auth/login -H 'content-type: application/json' -d '{"username":"booth1","password":"guardpass123"}' | jqget 'd["token"]')
log "  guard token: ${GUARD_TOK:0:16}..."
pass "$(code -X POST $API/passes -H "Authorization: Bearer $GUARD_TOK" -H 'content-type: application/json' -d '{"visitor_name":"Walk In"}')" 201 "guard creates visitor pass -> 201"
pass "$(code -X POST $API/vehicles -H "Authorization: Bearer $GUARD_TOK" -H 'content-type: application/json' -d '{"plate":"ABC1"}')" 403 "guard registers vehicle -> 403 (manager+)"
pass "$(code $API/users -H "Authorization: Bearer $GUARD_TOK")" 403 "guard lists users -> 403 (admin)"
pass "$(code $API/audit -H "Authorization: Bearer $GUARD_TOK")" 403 "guard reads audit -> 403 (manager+)"
pass "$(code -X POST $API/ai/events -H "Authorization: Bearer $GUARD_TOK" -H 'content-type: application/json' -d '{"camera_id":"x","task_type":"anpr"}')" 403 "guard ingests events -> 403 (integration/admin)"

log ""
log "## API key (integration role) can ingest"
KEY=$(curl -s -X POST $API/api-keys -H "Authorization: Bearer $ADMIN_TOK" -H 'content-type: application/json' -d '{"name":"worker","role":"integration"}' | jqget 'd["key"]')
log "  api key: ${KEY:0:12}..."
# camera does not exist -> ingest auth passes (not 401/403) but load_camera 404s; that proves the key authenticated.
IC=$(code -X POST $API/ai/events -H "X-API-Key: $KEY" -H 'content-type: application/json' -d '{"camera_id":"nope","task_type":"anpr"}')
pass "$IC" 404 "integration key ingest (unknown cam) -> 404 not 401/403 (authenticated)"
pass "$(code $API/users -H "X-API-Key: $KEY")" 403 "integration key lists users -> 403"
pass "$(code $API/vehicles -H 'Authorization: Bearer vos_deadbeef')" 401 "bogus token -> 401"

log ""
log "## AI/worker surface authentication floor (Stage 0 security floor)"
pass "$(code $API/ai/tasks)" 401 "GET /ai/tasks without token -> 401 (was open)"
pass "$(code $API/ai/tasks -H "X-API-Key: $KEY")" 200 "GET /ai/tasks with integration key -> 200"
pass "$(code $API/cameras/anycam/frame)" 401 "GET /cameras/{id}/frame without token -> 401 (was open)"

log ""
log "## logout revokes the session"
curl -s -X POST $API/auth/logout -H "Authorization: Bearer $GUARD_TOK" >/dev/null
pass "$(code $API/passes -H "Authorization: Bearer $GUARD_TOK")" 401 "guard token after logout -> 401"
log "DONE"
