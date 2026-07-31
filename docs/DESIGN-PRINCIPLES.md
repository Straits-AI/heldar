# Heldar design principles

The values that guide how Heldar is built. They are descriptive of the system as it exists, not
aspirational — each one is enforced somewhere in the code today. When a design decision is unclear,
these are the tie-breakers. New work should uphold them; work that contradicts one should say so
explicitly and why.

## Product & system

1. **Self-bounding by default.** Every store that can grow has a hard, self-enforced cap and
   least-valuable-first eviction — the appliance can never fill its own disk or unbounded-grow its own
   database. Recordings have a size cap plus a free-disk floor; the metadata DB has a size cap.
   Eviction sheds the cheapest data first: detections are disposable, while events, audit, and
   evidence-locked footage are protected.

2. **Boot is sacred — never block it.** The server binds its socket and answers `/healthz`
   immediately. Expensive one-time or maintenance work (e.g. the one-time `auto_vacuum` conversion)
   runs in the background *after* the socket is up. A slow disk or a multi-gigabyte database must never
   make the box look "down" to a health check or an operator.

3. **Maintenance is best-effort, idempotent, and self-healing.** Background upkeep — conversion,
   reclaim/vacuum, retention sweeps — is disk-gated, safe to re-run, and non-fatal: it logs and retries
   on the next pass rather than crashing the server, and it can never delete protected data or leave
   the store in a half-converted state.

4. **Operator settings live in the UI; infrastructure and security config lives in env.** Anything an
   operator tunes day-to-day — size caps, retention, a "reclaim now" action — is in the dashboard,
   admin-gated and audited. Security- and deploy-time config — auth toggle, secret key, CORS origins,
   session policy, bind address, remote-access and per-box tokens — is env-only and enforced at boot,
   never runtime-editable, so a compromised session can't weaken the box. Good UX and a hard security
   boundary are not in tension: the dividing line is "who owns this decision and how bad is it if it's
   changed by the wrong person."

5. **LAN-appliance defaults; production hardening is opt-in and fails loud.** Out of the box Heldar is
   a friendly LAN appliance (auth off, permissive CORS) so a first run just works. Production controls
   — auth, cookie-`Secure`, login lockout, secret-at-rest, TLS — are opt-in or auto-detected, and when
   the box is internet-exposed the boot guardrails refuse to start (or loudly warn) on unsafe config.
   Turning on remote access must never silently downgrade safety.

6. **Least privilege, always audited.** Privileged actions require the right role (viewer / manager /
   admin) and are written to the audit log — who did what, when. Read is broad; mutation is gated.

7. **Additive and reversible.** Migrations are append-only — never edit a shipped migration, add a new
   one. Deploys keep a rollback binary. A cleared runtime override reverts to the env default. Prefer
   changes that can be undone without a data migration.

8. **Open-core discipline.** This repository is the source of truth for the open platform, and it is
   developed in the open — the commit that lands here is the commit that ships. Proprietary code and
   client names therefore must never be written into it in the first place, and secrets are never
   committed (reference credentials by `file:line` + type only). Anything vertical- or
   client-specific belongs in its own private repository, consuming these crates through the
   published seams.

9. **Compose, don't couple.** Apps plug into the kernel through narrow, named seams — a
   `DetectionConsumer`, a `Router<AppState>` merge, a self-installed schema (`schema::init`). Adding a
   capability is a registration at the composition root, not an edit to the kernel's ingest or routing
   internals. The lean/open build must always compile without the proprietary and off-by-default
   features. **Cross-app reads go through a contract, not raw SQL:** when one app must read another app's
   table on the shared pool, the OWNING app publishes a stable `*_read` SQL view (e.g.
   `entry_events_read`, `breach_alerts_read`) exposing exactly the columns peers may depend on; consumers
   read the view, never the base table. A base-column rename is then a producer-local migration that
   redefines the view (aliasing the new column to the contract name), and the producer's
   `tests/read_contract.rs` fails if the contract breaks — so cross-app drift is caught in the owner's CI,
   not at runtime in a distant consumer. (A grep lint, `scripts/check-read-seam.sh`, forbids consumers
   from reading a peer's base table directly.)

## How we build

10. **Nothing is "done" until the verification gate passes.** The full gate — `fmt`, `clippy
    -D warnings`, workspace `build` + `test`, the open (`--no-default-features`) and off-by-default
    feature builds, and the web build — is the definition of done. "It compiles for me" is not.

11. **Design before code; verify adversarially.** Non-trivial work goes brainstorm → spec → plan →
    implement, and correctness claims are *checked*, not asserted — by tests that would fail if the
    behavior regressed, and by independent review that tries to break the claim. A plausible argument
    is not evidence.
