#!/usr/bin/env bash
# Boots the SECURE Heldar stack for the TLS end-to-end suite: MediaMTX + synthetic RTSP cameras + the
# core with AUTH ENABLED and same-origin media URLs, behind a real Caddy terminating HTTPS.
#
# This exists because the plain e2e stack (scripts/e2e_stack.sh) runs HTTP with auth off, which is
# exactly the configuration that cannot observe the failure this suite guards: MediaMTX serves HLS and
# WebRTC on plaintext ports, so an absolute `http://host:8888/...` URL handed to an HTTPS dashboard is
# blocked by the browser as mixed content and live view dies. Nothing in an HTTP-only suite can see
# that. Here the dashboard is genuinely served over HTTPS, so a regression fails the build.
#
# Differences from the plain stack, all deliberate — they ARE the coverage:
#   auth ENABLED + Secure cookie   (the production posture; the open API is not exercised)
#   HELDAR_MEDIA_SAME_ORIGIN=true  (media URLs must be origin-relative)
#   Caddy in front on HTTPS        (media prefixes must be routed to MediaMTX)
#
# Isolated DB/data under /tmp; foreground (Playwright's `webServer` keeps it alive); tears down every
# child on TERM/EXIT. No real cameras, no credentials.
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MTX="$ROOT/infra/mediamtx/mediamtx"
CADDY="${CADDY_BIN:-$ROOT/infra/caddy/caddy}"
CORE="$ROOT/target/debug/heldar-core"
PORT="${E2E_TLS_CORE_PORT:-8012}"      # core (cleartext, loopback) — distinct from the plain suite's 8011
TLS_PORT="${E2E_TLS_PORT:-8443}"       # Caddy HTTPS — unprivileged so CI can bind it as a normal user
API="http://127.0.0.1:$PORT"
NCAMS="${E2E_TLS_CAMS:-2}"             # live view only needs a couple; keeps the suite quick
DATA="/tmp/heldar-e2e-tls"
LOG="$DATA/logs"; mkdir -p "$LOG"
rm -f "$DATA/heldar.db"* 2>/dev/null; rm -rf "$DATA/recordings" "$DATA/frames" 2>/dev/null

# Credentials for the suite's login. Test-only, and the box is loopback-scratch — but they are read
# from env so nothing hard-codes a password that could drift into a real deployment.
ADMIN_USER="${E2E_TLS_ADMIN_USER:-e2eadmin}"
ADMIN_PASS="${E2E_TLS_ADMIN_PASSWORD:-e2e-tls-suite-pw}"

PIDS=()
cleanup() {
  for p in "${PIDS[@]:-}"; do kill "$p" 2>/dev/null; done
  pkill -f "rtsp://127.0.0.1:8554/cam_tls_" 2>/dev/null
  sleep 1
}
trap cleanup EXIT INT TERM

for bin in "$MTX" "$CADDY" "$CORE"; do
  [ -x "$bin" ] || { echo "[e2e_tls] missing $bin — run scripts/setup_mediamtx.sh, scripts/setup_caddy.sh, and cargo build --bin heldar-core" >&2; exit 1; }
done

# Free the ports + kill leftovers from a previous run whose trap never ran.
if command -v lsof >/dev/null 2>&1; then
  for p in "$PORT" "$TLS_PORT" 8554 9997; do
    lsof -ti "tcp:$p" 2>/dev/null | xargs -r kill -9 2>/dev/null || true
  done
else
  fuser -k "${PORT}/tcp" "${TLS_PORT}/tcp" 8554/tcp 9997/tcp 2>/dev/null || true
fi
pkill -9 -f "rtsp://127.0.0.1:8554/cam_tls_" 2>/dev/null || true
sleep 1

echo "[e2e_tls] MediaMTX"
# Repoint the kernel auth callback at this stack's core port; otherwise MediaMTX asks a dead port and
# denies every publish AND read with a 401 (same trap as the plain stack).
#
# Second edit: exempt the SYNTHETIC SOURCE paths (cam_tls_N) from the kernel auth callback.
#
# This models production rather than weakening the test. In a real deployment the "camera" is an
# external RTSP device: the recorder and the live publisher read it directly, and MediaMTX is not in
# that path at all. Here the synthetic camera IS a MediaMTX path, so with auth ON the kernel's own
# ffmpeg readers get denied with `DESCRIBE failed: 401` and no stream is ever produced — an artefact
# of the test topology, not a product behaviour.
#
# The security property under test is untouched: the BROWSER-facing paths the kernel publishes
# (cam_cam_tls_N) still require a kernel-minted token, so an unauthenticated read of the live stream
# is still refused.
MTX_CFG="$DATA/mediamtx.yml"
sed -e "s|http://127.0.0.1:8000/internal/mediamtx-auth|http://127.0.0.1:${PORT}/internal/mediamtx-auth|" \
    -e "s|^  - action: pprof|  - action: pprof\\
  - action: read\\
    path: ~^cam_tls_[0-9]+\$|" \
  "$ROOT/infra/mediamtx/mediamtx.yml" >"$MTX_CFG"
grep -q "cam_tls_" "$MTX_CFG" || {
  echo "[e2e_tls] failed to add the synthetic-source auth exemption to $MTX_CFG" >&2; exit 1; }
"$MTX" "$MTX_CFG" >"$LOG/mediamtx.log" 2>&1 & PIDS+=($!)
sleep 2

echo "[e2e_tls] core (auth ON, same-origin media, port $PORT)"
HELDAR_DATABASE_URL="sqlite://$DATA/heldar.db" \
HELDAR_DATA_DIR="$DATA" \
HELDAR_API_HOST=127.0.0.1 \
HELDAR_API_PORT="$PORT" \
HELDAR_WEB_DIR="$ROOT/apps/web/dist" \
HELDAR_DEFAULT_SEGMENT_SECONDS=5 \
HELDAR_INDEXER_INTERVAL_S=3 \
HELDAR_HEALTH_INTERVAL_S=5 \
HELDAR_AUTH_ENABLED=true \
HELDAR_AUTH_COOKIE_SECURE=true \
HELDAR_MEDIA_SAME_ORIGIN=true \
HELDAR_BOOTSTRAP_ADMIN_USER="$ADMIN_USER" \
HELDAR_BOOTSTRAP_ADMIN_PASSWORD="$ADMIN_PASS" \
"$CORE" >"$LOG/core.log" 2>&1 & CORE_PID=$!; PIDS+=($CORE_PID)
# HELDAR_INGEST_PROVENANCE / HELDAR_MACHINE_AUTH are deliberately LEFT AT THEIR DEFAULTS here. This
# suite's job is the HTTPS + auth live-view path, and it should exercise the posture a shipping box
# actually runs — pinning both to `enforce` meant no CI suite ran the default at all. The two tiers'
# ingest behaviour is covered explicitly, and separately, by scripts/validate_ingest_provenance.sh,
# which boots one core per tier.

for _ in $(seq 1 40); do curl -fsS "$API/healthz" >/dev/null 2>&1 && break; sleep 1; done
curl -fsS "$API/healthz" >/dev/null 2>&1 || { echo "[e2e_tls] core did not start"; tail -30 "$LOG/core.log"; exit 1; }

# Auth is ON, so seeding needs a session. Log in once and reuse the cookie.
COOKIE="$DATA/cookie.txt"
curl -fsS -c "$COOKIE" -X POST "$API/api/v1/auth/login" -H 'content-type: application/json' \
  -d "{\"username\":\"${ADMIN_USER}\",\"password\":\"${ADMIN_PASS}\"}" >/dev/null || {
  echo "[e2e_tls] admin login failed — bootstrap did not create the user"; tail -30 "$LOG/core.log"; exit 1; }

echo "[e2e_tls] $NCAMS synthetic cameras"
for i in $(seq 1 "$NCAMS"); do
  ffmpeg -nostdin -hide_banner -loglevel error -re \
    -f lavfi -i "testsrc=size=640x360:rate=10" \
    -c:v libx264 -preset ultrafast -tune zerolatency -g 20 -pix_fmt yuv420p \
    -f rtsp -rtsp_transport tcp "rtsp://127.0.0.1:8554/cam_tls_${i}" >"$LOG/cam_${i}.log" 2>&1 & PIDS+=($!)
done
sleep 3

# Same readiness contract as the plain stack: Playwright waits for the LAST camera to exist, so every
# earlier seed step must complete before it.
echo "[e2e_tls] registering $NCAMS cameras"
for i in $(seq 1 "$NCAMS"); do
  curl -fsS -b "$COOKIE" -X POST "$API/api/v1/cameras" -H 'content-type: application/json' -d "{
    \"id\":\"cam_tls_${i}\",\"name\":\"TLS Camera ${i}\",\"vendor\":\"generic\",
    \"main_stream_url\":\"rtsp://127.0.0.1:8554/cam_tls_${i}\",\"record_stream\":\"main\",
    \"segment_seconds\":5,\"retention_hours\":1
  }" >/dev/null || true
done

# ---------------------------------------------------------------------------------------------------
# ENFORCE LEG: lease -> ticketed frame -> ingest, against a core running
# HELDAR_INGEST_PROVENANCE=enforce.
#
# This stack is the only CI-gated one that runs with auth ON, so it is the only place the hardened
# posture can be exercised end to end over real HTTP with a real credential. The plain stack
# (e2e_stack.sh) runs auth OFF at the default `warn` tier and is deliberately left untouched.
#
# Split by what each assertion depends on:
#   * The REFUSAL assertions need no camera and no frame, so they are UNCONDITIONAL gates — a
#     regression that reopens ticketless ingest, or that lets `camera_native` be asserted over the
#     API, fails this suite outright.
#   * The positive lease->frame->ticket->ingest walk needs the sampler to have produced a JPEG, which
#     depends on ffmpeg/MediaMTX timing this suite does not otherwise gate. If no frame materializes
#     the walk is skipped loudly rather than turning a timing artefact into a red build; if a frame
#     DOES appear, every step of the walk is a hard gate.
# ---------------------------------------------------------------------------------------------------
echo "[e2e_tls] enforce leg: lease -> ticketed frame -> ingest"
ENF_FAIL=0
enf_pass(){ if [ "$1" = "$2" ]; then echo "[e2e_tls]   PASS $3 ($1)"; else echo "[e2e_tls]   FAIL $3 (got $1, want $2)"; ENF_FAIL=1; fi; }
enf_code(){ curl -s -o /dev/null -w '%{http_code}' "$@"; }
enf_json(){ python3 -c "import sys,json;d=json.load(sys.stdin);print($1)" 2>/dev/null; }

# A least-privilege AI credential: exactly the capabilities a real worker needs.
ENF_KEY=$(curl -fsS -b "$COOKIE" -X POST "$API/api/v1/api-keys" -H 'content-type: application/json' \
  -d '{"name":"e2e-tls-aiworker","capabilities":["ai:tasks","ai:frames","ai:ingest","camera:read","events:read"]}' \
  | enf_json 'd["key"]')
if [ -z "${ENF_KEY:-}" ]; then
  echo "[e2e_tls] FAIL could not mint a capability-scoped api key (is migration 0012 applied?)"; exit 1
fi

# UNCONDITIONAL GATE 1: under enforce there is no ingest without a server-issued frame ticket.
enf_pass "$(enf_code -X POST "$API/api/v1/ai/events" -H "X-API-Key: $ENF_KEY" \
  -H 'content-type: application/json' \
  -d '{"camera_id":"cam_tls_1","task_type":"anpr","detections":[]}')" 401 \
  "ticketless ingest under enforce -> 401"

# UNCONDITIONAL GATE 2: a frame pull WITHOUT ?task= must never emit a ticket (the dashboard's reads
# are unchanged), and a fabricated ticket must not be accepted.
enf_pass "$(enf_code -X POST "$API/api/v1/ai/events" -H "X-API-Key: $ENF_KEY" \
  -H 'content-type: application/json' \
  -d '{"frame_ticket":"f1.forged.1.9999999999.AAAA","camera_id":"cam_tls_1","task_type":"anpr","detections":[]}')" 401 \
  "a fabricated frame ticket -> 401"

# Positive walk. Needs a sampled frame, so create an AI task and wait for the sampler.
ENF_TASK=$(curl -fsS -b "$COOKIE" -X POST "$API/api/v1/cameras/cam_tls_1/ai-tasks" \
  -H 'content-type: application/json' -d '{"task_type":"anpr","stream_profile":"main","fps":2}' \
  | enf_json 'd["id"]')
ENF_TICKET=""
if [ -n "${ENF_TASK:-}" ]; then
  for _ in $(seq 1 40); do
    curl -fsS -o /dev/null "$API/api/v1/cameras/cam_tls_1/frame?profile=main" -H "X-API-Key: $ENF_KEY" 2>/dev/null && break
    sleep 1
  done
  curl -fsS -o /dev/null -X POST "$API/api/v1/ai/leases" -H "X-API-Key: $ENF_KEY" \
    -H 'content-type: application/json' -d '{"worker_id":"e2e-tls-w1"}' 2>/dev/null || true
  ENF_TICKET=$(curl -fsS -D - -o /dev/null \
    "$API/api/v1/cameras/cam_tls_1/frame?profile=main&task=$ENF_TASK" -H "X-API-Key: $ENF_KEY" 2>/dev/null \
    | tr -d '\r' | awk 'tolower($1)=="x-frame-ticket:"{print $2}')
fi

if [ -z "$ENF_TICKET" ]; then
  echo "[e2e_tls]   SKIP lease->frame->ingest walk (no sampled frame yet; the refusal gates above still ran)"
else
  enf_pass "$(enf_code -X POST "$API/api/v1/ai/events" -H "X-API-Key: $ENF_KEY" \
    -H 'content-type: application/json' \
    -d "{\"frame_ticket\":\"$ENF_TICKET\",\"camera_id\":\"cam_tls_1\",\"task_type\":\"anpr\",
         \"detections\":[{\"label\":\"vehicle\",\"confidence\":0.9,
           \"attributes\":{\"plate\":\"TLS0001\",\"source\":\"camera_native\"}}]}")" 200 \
    "ticketed ingest under enforce -> 200"
  # THE NEGATIVE CONTROL, over the wire: the batch above ASKED for camera_native.
  ENF_SRC=$(curl -fsS -b "$COOKIE" "$API/api/v1/cameras/cam_tls_1/detections?limit=1" \
            | enf_json 'd[0]["attributes"]["source"]')
  enf_pass "${ENF_SRC:-<none>}" worker "forged attributes.source=camera_native persisted as 'worker'"
  # A ticket is bound to its camera: re-pointing the body at another camera is a conflict.
  enf_pass "$(enf_code -X POST "$API/api/v1/ai/events" -H "X-API-Key: $ENF_KEY" \
    -H 'content-type: application/json' \
    -d "{\"frame_ticket\":\"$ENF_TICKET\",\"camera_id\":\"cam_tls_2\",\"task_type\":\"anpr\",\"detections\":[]}")" 409 \
    "a ticket re-pointed at another camera -> 409"
fi

if [ "$ENF_FAIL" != "0" ]; then
  echo "[e2e_tls] enforce leg FAILED — ingest provenance/lease enforcement regressed" >&2
  exit 1
fi
echo "[e2e_tls] enforce leg OK"

# Caddy starts LAST, once every camera is registered. That ordering is the readiness contract: the
# suite gates on HTTPS /healthz, so if TLS answers at all, seeding has already finished. Starting it
# earlier would let Playwright begin against a half-seeded stack.
#
# Its config is DERIVED FROM THE SHIPPED deploy/Caddyfile rather than hand-written here, because the
# point of the suite is to prove THAT file routes the media plane. Only the upstream address changes
# (the real one fronts nginx on :8080; this stack has the core serve the dashboard directly), plus
# disabling the :80 redirect so CI needs no privileged port. Drop the /live/* handlers from
# deploy/Caddyfile and this suite fails — which is exactly the intent.
echo "[e2e_tls] Caddy (HTTPS :$TLS_PORT, self-signed internal CA)"
CADDYFILE="$DATA/Caddyfile"
sed -e "s|reverse_proxy 127.0.0.1:8080|reverse_proxy 127.0.0.1:${PORT}|" \
    -e "s|^	admin off|	admin off\\
	auto_https disable_redirects|" \
    "$ROOT/deploy/Caddyfile" >"$CADDYFILE"
grep -q "handle_path /live/hls/\*" "$CADDYFILE" || {
  echo "[e2e_tls] deploy/Caddyfile no longer routes /live/hls — live view would be unreachable over TLS" >&2
  exit 1
}
grep -q "handle_path /live/whep/\*" "$CADDYFILE" || {
  echo "[e2e_tls] deploy/Caddyfile no longer routes /live/whep — WebRTC signalling would be unreachable over TLS" >&2
  exit 1
}
# XDG_* keeps the generated internal-CA root inside the scratch dir instead of the user's real Caddy
# data, so a dev machine is not left with a stray trusted CA.
XDG_DATA_HOME="$DATA/caddy-data" XDG_CONFIG_HOME="$DATA/caddy-config" \
HELDAR_TLS_DOMAIN="localhost:${TLS_PORT}" HELDAR_TLS_ISSUER=internal \
"$CADDY" run --config "$CADDYFILE" --adapter caddyfile >"$LOG/caddy.log" 2>&1 & PIDS+=($!)

for _ in $(seq 1 40); do curl -fsSk "https://localhost:${TLS_PORT}/healthz" >/dev/null 2>&1 && break; sleep 1; done
curl -fsSk "https://localhost:${TLS_PORT}/healthz" >/dev/null 2>&1 || {
  echo "[e2e_tls] Caddy did not come up"; tail -30 "$LOG/caddy.log"; exit 1; }

echo "[e2e_tls] ready: $NCAMS cameras, dashboard on https://localhost:${TLS_PORT} (self-signed). Waiting…"
wait "$CORE_PID"
