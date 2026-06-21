# Licensing

Heldar is **open-core**. The kernel and the generic reference apps are **Apache-2.0** and live in
the public `heldar` repo; vertical/client-specific products are **proprietary** and live in the
private `heldar-proprietary` repo, depending on the open crates through published seams.

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

See `ARCHITECTURE.md` for the seams, and `docs/OPEN-CORE-SPLIT.md` for how the two repos are produced
and the open crates are published.
