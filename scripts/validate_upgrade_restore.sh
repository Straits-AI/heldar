#!/usr/bin/env bash
# Upgrade + disaster-recovery qualification.
#
# Two drills the audit called release blockers, because "fresh install works" stops being the
# interesting question once a product has shipped migrations and a backup feature:
#
#   A. UPGRADE — boot a PREVIOUS RELEASE's binary against a fresh database, seed real rows through its
#      API, then boot the CURRENT build against that same database. Migrations must apply and the
#      seeded data must survive. This is the only test that exercises the migration chain against a
#      database an older release actually wrote, rather than one the current schema created.
#
#   B. RESTORE — seed, take an online backup with `heldar-core backup-db`, DESTROY the database, restore
#      the snapshot, and boot again. A backup feature that has never been restored is half-built; this
#      turns "we have backups" into "we have restores".
#
# Self-contained in the style of validate_rbac.sh: throwaway ports, temp data dirs, recorder and AI off,
# MediaMTX pointed at a dead port. Never touches the main stack or its database. Prints PASS/FAIL and
# EXITS NON-ZERO on any failure, so CI cannot go green on a broken upgrade path.
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CORE="$ROOT/target/debug/heldar-core"
DATA="${HELDAR_DATA_DIR:-$ROOT/data}"
mkdir -p "$DATA"
REPORT="$DATA/validate_upgrade_restore.txt"
: > "$REPORT"

# Releases to upgrade FROM. Oldest first. Override to test a specific chain.
FROM_RELEASES="${UPGRADE_FROM:-v0.3.1 v0.4.1}"

TMP=$(mktemp -d)
PIDS=()
cleanup() { for p in "${PIDS[@]:-}"; do kill "$p" 2>/dev/null; done; sleep 1; rm -rf "$TMP"; }
trap cleanup EXIT INT TERM

FAILS=0
log(){ echo "$@" | tee -a "$REPORT"; }
pass(){ if [ "$1" = "$2" ]; then log "  PASS $3 ($1)"; else log "  FAIL $3 (got $1, want $2)"; FAILS=$((FAILS+1)); fi; }
code(){ curl -s -o /dev/null -w '%{http_code}' "$@"; }
jqget(){ python3 -c "import sys,json;d=json.load(sys.stdin);print($1)" 2>/dev/null; }

[ -x "$CORE" ] || { log "FAIL missing $CORE — run: cargo build --bin heldar-core"; exit 1; }

# Boot a core and wait for /healthz. $1=binary $2=port $3=data dir $4=log file. Returns non-zero if it
# never comes up, so a failed boot is a reported FAIL rather than a hang.
boot_core() {
  local bin="$1" port="$2" dir="$3" logf="$4"
  HELDAR_DATA_DIR="$dir" \
  HELDAR_DATABASE_URL="sqlite://$dir/heldar.db" \
  HELDAR_API_PORT="$port" \
  HELDAR_API_HOST=127.0.0.1 \
  HELDAR_RECORDER_ENABLED=false \
  HELDAR_AI_ENABLED=false \
  HELDAR_MEDIAMTX_API_URL=http://127.0.0.1:65599 \
  "$bin" >"$logf" 2>&1 &
  local pid=$!
  PIDS+=("$pid")
  for _ in $(seq 1 45); do
    [ "$(code "http://127.0.0.1:$port/healthz")" = "200" ] && { echo "$pid"; return 0; }
    # A dead process will never answer; fail fast instead of burning the whole window.
    kill -0 "$pid" 2>/dev/null || { echo ""; return 1; }
    sleep 1
  done
  echo ""
  return 1
}

stop_core() {
  local pid="$1"
  [ -n "$pid" ] || return 0
  kill "$pid" 2>/dev/null
  # Wait for the port to actually free; a half-dead core makes the next boot look broken.
  for _ in $(seq 1 20); do kill -0 "$pid" 2>/dev/null || return 0; sleep 1; done
  kill -9 "$pid" 2>/dev/null
}

seed_camera() {
  local port="$1" id="$2"
  curl -fsS -X POST "http://127.0.0.1:$port/api/v1/cameras" -H 'content-type: application/json' \
    -d "{\"id\":\"$id\",\"name\":\"Drill $id\",\"vendor\":\"generic\",
         \"main_stream_url\":\"rtsp://127.0.0.1:8554/$id\",\"record_stream\":\"main\"}" >/dev/null 2>&1
}

camera_count() {
  curl -fsS "http://127.0.0.1:$1/api/v1/cameras" 2>/dev/null | jqget 'len(d)'
}

# ---------------------------------------------------------------------------------------------------
# A. UPGRADE DRILL
# ---------------------------------------------------------------------------------------------------
log "== A. upgrade drill: previous release -> current build =="
OS="$(uname -s)"
ARCH="$(uname -m)"
if [ "$OS" != "Linux" ] || { [ "$ARCH" != "x86_64" ] && [ "$ARCH" != "aarch64" ]; }; then
  # Released binaries are linux-musl only. SKIP loudly rather than silently passing — a skipped drill
  # that reads as a pass is how an untested upgrade path ships.
  log "  SKIP upgrade drill: released binaries are linux-musl only (this host is $OS/$ARCH)"
  log "       run it in CI, or set UPGRADE_FROM= and run on Linux"
else
  case "$ARCH" in
    x86_64) RARCH=x86_64 ;;
    aarch64) RARCH=aarch64 ;;
  esac
  for tag in $FROM_RELEASES; do
    log ""
    log "-- upgrading from $tag --"
    OLD_BIN="$TMP/heldar-core-$tag"
    URL="https://github.com/Straits-AI/heldar/releases/download/$tag/heldar-core-$tag-$RARCH-linux-musl"
    if ! curl -fsSL --retry 5 --retry-delay 3 --retry-all-errors -o "$OLD_BIN" "$URL"; then
      log "  FAIL could not download $tag binary ($URL)"
      FAILS=$((FAILS+1))
      continue
    fi
    chmod +x "$OLD_BIN"

    OLD_DIR="$TMP/upgrade-$tag"; mkdir -p "$OLD_DIR"
    OLD_PORT=8021
    log "  booting $tag on :$OLD_PORT"
    OLD_PID=$(boot_core "$OLD_BIN" "$OLD_PORT" "$OLD_DIR" "$TMP/old-$tag.log")
    if [ -z "$OLD_PID" ]; then
      log "  FAIL $tag did not start"; FAILS=$((FAILS+1)); tail -20 "$TMP/old-$tag.log" | tee -a "$REPORT"
      continue
    fi
    seed_camera "$OLD_PORT" "cam_upgrade_a"
    seed_camera "$OLD_PORT" "cam_upgrade_b"
    pass "$(camera_count "$OLD_PORT")" "2" "$tag seeded 2 cameras before upgrade"
    stop_core "$OLD_PID"

    # THE UPGRADE: same data dir, current binary. Migrations run at boot (sqlx::migrate!), so a broken
    # chain shows up as a core that never becomes healthy.
    NEW_PORT=8022
    log "  booting CURRENT build on :$NEW_PORT against $tag's database"
    NEW_PID=$(boot_core "$CORE" "$NEW_PORT" "$OLD_DIR" "$TMP/new-$tag.log")
    if [ -z "$NEW_PID" ]; then
      log "  FAIL current build did not start on $tag's database (migration failure?)"
      FAILS=$((FAILS+1)); tail -30 "$TMP/new-$tag.log" | tee -a "$REPORT"
      continue
    fi
    pass "$(camera_count "$NEW_PORT")" "2" "$tag data survived the upgrade"
    # The upgraded box must be functional, not merely booted.
    pass "$(code "http://127.0.0.1:$NEW_PORT/api/v1/system")" "200" "$tag upgraded box serves /system"
    pass "$(code "http://127.0.0.1:$NEW_PORT/readyz")" "200" "$tag upgraded box is ready"
    stop_core "$NEW_PID"
  done
fi

# ---------------------------------------------------------------------------------------------------
# B. RESTORE DRILL
# ---------------------------------------------------------------------------------------------------
log ""
log "== B. backup -> destroy -> restore drill =="
RDIR="$TMP/restore"; mkdir -p "$RDIR"
RPORT=8023
RPID=$(boot_core "$CORE" "$RPORT" "$RDIR" "$TMP/restore.log")
if [ -z "$RPID" ]; then
  log "  FAIL core did not start for the restore drill"; FAILS=$((FAILS+1)); tail -20 "$TMP/restore.log" | tee -a "$REPORT"
else
  seed_camera "$RPORT" "cam_restore_a"
  seed_camera "$RPORT" "cam_restore_b"
  seed_camera "$RPORT" "cam_restore_c"
  pass "$(camera_count "$RPORT")" "3" "seeded 3 cameras before backup"

  # Online snapshot while the core is RUNNING — that is the supported path (a plain file copy of a live
  # SQLite database is exactly what the production guide warns against).
  SNAP="$TMP/snapshot.db"
  if HELDAR_DATA_DIR="$RDIR" HELDAR_DATABASE_URL="sqlite://$RDIR/heldar.db" \
     "$CORE" backup-db "$SNAP" >"$TMP/backup.log" 2>&1; then
    log "  PASS backup-db wrote a snapshot ($(wc -c <"$SNAP" | tr -d ' ') bytes)"
  else
    log "  FAIL backup-db failed"; FAILS=$((FAILS+1)); tail -10 "$TMP/backup.log" | tee -a "$REPORT"
  fi
  stop_core "$RPID"

  # DESTROY. Not "overwrite" — remove the database and its WAL/SHM so the restore cannot be a no-op
  # that silently reads the original file.
  rm -f "$RDIR"/heldar.db "$RDIR"/heldar.db-wal "$RDIR"/heldar.db-shm
  [ -f "$RDIR/heldar.db" ] && { log "  FAIL database was not destroyed"; FAILS=$((FAILS+1)); }

  # Prove the destruction was real: a fresh boot on the wiped dir must have ZERO cameras. Without this
  # control, a restore that quietly did nothing would still "pass" the count check below.
  CPORT=8024
  CPID=$(boot_core "$CORE" "$CPORT" "$RDIR" "$TMP/control.log")
  if [ -n "$CPID" ]; then
    pass "$(camera_count "$CPORT")" "0" "CONTROL: the wiped database really is empty"
    stop_core "$CPID"
  else
    log "  FAIL control boot on the wiped database failed"; FAILS=$((FAILS+1))
  fi
  rm -f "$RDIR"/heldar.db "$RDIR"/heldar.db-wal "$RDIR"/heldar.db-shm

  # RESTORE + verify.
  cp "$SNAP" "$RDIR/heldar.db"
  VPORT=8025
  VPID=$(boot_core "$CORE" "$VPORT" "$RDIR" "$TMP/verify.log")
  if [ -z "$VPID" ]; then
    log "  FAIL core did not start on the restored database"; FAILS=$((FAILS+1)); tail -20 "$TMP/verify.log" | tee -a "$REPORT"
  else
    pass "$(camera_count "$VPORT")" "3" "all 3 cameras came back from the snapshot"
    pass "$(code "http://127.0.0.1:$VPORT/api/v1/cameras/cam_restore_b")" "200" "a specific camera is readable after restore"
    pass "$(code "http://127.0.0.1:$VPORT/readyz")" "200" "restored box is ready"
    stop_core "$VPID"
  fi
fi

log ""
if [ "$FAILS" -ne 0 ]; then
  log "UPGRADE/RESTORE VALIDATION FAILED: $FAILS check(s)"
  exit 1
fi
log "UPGRADE/RESTORE VALIDATION PASSED: all checks green"
