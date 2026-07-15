# Native (systemd) deployment — the appliance / DVR engine

This is the **recommended way to run Heldar on a real appliance** (a DVR, an edge box, or a flashed OS
image): native binaries under systemd, **no Docker** and **no `sudo` at runtime**. It's the lightest
option — just the `heldar-core` and `mediamtx` processes, no container runtime overhead — which matters
on constrained DVR hardware. `heldar-core` runs as the unprivileged `heldar` user with zero capabilities.

## Why not Docker on the appliance?

Docker is great on a dev box, but on a DVR the Docker daemon + containerd + runc + overlay storage are
exactly the load you don't want. systemd is already PID 1, so it supervises the binaries directly with no
container runtime in the path.

> A Docker image is **not** a bootable/flashable disk image. To "flash Heldar as the DVR OS" you build an
> OS image (a rootfs) that bundles these binaries + units — see "Building a flashable image" below.

## Install (done once, by whoever builds the image — not the end user)

```bash
# 1. Binaries
cargo build --release -p heldar-server   # add --features smtp for the email notifier, etc.
install -m755 target/release/heldar-core /usr/local/bin/heldar-core
install -m755 infra/mediamtx/mediamtx    /usr/local/bin/mediamtx   # or download the upstream binary

# 2. Dependencies the binary shells out to
#    ffmpeg (recorder/clip/snapshot)
apt-get install -y ffmpeg

# 3. Service user + config
useradd -r -s /usr/sbin/nologin heldar || true
install -d /etc/heldar
install -m644 infra/mediamtx/mediamtx.yml /etc/heldar/mediamtx.yml
install -m600 infra/systemd/heldar.env.example /etc/heldar/heldar.env   # edit for your deployment

# 4. Dashboard (the kernel serves the built SPA itself — no nginx needed)
(cd apps/web && npm ci && npm run build)
install -d /usr/local/share/heldar/web
cp -r apps/web/dist/. /usr/local/share/heldar/web/
#    then set HELDAR_WEB_DIR=/usr/local/share/heldar/web in /etc/heldar/heldar.env — without it the
#    box boots API-only, with no UI.

# 5. Units
install -m644 infra/systemd/heldar-core.service infra/systemd/mediamtx.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now mediamtx heldar-core
```

The end user who flashes/boots the resulting image never runs any of this and never types `sudo` — the
service comes up unprivileged under systemd at boot.

## Remote access

Browser-based remote viewing is delivered over WebRTC (signaling + TURN in the control-plane, video via
MediaMTX/WHEP) — see `docs/REMOTE-ACCESS.md` — and needs no special capabilities on the appliance. Self-
hosters who prefer a private overlay can run a Tailscale/NetBird/wg daemon alongside Heldar; `heldar-core`
only *observes* that interface (overlay awareness) and never manages it.

## Backups

`heldar-db-backup.timer` (+ `.service`) takes a daily online snapshot of the metadata DB via
`heldar-core backup-db` into `/var/lib/heldar/backups`, keeping the 14 newest. Enable it with
`systemctl enable --now heldar-db-backup.timer` (the flashable image enables it by default). The DB holds
users, the audit trail, camera config, and sealed credentials — only video is otherwise backed up — so
copy the snapshots offsite and store `HELDAR_SECRET_KEY` separately. See `docs/PRODUCTION.md`.

## Building a flashable image

The units above make Heldar a native service; turning that into a bootable DVR OS is a packaging step.
Common routes, lightest first:

- **debootstrap / mmdebstrap** — a minimal Debian rootfs + these binaries + units, written to a disk
  image. Quickest to stand up; good for x86 / Raspberry Pi-class boards.
- **Buildroot** — a tiny, fully-custom embedded rootfs (tens of MB). Best fit for low-resource DVR SoCs.
- **Yocto** — heavier tooling, most control; worth it for a product line across many boards.

Ask if you want a minimal image-build scaffold (debootstrap is the fastest to demonstrate).
