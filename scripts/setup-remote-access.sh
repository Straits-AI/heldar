#!/usr/bin/env bash
# Privileged setup for kernel-managed WireGuard remote access. Run with sudo:
#
#   sudo scripts/setup-remote-access.sh           # grant the cap + preview what core will allocate
#   sudo scripts/setup-remote-access.sh --run      # ...then launch core (as YOUR user, not root)
#   sudo scripts/setup-remote-access.sh --down      # remove the managed interface (cleanup)
#
# What needs root here is ONE thing: setcap on the heldar-core binary, so it can manage its OWN
# WireGuard interface without running as root. This script does NOT touch wg0, the default route, DNS,
# or any existing interface. The managed interface is only created later, when core runs with
# HELDAR_WG_MANAGED=true (which `--run` does, dropping back to your user so files aren't root-owned).
set -euo pipefail

# --- must be root, but we need the real (non-root) user to build/run as ---
if [ "$(id -u)" -ne 0 ]; then
  echo "Run me with sudo:  sudo $0 [--run|--down]" >&2
  exit 1
fi
REAL_USER="${SUDO_USER:-}"
if [ -z "$REAL_USER" ] || [ "$REAL_USER" = "root" ]; then
  echo "WARNING: SUDO_USER is unset — running core as root creates root-owned files in ./data." >&2
  echo "         Prefer: sudo -u <you> ... or invoke this via 'sudo' from your normal account." >&2
fi

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${HELDAR_CORE_BIN:-$REPO/target/debug/heldar-core}"
IFACE="${HELDAR_WG_IFACE:-heldar0}"
MODE="${1:-}"

say() { printf '\n\033[1m== %s ==\033[0m\n' "$*"; }

# ---- --down: remove ONLY the managed interface (cleanup) ----
if [ "$MODE" = "--down" ]; then
  say "teardown"
  if ip link show "$IFACE" >/dev/null 2>&1; then
    ip link del dev "$IFACE"
    echo "  removed $IFACE"
  else
    echo "  $IFACE not present — nothing to do"
  fi
  exit 0
fi

# ---- binary must exist (build it as your user, not root, so target/ stays yours) ----
say "binary"
if [ ! -x "$BIN" ]; then
  echo "ERROR: $BIN not found." >&2
  echo "Build it first AS YOUR USER (not sudo):" >&2
  echo "  cargo build -p heldar-server --features wireguard" >&2
  exit 1
fi
echo "  $BIN"

# ---- grant CAP_NET_ADMIN (+net_raw) on the binary; verify ----
say "setcap CAP_NET_ADMIN"
setcap 'cap_net_admin,cap_net_raw+eip' "$BIN"
echo "  getcap: $(getcap "$BIN")"
echo "  (re-run this after any 'cargo build' — a rebuild replaces the binary and drops the cap)"

# ---- read-only preview of what core will allocate (core decides authoritatively at boot) ----
say "allocation preview (read-only; coexists with your existing network)"
INUSE="$( { ip -o -4 addr show; ip -o -4 route show; } 2>/dev/null \
          | grep -oE '([0-9]{1,3}\.){3}[0-9]{1,3}/[0-9]+' | sort -u )"
echo "  in-use IPv4 ranges (avoided):"; echo "$INUSE" | sed 's/^/    /'
# interface: heldar0 unless taken
PICK_IFACE="$IFACE"
n=0; while ip link show "heldar$n" >/dev/null 2>&1; do n=$((n+1)); PICK_IFACE="heldar$n"; done
# first free 10.<200..254>.0.0/24 not seen in the in-use list (preview heuristic)
PICK_SUBNET=""
for s in $(seq 200 254); do
  if ! echo "$INUSE" | grep -qE "^10\.$s\."; then PICK_SUBNET="10.$s.0.0/24"; break; fi
done
PICK_V6="$(ip -o -6 addr show scope global 2>/dev/null | awk '{print $4}' | sed 's#/.*##' \
           | grep -viE '^(fe80|fc|fd)' | head -1)"
echo "  interface : $PICK_IFACE   (never wg0/existing)"
echo "  subnet    : ${PICK_SUBNET:-<auto>}  -> host ${PICK_SUBNET%%.0/24}.0.1"
echo "  endpoint  : ${PICK_V6:+[$PICK_V6]:51820}${PICK_V6:-<set HELDAR_WG_ENDPOINT: no global IPv6>}"
if ip link show wg0 >/dev/null 2>&1; then echo "  wg0       : present and UNTOUCHED"; fi

# ---- run, or print the command ----
RUN_ENV=(HELDAR_WG_MANAGED=true)
if [ "$MODE" = "--run" ]; then
  say "launching core (HELDAR_WG_MANAGED=true) as ${REAL_USER:-root}"
  echo "  (Ctrl-C to stop; the interface persists — run with --down to remove it.)"
  cd "$REPO"
  if [ -n "$REAL_USER" ] && [ "$REAL_USER" != "root" ]; then
    exec sudo -u "$REAL_USER" env "${RUN_ENV[@]}" "$BIN"
  else
    exec env "${RUN_ENV[@]}" "$BIN"
  fi
else
  say "next step"
  cat <<EOF
  Cap is set. Now start core AS YOUR USER (so ./data isn't root-owned):
    cd '$REPO' && HELDAR_WG_MANAGED=true ./target/debug/heldar-core
  or re-run this script with --run to launch it for you.

  Then: dashboard -> Remote -> enroll your device -> import the .conf -> connect ->
  browse http://<host-wg-ip>:8000  (e.g. http://10.200.0.1:8000).

  For LIVE video over the tunnel, also export before starting core:
    HELDAR_MEDIAMTX_HLS_BASE=http://<host-wg-ip>:8888
    HELDAR_MEDIAMTX_WEBRTC_BASE=http://<host-wg-ip>:8889
EOF
fi
