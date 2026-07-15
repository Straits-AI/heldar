#!/usr/bin/env bash
# Scaffold: build a Heldar APPLIANCE rootfs — native binaries under systemd, NO Docker.
#
# Produces a Debian rootfs (tarball) with heldar-core + mediamtx baked in as systemd services, running
# unprivileged under systemd (no setcap, no sudo at boot).
# Boot it to test with systemd-nspawn, or extend it into a board-specific bootable disk image (add a
# kernel + bootloader for your DVR SoC — that part is hardware-specific and left as a TODO below).
#
#   scripts/build-appliance-image.sh [OUT_DIR]      # default: dist/heldar-appliance
#   SUITE=bookworm ARCH=arm64 scripts/build-appliance-image.sh
#
# Needs `mmdebstrap` (apt-get install mmdebstrap) — fast, rootless-capable. Cross-arch (ARCH=arm64)
# additionally needs qemu-user-static + binfmt. Builds the binary on the host first (so install rustup).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${1:-$ROOT/dist/heldar-appliance}"
SUITE="${SUITE:-bookworm}"
ARCH="${ARCH:-$(dpkg --print-architecture 2>/dev/null || echo amd64)}"
STAGE="$OUT/overlay"           # files copied verbatim into the rootfs
ROOTFS_TAR="$OUT/heldar-rootfs-$SUITE-$ARCH.tar"

say() { printf '\n\033[1m== %s ==\033[0m\n' "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }

have mmdebstrap || { echo "ERROR: mmdebstrap not found — apt-get install mmdebstrap" >&2; exit 1; }

# ---- 1. Build the native binary (release) ----
say "build heldar-core (release)"
( cd "$ROOT" && cargo build --release -p heldar-server )
BIN="$ROOT/target/release/heldar-core"
[ -x "$BIN" ] || { echo "ERROR: $BIN missing after build" >&2; exit 1; }

# ---- 1b. Build the dashboard (the appliance serves it from HELDAR_WEB_DIR — an appliance without
# the web UI is an API-only box, useless to the operator it targets) ----
say "build dashboard (apps/web)"
have npm || { echo "ERROR: npm not found — the appliance image needs the dashboard built (install Node.js >= 20)" >&2; exit 1; }
( cd "$ROOT/apps/web" && npm ci && npm run build )
[ -f "$ROOT/apps/web/dist/index.html" ] || { echo "ERROR: apps/web/dist missing after build" >&2; exit 1; }

# ---- 2. Stage the overlay (files to drop into the rootfs verbatim) ----
say "stage overlay → $STAGE"
rm -rf "$STAGE"
install -D -m755 "$BIN" "$STAGE/usr/local/bin/heldar-core"
# dashboard: served by heldar-core itself (tower-http ServeDir) — heldar-core.service sets HELDAR_WEB_DIR here
mkdir -p "$STAGE/usr/local/share/heldar"
cp -a "$ROOT/apps/web/dist" "$STAGE/usr/local/share/heldar/web"
# mediamtx: use the vendored binary if present, else the appliance build downloads it (document per-arch).
if [ -x "$ROOT/infra/mediamtx/mediamtx" ]; then
  install -D -m755 "$ROOT/infra/mediamtx/mediamtx" "$STAGE/usr/local/bin/mediamtx"
else
  echo "  NOTE: infra/mediamtx/mediamtx absent — fetch the $ARCH mediamtx binary into the image yourself."
fi
install -D -m644 "$ROOT/infra/mediamtx/mediamtx.yml"        "$STAGE/etc/heldar/mediamtx.yml"
install -D -m600 "$ROOT/infra/systemd/heldar.env.example"  "$STAGE/etc/heldar/heldar.env"
install -D -m644 "$ROOT/infra/systemd/heldar-core.service" "$STAGE/etc/systemd/system/heldar-core.service"
install -D -m644 "$ROOT/infra/systemd/mediamtx.service"    "$STAGE/etc/systemd/system/mediamtx.service"
install -D -m644 "$ROOT/infra/systemd/heldar-db-backup.service" "$STAGE/etc/systemd/system/heldar-db-backup.service"
install -D -m644 "$ROOT/infra/systemd/heldar-db-backup.timer"   "$STAGE/etc/systemd/system/heldar-db-backup.timer"

# ---- 3. Build the rootfs: base packages + runtime deps + overlay + first-boot setup ----
say "mmdebstrap $SUITE/$ARCH → $ROOTFS_TAR"
mkdir -p "$OUT"
mmdebstrap \
  --variant=minbase \
  --architectures="$ARCH" \
  `# tzdata: recording schedules are evaluated in the server's local timezone (chrono::Local); without` \
  `# tzdata (and with TZ unset) Local silently becomes UTC, so schedules fire at the wrong wall time.` \
  --include=systemd,systemd-sysv,dbus,udev,ffmpeg,ca-certificates,curl,tzdata \
  --customize-hook='
    # service user + data dir
    chroot "$1" useradd -r -s /usr/sbin/nologin heldar || true
    install -d -o heldar -g heldar "$1/var/lib/heldar"
    # enable services at boot
    chroot "$1" systemctl enable heldar-core.service mediamtx.service heldar-db-backup.timer
    # a basic hostname/login so the image is usable (override for your fleet)
    echo heldar > "$1/etc/hostname"
    # First-boot security banner (shown on every console/SSH login): this image ships LAN defaults
    # (auth OFF, API on 0.0.0.0) by design, so force a conscious decision before it touches a wider net.
    cat > "$1/etc/motd" <<MOTD
============================================================
 Heldar appliance — LAN DEFAULTS (NOT hardened)
 Auth is OFF and the API listens on 0.0.0.0 by design, for a
 trusted local network. Before exposing this box beyond a
 trusted segment: set HELDAR_AUTH_ENABLED=true (plus the rest
 of /etc/heldar/heldar.env) and put it behind a firewall.
 See docs/PRODUCTION.md.
============================================================
MOTD
  ' \
  --setup-hook='cp -a '"$STAGE"'/. "$1/"' \
  "$SUITE" "$ROOTFS_TAR"

say "DONE"
cat <<EOF
Rootfs: $ROOTFS_TAR

SECURITY: this image ships LAN defaults (auth OFF, API on 0.0.0.0). Before attaching the flashed box
to anything but a trusted segment, set HELDAR_AUTH_ENABLED=true in /etc/heldar/heldar.env and firewall
it (see docs/PRODUCTION.md).

Test it (no flashing) with systemd-nspawn:
  sudo mkdir -p /tmp/heldar-rootfs && sudo tar -C /tmp/heldar-rootfs -xf "$ROOTFS_TAR"
  sudo systemd-nspawn -D /tmp/heldar-rootfs --boot

Turn it into a bootable DVR image (board-specific, the remaining TODO):
  1. Create a partitioned disk image (e.g. via 'genimage' or manual: parted + mkfs).
  2. Unpack this rootfs onto the root partition.
  3. Add a kernel + bootloader for your SoC (u-boot/extlinux for ARM DVRs; GRUB for x86).
  4. Set up fstab + a first-boot resize. Flash with dd / bmaptool.
EOF
