# Remote Access

How to view a Heldar deployment from outside its LAN — when the site is behind **CGNAT** (the
common case for home/small-site internet: a shared public IPv4, no inbound port-forward, DDNS
useless). This is an **open kernel** capability: every deployment of the Apache-2.0 kernel gets
private, peer-to-peer-first remote viewing out of the box.

---

## TL;DR

- **Transport:** a **WireGuard overlay** running as an external daemon on the host.
  - **Your own / dev use → Tailscale** (Personal, free): zero servers, near-zero ops, $0.
  - **Shipped product (paying clients) → NetBird self-hosted**: a single container per deployment,
    no per-seat licensing, no third-party metadata — fits the single-tenant-per-deployment model.
- **Why an overlay (not port-forward / DDNS / a public reverse proxy):** CGNAT blocks all inbound,
  so the only thing that works is the local node **dialing out**. An overlay does exactly that, and
  is **P2P-first** (direct hole-punch / IPv6 when possible; an encrypted relay only as fallback) and
  **end-to-end encrypted** (no relay can ever decrypt your camera video).
- **The kernel's role is thin:** it does **not** embed or manage WireGuard. It reports whether the
  configured overlay interface is up (`/api/v1/system → remote_access`) so the dashboard can show
  remote-access health. The overlay is orthogonal to the media stack.

---

## Why CGNAT forces this design

| Approach | Works behind CGNAT? | Why |
| --- | --- | --- |
| Port-forward a public IP | ❌ | No dedicated public IPv4; you can't forward a port on the carrier's shared NAT. |
| DDNS | ❌ | Nothing to point a hostname at; still can't accept inbound. |
| IPv6 direct | ⚠️ sometimes | The site usually has routable IPv6, but **viewer-side** v6 is unreliable (mobile/hotel/corporate are often v4-only). Good when both ends have it; can't be the only path. |
| **Outbound overlay (WireGuard)** | ✅ | The node dials **out** to a coordinator; CGNAT always allows outbound. Reachability needs no inbound port. |

Carrier CGNAT is typically **symmetric NAT**, which defeats plain STUN hole-punching. This is why a
naive WebRTC/STUN setup ends up relaying through TURN most of the time, and why the overlay's smarter
NAT traversal (hole-punch **+ UDP port prediction**) matters — it recovers a **direct** path in the
common case (symmetric CGNAT camera side + endpoint-independent home/mobile viewer), keeping the
relay a true fallback rather than the primary path.

## P2P-first, and "no proxy reads my bytes"

Two layers, deliberately separate:

- **Reachability (control + media transport): the overlay.** WireGuard establishes a direct
  encrypted tunnel between the viewer's device and the camera-site host whenever NAT traversal
  succeeds. When the hostile case hits (symmetric NAT on *both* ends), it falls back to a relay
  (Tailscale DERP / NetBird relay) **that forwards only ciphertext** — it can never decrypt the
  traffic. So even on the fallback path, no proxy sees your video.
- **Media:** MediaMTX keeps serving its normal **WebRTC (WHEP)** / HLS on its normal ports; over the
  overlay it's reachable at the host's overlay address. WebRTC media is itself DTLS-SRTP encrypted,
  a second independent layer.

**Net:** content privacy is strong on every path. The only thing a managed coordinator (Tailscale
Inc.) sees is **connection metadata** (device keys/names, the camera + viewer public IPs, timestamps,
ACL topology) — never video, URLs, or camera credentials. Self-hosting the coordinator (NetBird /
Headscale) removes even that third-party metadata, at the cost of a small VPS to run + patch.

> The genuine tradeoff: you cannot have **zero-ops + zero-cost + zero-third-party-metadata**
> simultaneously. Managed Tailscale gives the first two; self-hosting buys the third with a VPS.

---

## Recipe A — Tailscale (personal / dev): lowest maintenance, $0

1. Install the Tailscale client on the **camera-site host** (the box running the kernel + MediaMTX)
   and on each **viewer device**; log in via OIDC. Enable MagicDNS for a stable host name.
2. Lock it down with a Tailscale **ACL** so viewers can reach **only** the media/control ports, not
   the whole host:
   ```jsonc
   // tag:viewer may reach the camera host on MediaMTX WHEP/HLS + the kernel API only
   "acls": [
     { "action": "accept", "src": ["tag:viewer"], "dst": ["tag:cctv:8889,8888,8000"] }
   ]
   ```
3. Point the kernel at the interface so the dashboard shows status:
   ```bash
   HELDAR_OVERLAY_ENABLED=true
   HELDAR_OVERLAY_KIND=tailscale
   HELDAR_OVERLAY_IFACE=tailscale0
   ```
4. Viewers open the host's MagicDNS name in a normal browser — WHEP plays P2P, sub-second latency.

**Note:** Tailscale **Personal is non-commercial only.** Do not ship it to paying clients — use
Recipe B for the product.

## Recipe B — NetBird self-hosted (product): no third-party metadata, no per-seat cost

1. Run a single `netbird-server` container (management + signal + the built-in WebSocket relay) on a
   small VPS (~€4/mo), or co-located on the deployment's cloud gateway. Wire OIDC.
2. Enroll the camera-site host and viewer devices; restrict reach with NetBird ACLs to the same
   media/control ports as above.
3. Configure the kernel:
   ```bash
   HELDAR_OVERLAY_ENABLED=true
   HELDAR_OVERLAY_KIND=netbird
   HELDAR_OVERLAY_IFACE=wt0
   ```

This keeps the device graph/metadata on infrastructure **you** control, WireGuard E2E end to end,
and a managed-like UX — at materially lower ops than Headscale (which lags upstream protocol changes).
Headscale + a self-hosted DERP is the maximal-privacy variant, but the highest ops.

## Recipe C — kernel-managed WireGuard (built in): zero extra software, IPv6-first

For deployments that **have a reachable endpoint** — a routable IPv6 (common) or a public IPv4 / port-
forward — Heldar can manage its own WireGuard interface directly, with no Tailscale/NetBird daemon and
no coordinator. Enable the off-by-default `wireguard` cargo feature:

1. Build/run with the feature and grant the binary network privilege once:
   ```bash
   cargo build -p heldar-server --features wireguard
   sudo setcap cap_net_admin,cap_net_raw+eip ./target/debug/heldar-core
   ```
2. Turn it on:
   ```bash
   HELDAR_WG_MANAGED=true
   # all optional — sensible non-conflicting values are auto-selected otherwise:
   # HELDAR_WG_IFACE=heldar0   HELDAR_WG_SUBNET=10.200.0.0/24   HELDAR_WG_PORT=51820
   # HELDAR_WG_ENDPOINT=[2001:db8::1]:51820   # defaults to the host's public IPv6
   ```
   On boot the kernel brings up its own interface, **auto-selecting a name / subnet / port that don't
   collide with anything already on the host** (the LAN, Docker bridges, *other WireGuard tunnels*). It
   touches only the interface it creates plus that interface's own scope route — never an existing
   interface, the default route, or DNS.
3. Enroll a device in the dashboard (**Remote** page) or via `POST /api/v1/remote-access/peers`; import
   the returned `.conf` into the device's WireGuard app and connect. The dashboard + recorded playback
   are reachable at the host's WireGuard address (e.g. `http://10.200.0.1:8000`). For **live** video
   over the tunnel, also set `HELDAR_MEDIAMTX_HLS_BASE`/`_WEBRTC_BASE` to that same WireGuard host IP so
   the browser fetches streams over the tunnel rather than its own localhost.

**When to prefer this vs Recipe A/B:** kernel-managed plain WireGuard needs **one side to be reachable**
(IPv6 endpoint, or a public IPv4 / port-forward). It does **no** hole-punching or relay, so behind
**dual** CGNAT with no IPv6 it can't connect — use Tailscale/NetBird (which relay-fallback) there. It is
the lightest option when the host has a routable endpoint (this is the common IPv6 case).

---

## Security: authenticate before any exposure

The kernel/MediaMTX read + playback surface is **not** authenticated by default (LAN-appliance
default). On a tailnet/overlay an ACL gates who can reach it, which is sufficient for a handful of
trusted viewers. But:

- Add the signed read tokens for MediaMTX playback before exposing it even on the overlay.
- For **any** public endpoint (a Cloudflare/VPS browser gateway, if ever added), authentication in
  front of the endpoint is **mandatory** — never expose the media surface publicly unauthenticated.

## What the kernel provides (and what it doesn't)

- **Provides (open, Apache-2.0), always on:** overlay-status *awareness* for an external daemon —
  config (`HELDAR_OVERLAY_*`) and a probe of the configured interface (`/sys/class/net/<iface>`),
  surfaced at `GET /api/v1/system → remote_access { enabled, kind, iface, present, operstate, up, note }`.
  Transport-agnostic: any overlay that presents a network interface is supported (Recipes A/B).
- **Provides (open, Apache-2.0), behind the `wireguard` feature:** a kernel-*managed* WireGuard
  interface (Recipe C) — collision-free auto-allocation, peer enrollment, and status at
  `GET /api/v1/remote-access`. Off by default; needs `CAP_NET_ADMIN`. It manages only its own
  interface and never disturbs existing host networking.
- **Does not provide:** NAT hole-punching or a relay/coordinator (so dual-CGNAT-without-IPv6 still
  wants Tailscale/NetBird), nor any managed hosted multi-site control plane — that would live in a
  proprietary crate; the default open path needs none of it.

## Alternatives (and why they're not the default)

- **Cloudflare-native (Tunnel + Workers/D1/KV + WebRTC via Cloudflare Realtime TURN):** zero-install
  *browser* viewing with no client, and effectively free for occasional use — but the Tunnel
  terminates TLS (so Cloudflare reads control-plane payloads + WHEP SDP in plaintext), TURN relay is
  the *primary* media path under CGNAT (metered bandwidth), and it's the most moving parts to build/
  maintain. Choose it only if "send anyone a link, no client install" is a hard requirement.
  ⚠️ Never reverse-proxy the camera **video** over the Tunnel — that violates Cloudflare's CDN ToS;
  media must ride WebRTC/TURN, the Tunnel carries only the small control API + SDP signaling.
- **Self-hosted reverse tunnel (frp/rathole + coturn on your VPS):** best privacy (you own every
  hop) and browser-zero-install, but the heaviest ops (a VPS + three daemons + certs) and not free.
  Use WebRTC+coturn (or TLS passthrough) so the VPS only ever sees ciphertext.
