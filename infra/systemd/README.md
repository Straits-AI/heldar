# Native (systemd) deployment — the appliance / DVR engine

This is the **recommended way to run Heldar on a real appliance** (a DVR, an edge box, or a flashed OS
image): native binaries under systemd, **no Docker** and **no `sudo` at runtime**. It's the lightest
option — just the `heldar-core` and `mediamtx` processes, no container runtime overhead — which matters
on constrained DVR hardware.

## Why not Docker on the appliance?

Docker is great on a dev box (it let us grant `CAP_NET_ADMIN` without `sudo setcap`), but on a DVR the
Docker daemon + containerd + runc + overlay storage are exactly the load you don't want. systemd is
already PID 1, so it can grant the one capability the remote-access feature needs **declaratively** in
the unit (`AmbientCapabilities=CAP_NET_ADMIN`) — to a non-root service, with no `setcap` and no `sudo`.

> A Docker image is **not** a bootable/flashable disk image. To "flash Heldar as the DVR OS" you build an
> OS image (a rootfs) that bundles these binaries + units — see "Building a flashable image" below.

## Install (done once, by whoever builds the image — not the end user)

```bash
# 1. Binaries
cargo build --release -p heldar-server --features wireguard   # omit --features for no remote access
install -m755 target/release/heldar-core /usr/local/bin/heldar-core
install -m755 infra/mediamtx/mediamtx    /usr/local/bin/mediamtx   # or download the upstream binary

# 2. Dependencies the binary shells out to
#    ffmpeg (recorder/clip/snapshot), plus for remote access: iproute2 (ip) + wireguard-tools (wg)
apt-get install -y ffmpeg iproute2 wireguard-tools

# 3. Service user + config
useradd -r -s /usr/sbin/nologin heldar || true
install -d /etc/heldar
install -m644 infra/mediamtx/mediamtx.yml /etc/heldar/mediamtx.yml
install -m600 infra/systemd/heldar.env.example /etc/heldar/heldar.env   # edit: HELDAR_WG_MANAGED=true, ...

# 4. Units
install -m644 infra/systemd/heldar-core.service infra/systemd/mediamtx.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now mediamtx heldar-core
```

The end user who flashes/boots the resulting image never runs any of this and never types `sudo` — the
capability is baked into the unit and applied by systemd at boot.

## How the no-sudo privilege works

`heldar-core` runs as the unprivileged `heldar` user. The unit's `AmbientCapabilities=CAP_NET_ADMIN`
tells systemd (which is privileged) to grant just that one capability to the process at launch. The
`ip`/`wg` children inherit it (ambient caps survive `exec`). `CapabilityBoundingSet` caps the service at
those two capabilities and nothing more. If you don't enable remote access, delete both lines — core
then runs with zero capabilities.

The kernel code only ever creates/manages its own `heldar0` interface (auto-selected to avoid every
existing subnet/interface, including any other WireGuard tunnel). It never touches the host's other
interfaces or the default route.

## Building a flashable image

The units above make Heldar a native service; turning that into a bootable DVR OS is a packaging step.
Common routes, lightest first:

- **debootstrap / mmdebstrap** — a minimal Debian rootfs + these binaries + units, written to a disk
  image. Quickest to stand up; good for x86 / Raspberry Pi-class boards.
- **Buildroot** — a tiny, fully-custom embedded rootfs (tens of MB). Best fit for low-resource DVR SoCs.
- **Yocto** — heavier tooling, most control; worth it for a product line across many boards.

Ask if you want a minimal image-build scaffold (debootstrap is the fastest to demonstrate).
