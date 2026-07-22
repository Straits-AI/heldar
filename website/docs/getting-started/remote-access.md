---
id: remote-access
title: Remote Access
sidebar_label: Remote Access
sidebar_position: 3
---

# Remote Access

View a Heldar deployment from outside its LAN — including from behind **CGNAT** (the common
home/small-site case: a shared public IP, no inbound port to forward). Remote viewing is
**browser-based over WebRTC** and is an **open-kernel capability**: every deployment gets private
remote access with no client app and no inbound port.

## How it works

The box **dials out** to a rendezvous; a viewer just opens the dashboard in a browser. CGNAT always
allows outbound, so this works where port-forwarding and DDNS can't.

- **Live video** rides **MediaMTX / WHEP** and is **end-to-end encrypted (DTLS-SRTP)**. The rendezvous
  brokers only the SDP/ICE handshake and relayed control — never the video bytes. When a direct peer
  path can't be punched through symmetric CGNAT, **TURN** relays packets it cannot read.
- **The full dashboard** (live view, recorded playback, config, events) runs over the same brokered
  path, behind a **two-gate** auth model that keeps the kernel the sole RBAC authority:
  - **Outer gate** — a short-lived, per-user, site-scoped capability proves the browser may *reach*
    this box.
  - **Inner gate** — your **real kernel session** is forwarded verbatim and replayed against the box's
    own `127.0.0.1` kernel, which runs its normal auth + RBAC. The relay is a dumb, allowlisted pipe,
    never an auth bypass; the session token lives in an HttpOnly cookie that browser JS never holds.
- **Fail-safe:** the relay **refuses to run unless auth is enabled and a real user exists** — the
  open, auth-off API is never exposed remotely.

> **Why not port-forward / DDNS / a public reverse proxy?** Carrier CGNAT blocks all inbound and is
> usually *symmetric* NAT, which defeats plain STUN hole-punching. The only thing that reliably works
> is the box dialing out — which is exactly what the WebRTC path (and the overlay alternative below)
> does.

:::info Where the managed tier begins
The **box side** of this path — the dial-out client, the WHEP bridge, the relay — is open kernel
code, but it needs a **rendezvous** to dial: the hosted rendezvous is Heldar's managed tier. If you'd
rather not depend on a hosted service at all, the fully self-hosted alternative is the network
overlay described below.
:::

## Turn it on (the box side)

Remote access is opt-in. The box needs auth on and a rendezvous to dial:

```bash
HELDAR_AUTH_ENABLED=true              # required — the relay refuses to run without it
HELDAR_AUTH_COOKIE_SECURE=true        # the rendezvous terminates TLS
HELDAR_REMOTE_RENDEZVOUS_URL=https://<your-rendezvous>   # the box dials OUT to this
HELDAR_CP_TOKEN=<dial-out bearer>     # the per-box token your rendezvous issued for this HELDAR_SITE_ID (site-bound)
HELDAR_SITE_ID=<stable-id-for-this-box>
```

With these set, the kernel dials out, bridges browser WHEP offers to its own MediaMTX, and programs
MediaMTX's ICE servers so the box gathers a relay candidate for symmetric-NAT traversal. Viewers open
the dashboard the rendezvous serves for your site.

**TURN — use the managed rendezvous, or bring your own:**

- **Managed:** point `HELDAR_REMOTE_RENDEZVOUS_URL` at the hosted rendezvous; the kernel fetches
  short-lived TURN credentials from it and refreshes them automatically.
- **Your own:** set `HELDAR_WEBRTC_ICE_SERVERS` to a MediaMTX `webrtcICEServers2` JSON array pointing
  at any STUN/TURN you run (coturn, Cloudflare Realtime, …). The kernel programs it into MediaMTX.
- **Neither** → MediaMTX stays on its STUN baseline: LAN / non-symmetric-NAT only.

Recorded playback is **HEVC/H.265 pass-through** — the box ships the recorded bitstream untouched and
the client's hardware decodes it (the most efficient path over a thin uplink); the no-HEVC browser
tail gets a clear note rather than a black frame.

:::warning Harden before you expose it
The relay enforces auth, but an internet-facing box needs the full checklist — a `Secure` cookie, a
short session TTL + idle timeout, camera-credential encryption (`HELDAR_SECRET_KEY`), and the
rendezvous secrets (with an optional Cloudflare Turnstile login challenge). Work through the
[Production hardening guide](https://github.com/Straits-AI/heldar/blob/main/docs/PRODUCTION.md) first —
the kernel also **fails loud** and refuses to boot with auth off behind a rendezvous.
:::

## Alternative: a self-hosted network overlay

If you'd rather have full L3 reach to the whole host than the hosted rendezvous, run a WireGuard
overlay as an external daemon and point the kernel at its interface for status:

```bash
HELDAR_OVERLAY_ENABLED=true
HELDAR_OVERLAY_KIND=tailscale         # or netbird
HELDAR_OVERLAY_IFACE=tailscale0       # wt0 for netbird
```

- **Personal / dev → Tailscale** (free, near-zero ops; non-commercial only).
- **Shipped product → NetBird self-hosted** (one container per deployment, no per-seat cost, no
  third-party metadata).

The kernel only *observes* the overlay (it never manages WireGuard) and surfaces its health at
`GET /api/v1/system → remote_access`. Lock the overlay's ACL to the media/control ports
(`8889` / `8888` / `8000`), not the whole host.

## Learn more

- **Full reference + overlay recipes:**
  [`docs/REMOTE-ACCESS.md`](https://github.com/Straits-AI/heldar/blob/main/docs/REMOTE-ACCESS.md) — the
  CGNAT rationale, the P2P-first privacy model, and the Tailscale / NetBird recipes in full.
- **Hardening:**
  [Production hardening](https://github.com/Straits-AI/heldar/blob/main/docs/PRODUCTION.md).
- **Operate hub:** [Operate](../operate/index.md).
