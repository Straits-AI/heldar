# Remote Access

How to view a Heldar deployment from outside its LAN — when the site is behind **CGNAT** (the
common case for home/small-site internet: a shared public IPv4, no inbound port-forward, DDNS
useless). This is an **open kernel** capability: every deployment of the Apache-2.0 kernel gets
private remote viewing out of the box.

Remote access is **WebRTC-primary, browser-based** — see
[`docs/adr/0003-webrtc-remote-access.md`](adr/0003-webrtc-remote-access.md) for the design of
record. All phases are **shipped** (P1 live video → P2 universal reach → P3 the full dashboard; see
_Status & phasing_ below). The optional self-hoster **overlay** paths (Recipes A/B) remain available
for operators who prefer full-L3 reach over the hosted rendezvous. Hardening a deployment for the
public internet: [`docs/PRODUCTION.md`](PRODUCTION.md).

---

## TL;DR

- **Primary: WebRTC, in the browser.** A viewer opens the dashboard in any browser and
  the box **dials out** — no inbound port, no client install. Universal NAT traversal comes from
  **signaling + TURN hosted in `heldar-control-plane`**; live video rides **MediaMTX / WHEP**
  (`:8889`). Media is **end-to-end encrypted (DTLS-SRTP)**: the rendezvous brokers only SDP/ICE and
  relayed control, never the video bytes. Design of record:
  [`docs/adr/0003-webrtc-remote-access.md`](adr/0003-webrtc-remote-access.md).
- **Optional (works today): a WireGuard overlay** running as an external daemon on the host, for
  self-hosters who want full L3 reachability rather than just the browser view.
  - **Your own / dev use → Tailscale** (Personal, free): zero servers, near-zero ops, $0.
  - **Shipped product (paying clients) → NetBird self-hosted**: a single container per deployment,
    no per-seat licensing, no third-party metadata — fits the single-tenant-per-deployment model.
- **Why outbound-dialing (not port-forward / DDNS / a public reverse proxy):** CGNAT blocks all
  inbound, so the only thing that reliably works is the local node **dialing out**. Both the WebRTC
  path and an overlay do exactly that. Overlays are additionally **P2P-first** (direct hole-punch /
  IPv6 when possible; an encrypted relay only as fallback) and **end-to-end encrypted** (no relay can
  ever decrypt your camera video).
- **The kernel's role is thin:** it does **not** embed or manage WireGuard. For overlays it reports
  whether the configured interface is up (`/api/v1/system → remote_access`) so the dashboard can show
  remote-access health. The overlay is orthogonal to the media stack.

---

## Why CGNAT forces this design

| Approach | Works behind CGNAT? | Why |
| --- | --- | --- |
| Port-forward a public IP | ❌ | No dedicated public IPv4; you can't forward a port on the carrier's shared NAT. |
| DDNS | ❌ | Nothing to point a hostname at; still can't accept inbound. |
| IPv6 direct | ⚠️ sometimes | The site usually has routable IPv6, but **viewer-side** v6 is unreliable (mobile/hotel/corporate are often v4-only). Good when both ends have it; can't be the only path. |
| Plain WireGuard (manual peer) | ❌ behind CGNAT | A bare WireGuard peer needs **one side reachable** (a routable IPv6 endpoint, or a public IPv4 / port-forward). With dual CGNAT and no IPv6 there's no endpoint to dial and **no built-in hole-punch or relay**, so it can't connect. Works only when you already have an endpoint. |
| **WebRTC (signaling + TURN)** | ✅ | The box dials **out** to the control-plane rendezvous; CGNAT always allows outbound. TURN relays media when a direct path can't be punched. No inbound port. |
| **Outbound overlay (Tailscale / NetBird)** | ✅ | The node dials **out** to a coordinator; CGNAT always allows outbound. P2P-first with an encrypted relay fallback, so it connects even under dual symmetric CGNAT. |

Carrier CGNAT is typically **symmetric NAT**, which defeats plain STUN hole-punching — so a naive
WebRTC/STUN setup ends up relaying through TURN much of the time. The control-plane therefore hosts
TURN as a first-class relay (the universal-reach path), while overlays add smarter NAT traversal
(hole-punch **+ UDP port prediction**) that recovers a **direct** path in the common case (symmetric
CGNAT camera side + endpoint-independent home/mobile viewer), keeping the relay a true fallback.

## P2P-first, and "no proxy reads my bytes"

The privacy guarantee holds on **both** the WebRTC and overlay paths:

- **WebRTC (primary): media is end-to-end over DTLS-SRTP.** The control-plane rendezvous
  only brokers the **SDP/ICE handshake + relayed control** — it never terminates the media. When a
  direct peer path can't be punched, TURN relays **encrypted** packets it cannot read. So even on the
  relayed path, no proxy (and not the control-plane) ever sees decrypted video, URLs, or credentials.
- **Overlay (optional): WireGuard is end-to-end.** The overlay establishes a direct encrypted tunnel
  between the viewer's device and the camera-site host whenever NAT traversal succeeds. When the
  hostile case hits (symmetric NAT on *both* ends), it falls back to a relay (Tailscale DERP /
  NetBird relay) **that forwards only ciphertext** — it can never decrypt the traffic. Over the
  overlay, MediaMTX still serves its normal **WebRTC (WHEP)** / HLS, itself DTLS-SRTP encrypted — a
  second independent layer.

**Net:** content privacy is strong on every path. On the WebRTC path the control-plane sees only
signaling/connection metadata; on a managed overlay the only thing a coordinator (Tailscale Inc.)
sees is **connection metadata** (device keys/names, the camera + viewer public IPs, timestamps, ACL
topology) — never video, URLs, or camera credentials. Self-hosting the coordinator (NetBird /
Headscale) removes even that third-party metadata, at the cost of a small VPS to run + patch.

> The genuine tradeoff for the overlay path: you cannot have **zero-ops + zero-cost +
> zero-third-party-metadata** simultaneously. Managed Tailscale gives the first two; self-hosting
> buys the third with a VPS.

---

## Status & phasing

WebRTC remote access **shipped in three phases, all landed** (full plan in
[`docs/adr/0003-webrtc-remote-access.md`](adr/0003-webrtc-remote-access.md)):

- **P1 — LAN / WHEP ✅:** sub-second live video in the browser over MediaMTX WHEP (`:8889`) on the LAN.
- **P2 — universal reach ✅:** the box dials out to **signaling + TURN**, so the same browser view works
  from anywhere behind CGNAT, no inbound port.
- **P3 — full dashboard ✅:** the complete dashboard (recorded playback, config, events) over the same
  brokered, end-to-end-encrypted path, behind the two-gate cookie auth.

The overlay recipes below remain an alternative for operators who want full-L3 reach (a private network
to the whole host) rather than the browser dashboard. Before exposing a deployment to the public
internet, work through [`docs/PRODUCTION.md`](PRODUCTION.md) (auth, TLS, secrets, lockout, Turnstile).

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

---

## Security: authenticate before any exposure

The kernel/MediaMTX read + playback surface is **not** authenticated by default (LAN-appliance
default). On a tailnet/overlay an ACL gates who can reach it, which is sufficient for a handful of
trusted viewers. But:

- Add the signed read tokens for MediaMTX playback before exposing it even on the overlay.
- For **any** public endpoint (the control-plane WebRTC rendezvous, or a VPS browser gateway),
  authentication in front of the endpoint is **mandatory** — never expose the media surface publicly
  unauthenticated. On the WebRTC path the rendezvous brokers signaling only and never sees media, but
  the signaling/control surface itself must still be authenticated.

## What the kernel provides (and what it doesn't)

- **Provides (open, Apache-2.0), always on:** overlay-status *awareness* for an external daemon —
  config (`HELDAR_OVERLAY_*`) and a probe of the configured interface (`/sys/class/net/<iface>`),
  surfaced at `GET /api/v1/system → remote_access { enabled, kind, iface, present, operstate, up, note }`.
  Transport-agnostic: any overlay that presents a network interface is supported (Recipes A/B).
- **Provides (open, Apache-2.0): the box-side of WebRTC remote access.** Opt-in via
  `HELDAR_REMOTE_RENDEZVOUS_URL`, the kernel **dials OUT** to a rendezvous (`services/webrtc_rendezvous.rs`)
  and bridges browser WHEP offers to its own MediaMTX; it also **programs MediaMTX's ICE servers** so the
  box gathers a relay candidate for symmetric-NAT traversal. Live video is MediaMTX/WHEP; media is
  end-to-end over DTLS-SRTP. Design of record:
  [`docs/adr/0003-webrtc-remote-access.md`](adr/0003-webrtc-remote-access.md).
- **Bring your own TURN, or use ours.** TURN is the only thing that needs hosting:
  - **Your own** — set `HELDAR_WEBRTC_ICE_SERVERS` (a MediaMTX `webrtcICEServers2` JSON array) to point
    MediaMTX at any STUN/TURN you run (coturn, your Cloudflare Realtime, …). The kernel programs it in.
  - **Heldar-hosted** — point `HELDAR_REMOTE_RENDEZVOUS_URL` at the managed `heldar` rendezvous; the
    kernel fetches short-lived TURN from it and refreshes the credentials automatically.
  - Neither set → MediaMTX stays on the STUN baseline (`mediamtx.yml`), i.e. LAN / non-symmetric-NAT only.
- **Provides (open): the full `apps/web` dashboard, remotely** (ADR 0003 P3). The dashboard is served from
  the rendezvous Worker at `/app/?site=<id>`; a second dial-out channel (`webrtc_relay`) + a same-origin
  **cookie reverse-proxy** forward the kernel's REST + media surface (reads, **writes/config**, recorded
  playback) to the box, and live video rides a WHEP-proxy (rendezvous + TURN). It runs under a **two-gate**
  model that keeps the kernel the sole identity/RBAC authority:
  - **Outer gate** — a short-lived, per-user, site-scoped HMAC **relay capability** (the rendezvous mints
    it after a real kernel login; separate `RELAY_CAP_SECRET`) proves the browser may *reach* this box.
  - **Inner gate** — the browser's **real kernel session** is forwarded verbatim; the box replays the
    request against its own `127.0.0.1` kernel, which runs its normal auth + RBAC. The relay is a dumb,
    **allowlisted** pipe (GET reads + login/out only in Stage C; traversal/admin/internal paths refused),
    never an auth-bypass. The kernel session token lives in a Worker-side **HttpOnly** cookie, so browser
    JS never holds it.
  - **Fail-safe:** the relay **refuses to run unless `HELDAR_AUTH_ENABLED=true` and a real user exists** —
    the open auth-off API is never exposed remotely. Pair with a short `HELDAR_SESSION_TTL_HOURS` +
    `HELDAR_SESSION_IDLE_TIMEOUT_MIN` and `HELDAR_AUTH_COOKIE_SECURE=true`.
  - **Recorded playback is HEVC/H.265+ pass-through:** the box ships the recorded bitstream untouched and
    the client's hardware decodes it (Chrome/Edge with HW HEVC, Safari, mobile) — the most efficient path
    over a thin uplink; the no-HEVC browser tail (Firefox / old devices) gets a clear note instead of a
    black frame.
- **Does not provide (the managed tier, kept private):** the **rendezvous** itself — the signaling
  broker + TURN credential minting. That is the `heldar` Cloudflare **Worker + Durable Object**
  (`apps/edge/`, NOT in the open repo). The open kernel only *dials* it; it never sees media (DTLS-SRTP
  rides TURN). Removed entirely: the old kernel-managed WireGuard interface (the WebRTC path supersedes it).
- **Self-hosted reverse tunnel (frp/rathole + coturn on your VPS):** best privacy (you own every
  hop) and browser-zero-install, but the heaviest ops (a VPS + three daemons + certs) and not free.
  Use WebRTC+coturn (or TLS passthrough) so the VPS only ever sees ciphertext.
