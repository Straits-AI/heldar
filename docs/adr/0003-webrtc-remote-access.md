# ADR 0003 — Remote camera viewing over WebRTC; retire the mobile app + kernel WireGuard

**Status:** accepted (2026-06-21). Supersedes ADR 0002 (a React Native app embedding a
kernel-managed WireGuard tunnel), which is not published here.
**Context date:** 2026-06-21. Touches the kernel (`crates/heldar-kernel`), the dashboard
(`apps/web`), and the private fleet control-plane.

> The architecture that resulted is documented in [`ARCHITECTURE.md`](../../ARCHITECTURE.md) §21 and
> operationally in [`docs/REMOTE-ACCESS.md`](../REMOTE-ACCESS.md). This ADR records only why the
> earlier approach was abandoned and this one chosen.

## Context

ADR 0002 chose a React Native app that embeds a **kernel-managed WireGuard** tunnel, betting on the
box's **public IPv6** for reachability. In practice that bet fails the product bar — *"a customer can
view their cameras from any device, on any network, with the least friction."*

What we hit:

- **Plain WireGuard does no NAT traversal.** It is an encrypted point-to-point transport; for a
  handshake to start, the client must reach the box's endpoint. There is no hole-punching, no relay,
  no rendezvous — that coordination layer is what Tailscale/NetBird add *on top* of WireGuard. The
  kernel shipped *plain* managed WireGuard and relied on IPv6 for reachability.
- **IPv6 is not universal.** The box is frequently behind IPv4 **CGNAT** (only outbound works). Its
  public IPv6 is reachable only from clients that *also* have working IPv6 — which excludes IPv4-only
  networks and, notably, the **Android emulator** (its user-mode network is IPv4-only). We reproduced
  exactly this: a healthy box (WireGuard listening, cameras configured) that no client on hand could
  reach.
- **A native app is friction.** The app forced two Apple/Google **organization** developer accounts
  for the VPN entitlements (App Store Guideline 5.4; Play `VpnService` disclosure) before we could
  even ship — a hard gate.

The real requirement is **universal reachability (NAT traversal)** plus **no app**. Both are
properties of a *reachability layer*, not of the tunnel technology. The universal pattern — for any
transport — is **the box dials OUT to a public rendezvous** (outbound traverses CGNAT/firewalls), and
the client meets it there.

## Decision

Pivot remote access to **WebRTC**:

1. **Media over WebRTC/WHEP.** Live camera video is delivered via WebRTC, reusing the **MediaMTX
   WHEP** endpoint the box already serves (`HELDAR_MEDIAMTX_WEBRTC_BASE`, `:8889`; the kernel mints
   `LiveUrls.webrtc_url` in `services/mediamtx.rs`). NAT traversal is ICE/STUN with **TURN** relay
   fallback. TURN is operator-tunable: `HELDAR_WEBRTC_ICE_SERVERS` (a MediaMTX
   `webrtcICEServers2`-shaped JSON array) lets an operator bring their own STUN/TURN; unset, the box
   uses short-lived credentials minted by the rendezvous, else STUN/LAN-only. HLS stays as the
   always-works fallback transport.
2. **Browser-native, no app.** The viewer is the existing **`apps/web`** dashboard. No native app, no
   app-store VPN entitlements.
3. **Box dials OUT to a rendezvous.** An opt-in kernel service maintains an **outbound** connection to
   a public signaling service, modeled on the existing `services/fleet_register.rs` (dials out, parks
   when unconfigured, no inbound port). This is what makes CGNAT boxes reachable.
4. **Signaling + TURN live in the commercial tier.** They need an always-on public endpoint and
   per-customer operational ownership, so they sit with the fleet control-plane rather than in the
   kernel. A deployment with no rendezvous configured degrades to LAN/WHEP-only.
5. **Pairing model reused.** The single-use, short-TTL, manager-minted **pairing token** concept is
   repurposed to authorize a browser WebRTC session and bind it to a rendezvous channel + TURN lease —
   keeping the manager-minted / audited / short-TTL properties.

And remove the superseded pieces:

6. **Delete the React Native app** and its local WireGuard module.
7. **Remove the kernel-managed `wireguard` feature entirely** — `services/wireguard.rs`,
   `routes/remote_access.rs`, the `wireguard` cargo feature, `HELDAR_WG_*` config, the boot bring-up,
   and the `CAP_NET_ADMIN`/`setcap` deployment plumbing. This also deletes an **unauthenticated
   token-gated `/pair`** endpoint and the privileged `ip`/`wg` shell-out surface — a net security and
   ops simplification.

**Kept:** the separate, always-on **external-overlay *awareness*** (`services/remote_access.rs`,
`HELDAR_OVERLAY_*`, `OverlayStatus` via `/api/v1/system`). It only *observes* an externally-run overlay
(Tailscale/NetBird/wg) and is the supported **self-hoster full-L3 path**: a self-hoster who wants
transparent whole-box/LAN access runs their own overlay and the kernel reports its health. We no longer
ship a kernel-managed tunnel.

## Consequences

**Pros**
- Works on any device and any network (ICE + TURN) — the requirement plain WireGuard missed.
- No app, and no app-store organization-account gating. The whole dashboard becomes the remote surface.
- Reuses what we already run: MediaMTX/WHEP, the dial-out pattern, the token model.
- Smaller, safer kernel: no privileged networking, no `CAP_NET_ADMIN`, no unauthenticated `/pair`.

**Cons / costs**
- Adds a **cloud dependency** for the universal path (signaling + TURN). Mitigated: the LAN/WHEP path
  needs no cloud, and the self-hoster overlay path needs no Heldar cloud at all.
- WebRTC gives media plus a control channel, not transparent L3. The **full dashboard** remotely
  (playback, config, API) therefore needs a relayed-control channel, which WireGuard gave for free.
  Self-hosters who need true L3 use the overlay path.
- TURN relay consumes bandwidth for the symmetric-NAT tail; ICE prefers direct/STUN first.

## Alternatives considered

- **Keep WireGuard, add a coordination/relay layer (Headscale/Tailscale-style).** Keeps transparent L3
  and would solve reachability, but keeps the native-app requirement and a privileged data plane, and
  is more infrastructure to build and run than reusing WebRTC. Rejected as the *primary* path;
  self-hosters can still get L3 via the kept overlay-awareness.
- **Hybrid (WebRTC for most users, kernel-managed WireGuard for power users).** Rejected: the
  kernel-managed WireGuard carried real cost (privileged paths, the unauthenticated `/pair`,
  deployment capability plumbing) for a narrow audience already served by *external* overlays plus
  overlay-awareness.

## Delivery

Built in phases, each independently demoable, all since shipped: LAN WHEP live video (no cloud) →
outbound rendezvous + TURN for universal reach → the full dashboard over a relayed-HTTPS transport →
cutover (mobile app deleted, `wireguard` feature removed, docs rewritten).

The relayed control channel is **two-gate**: an outer per-user site capability gates *reachability*,
and the browser's real kernel session is replayed against the box's own loopback kernel, which runs its
**normal RBAC**. The kernel remains the sole auth authority — the relay never injects a principal — and
the box refuses to relay at all unless kernel auth is on and a real user exists. That model was
adversarially reviewed before implementation; see [`docs/PRODUCTION.md`](../PRODUCTION.md) for the
posture an internet-exposed deployment is expected to run.

## Open-core implications

The open kernel keeps a complete remote-access story; the universal-reach coordination is commercial:

- **Open (Apache-2.0, this repo):** kernel WHEP minting (`services/mediamtx.rs`); the browser
  WebRTC/WHEP viewer in `apps/web`; the box-side outbound rendezvous **client** (opt-in, parks when
  unconfigured — the same pattern as the already-open `services/fleet_register.rs`); external-overlay
  awareness.
- **Commercial:** the signaling/rendezvous service and TURN coordination. This mirrors the fleet split
  — the open kernel can *dial* a coordinator; running one is the proprietary tier.

The open dashboard and the `--no-default-features` build must degrade gracefully to LAN/WHEP-only when
no rendezvous is configured, since this repo ships no signaling server.

## Risks

- **TURN bandwidth/capacity** for the symmetric-CGNAT tail. Mitigate: ICE prefers direct/STUN; monitor
  the relay share.
- **ICE candidate gaps** — MediaMTX must advertise reachable candidates or WHEP silently fails behind
  NAT. Mitigate: configure `webrtcAdditionalHosts`/ICE servers plus STUN; keep the HLS fallback.
- **Cloud dependency** for the universal path. Mitigate: the LAN path and the self-hoster overlay path
  need no Heldar cloud.
- **Auth on the relayed control channel** touches a high-risk, historically low-test surface
  (`routes/auth.rs`). Mitigate: characterization tests first; keep tokens single-use, short-TTL,
  manager-minted and audited.
- **Privacy** — the rendezvous must never see plaintext media. Mitigate: media stays on WebRTC/TURN
  (DTLS-SRTP); the rendezvous brokers only SDP/ICE and the relayed control API.
- **Sunk cost** — retiring the mobile app (Android verified, iOS authored). Mitigate: its pairing/QR UX
  informed `apps/web`; nothing shipped to stores; the rationale is recorded here.
