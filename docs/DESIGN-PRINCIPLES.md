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

8. **Open-core discipline.** The public repo is *generated* from the private monorepo by a scrubbing
   step; proprietary code and names never reach it, and secrets are never committed (reference
   credentials by `file:line` + type only). If a change could leak proprietary material to the open
   tree, the generator must strip it and the leak-gate must catch it.

9. **Compose, don't couple.** Apps plug into the kernel through narrow, named seams — a
   `DetectionConsumer`, a `Router<AppState>` merge, a self-installed schema (`schema::init`). Adding a
   capability is a registration at the composition root, not an edit to the kernel's ingest or routing
   internals. The lean/open build must always compile without the proprietary and off-by-default
   features.

## How we build

10. **Nothing is "done" until the verification gate passes.** The full gate — `fmt`, `clippy
    -D warnings`, workspace `build` + `test`, the open (`--no-default-features`) and off-by-default
    feature builds, and the web build — is the definition of done. "It compiles for me" is not.

11. **Design before code; verify adversarially.** Non-trivial work goes brainstorm → spec → plan →
    implement, and correctness claims are *checked*, not asserted — by tests that would fail if the
    behavior regressed, and by independent review that tries to break the claim. A plausible argument
    is not evidence.
