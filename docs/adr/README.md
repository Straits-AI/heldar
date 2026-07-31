# Architecture Decision Records

An ADR records a decision that shaped the platform: the context that forced it, the options weighed,
what was chosen, and what it cost. They are written once and then left alone — a superseded ADR is
marked superseded rather than edited, so the reasoning stays readable years later.

These are **history, not documentation**. For how the system works today, read
[`ARCHITECTURE.md`](../../ARCHITECTURE.md); an ADR tells you *why* it is that way.

| ADR | Decision | Status |
|---|---|---|
| [0003](0003-webrtc-remote-access.md) | Remote camera viewing over WebRTC; retire the mobile app and kernel-managed WireGuard | Accepted (2026-06-21) |
| [0004](0004-store-abstraction-postgres.md) | Edge nodes stay on SQLite; scale horizontally rather than porting to Postgres | Accepted — no port (2026-07-16) |

Numbers are not contiguous here. Decisions that are wholly about the commercial tier — fleet
control-plane topology, repository and licensing strategy — are recorded in a private tracker, since
they concern infrastructure and business arrangements rather than this codebase. Where such a decision
constrains the open platform, that constraint is stated in the open ADRs and in `ARCHITECTURE.md`
rather than left implicit.
