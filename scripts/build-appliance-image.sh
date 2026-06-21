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

# ---- 2. Stage the overlay (files to drop into the rootfs verbatim) ----
say "stage overlay → $STAGE"
rm -rf "$STAGE"
install -D -m755 "$BIN" "$STAGE/usr/local/bin/heldar-core"
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

# ---- 3. Build the rootfs: base packages + runtime deps + overlay + first-boot setup ----
say "mmdebstrap $SUITE/$ARCH → $ROOTFS_TAR"
mkdir -p "$OUT"
mmdebstrap \
  --variant=minbase \
  --architectures="$ARCH" \
  --include=systemd,systemd-sysv,dbus,udev,ffmpeg,ca-certificates,curl \
  --customize-hook='
    # service user + data dir
    chroot "$1" useradd -r -s /usr/sbin/nologin heldar || true
    install -d -o heldar -g heldar "$1/var/lib/heldar"
    # enable services at boot
    chroot "$1" systemctl enable heldar-core.service mediamtx.service
    # a basic hostname/login so the image is usable (override for your fleet)
    echo heldar > "$1/etc/hostname"
  ' \
  --setup-hook='cp -a '"$STAGE"'/. "$1/"' \
  "$SUITE" "$ROOTFS_TAR"

say "DONE"
cat <<EOF
Rootfs: $ROOTFS_TAR

Test it (no flashing) with systemd-nspawn:
  sudo mkdir -p /tmp/heldar-rootfs && sudo tar -C /tmp/heldar-rootfs -xf "$ROOTFS_TAR"
  sudo systemd-nspawn -D /tmp/heldar-rootfs --boot

Turn it into a bootable DVR image (board-specific, the remaining TODO):
  1. Create a partitioned disk image (e.g. via 'genimage' or manual: parted + mkfs).
  2. Unpack this rootfs onto the root partition.
  3. Add a kernel + bootloader for your SoC (u-boot/extlinux for ARM DVRs; GRUB for x86).
  4. Set up fstab + a first-boot resize. Flash with dd / bmaptool.
EOF
