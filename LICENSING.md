# Licensing

VisionOps is **open-core**:

| Component | Crate / path | License |
|---|---|---|
| **Kernel** (media/DVR, perception ingest + sampler, zone engine, auth, observability, retention, worker SDK contract) | `crates/visionops-kernel` | **Apache-2.0** (`crates/visionops-kernel/LICENSE`) |
| **Campus Entry** app (ANPR authorization, vehicle/visitor/watchlist registry, guard workflow, reports) | `crates/visionops-entry` | **Proprietary** |
| Composing server (links the kernel + bundled apps for a deployment) | `crates/visionops-server` | **Proprietary** |
| Reference AI worker | `apps/ai` | (worker SDK contract is open; the reference implementation ships with the project) |

The kernel is the domain-agnostic platform anyone can self-host and build on. Domain/client-specific
applications (Campus Entry, BakerySense, the school parental app, …) are separate, proprietary crates
that depend on the kernel and plug in only through its public seams (the `DetectionConsumer` trait, the
HTTP/worker contract, `AppState`, and the auth primitive). A deployment is **composed** from the kernel
plus whichever app crates that client needs.
