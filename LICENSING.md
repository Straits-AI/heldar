# Licensing

Heldar is **open-core**. The kernel and the generic reference apps are **Apache-2.0** and are
developed in this public `heldar` repo — the source of truth for the open platform. Vertical /
client-specific products (and the hosted relay) are **proprietary**, live in their own private
repositories, and depend on the crates published from here through the documented seams. Each
vertical has its own repo and lifecycle; none of them is required to build or run the open
platform.

| Component | Crate / path | License |
|---|---|---|
| **Kernel** — media/DVR, perception ingest + sampler, zone engine, auth/RBAC, observability, retention, remote-access overlay awareness, worker SDK contract | `crates/heldar-kernel` | **Apache-2.0** |
| **Access Control** — generic ANPR authorization, vehicle/visitor/watchlist registry, guard workflow, entry/exception/audit reports | `crates/heldar-entry` | **Apache-2.0** |
| **Movement intelligence** — generic cross-camera ReID candidates, trails, red-zone breach engine | `crates/heldar-movement` | **Apache-2.0** |
| **Semantic search** — generic deterministic query layer + LLM-as-planner + proof ladder | `crates/heldar-search` | **Apache-2.0** |
| **Reference composing server** — links the kernel + open apps (proprietary verticals only via the `verticals` Cargo feature, off in the open build) | `crates/heldar-server` | **Apache-2.0** |
| **Reference AI worker** — YOLO/ByteTrack reference implementation of the open worker contract | `apps/ai` | **Apache-2.0** (model weights download separately under their own licenses, e.g. Ultralytics AGPL) |
| **BakerySense** — retail behaviour analytics (a vertical) | `crates/heldar-bakery` | **Proprietary** |
| **Campus** — school products (students/guardians, pickup/dismissal, parental-app integration) | `crates/heldar-campus-*` *(future)* | **Proprietary** |

## The boundary

The **open kernel** is the domain-agnostic platform anyone can self-host and build on. The **open
generic apps** are complete, deployable reference applications (access control, movement, search) —
they make the kernel immediately useful and carry no client-specific logic. **Remote access** is
browser-based and WebRTC-first (NAT traversal via signaling + TURN, MediaMTX/WHEP for video; see
`docs/REMOTE-ACCESS.md`); the open kernel also carries **overlay awareness** (it observes an external
Tailscale/NetBird/WireGuard daemon and reports reachability), the optional path for self-hosters.

**Vertical/client products** (BakerySense; the future Campus school suite with its students/guardians
model and parental-app integration) are proprietary crates that depend on the open generic crates and
layer their specifics on top. They plug in only through the kernel's public seams — the
`DetectionConsumer` trait, the HTTP/worker contract, `AppState`, the shared pool, and the auth
primitive. A deployment is **composed** from the open kernel + open apps + whichever proprietary app
crates that client needs (single-tenant per deployment).

See `ARCHITECTURE.md` for the seams. This repository **is** the source of truth for everything
Apache-2.0 above (this file is the licensing statement of record); the open crates are published to
crates.io from here. Proprietary products live in their own private repositories and consume the
published crates through the documented seams — they are never merged into this tree.
