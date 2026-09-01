# ADR 0005 — Task-lease-bound AI ingest with server-authored provenance

**Status:** ACCEPTED — implemented 2026-08-09 (kernel migration `0012`, staged rollout).
**Date:** 2026-08-09
**Context:** the AI ingest API trusted its own request body. Anything holding an integration key could
name any camera, any task type, any frame id — and could assert the one attribute the barrier treats
as authoritative.

## The problem

Three defects compounded into "an integration key can open a gate".

1. **Forgeable provenance.** `heldar-entry` weights a plate read carrying
   `attributes.source = "camera_native"` at the whole configured vote threshold: the camera's on-board
   engine has already consolidated many frames, so one read is enough. But `source` arrived *inside
   client-supplied detection attributes*. A single POST with that string was therefore a barrier open.
2. **Unbound ingest.** `POST /api/v1/ai/events` checked only `principal.can_ingest()` and that the
   camera row existed. It never checked that a matching `ai_task` existed, was enabled, belonged to
   the caller, or that the caller could touch that camera at all.
3. **A client-named idempotency key.** `frame_id` was constructed by the worker. Since the outbox is
   first-writer-wins on `(camera_id, frame_id)`, pre-claiming the id a real worker was about to use
   turned its genuine detection into a silent no-op — a *suppression* primitive, not just a forgery
   one.

None of these needed a compromised box. An integration key — the credential every AI worker and
third-party sidecar holds — was sufficient.

## Decision

**Provenance becomes a parameter of ingest, not a field of the payload**, and **ingest becomes bound
to a lease the kernel issued**.

- `perception_ingest::ingest_batch(st, body, prov)` takes a `Provenance` from its *caller*. Before the
  INSERT it rewrites every detection's `attributes`, stripping any client `source`/`_prov` and writing
  its own. `Provenance::Kernel { producer }` names a **closed enum** (`KernelProducer::NativeAnpr`)
  that only in-process kernel code can construct; the HTTP handler can construct only
  `Provenance::Worker`. `source = "camera_native"` is therefore not "rejected" at the API — it is
  **inexpressible**: there is no request that yields it.
- The rewrite lives in the ingest service, not the HTTP handler, because `services/fanout.rs` replays
  crashed batches by rebuilding them from the persisted `detections` rows. Rewriting in the handler
  would leave the replay path carrying whatever the client said.
- **Per-TASK leases** (`ai_task_leases`, one row per task, renewed ~once a minute) plus a **stateless
  per-frame HMAC ticket**. The ticket is issued with the JPEG when a lease-holder pulls a frame with
  `?task=`, and names `(api_key_id, camera_id, task_id, captured_ms, exp)`. At ingest the kernel
  *derives* `camera_id`, `task_type` and `frame_id` from the ticket; body values are cross-checked
  (`409`), never trusted.
- Two gate hardenings that provenance alone does not cover: **one vote per frame** (a batch cannot
  reach the threshold by repeating itself), and **commit-on-prune never actuates** (a below-threshold
  track still gets its audit row, marked `review`, but does not open the boom ~30 s later).

### Why per-task leases and not per-frame

Per-frame leases were rejected on write pressure: the box's SQLite has a single writer that the
recorder depends on. A lease is coarse (one write per task per ~60 s) and cacheable; the per-frame
layer is an HMAC that costs **zero writes**. Lease expiry is a predicate evaluated at claim time
rather than a reaper task, for the same reason — no new background writer.

### Why staged, and why only one thing is staged

Enforcement ships behind `HELDAR_INGEST_PROVENANCE = off | warn | enforce`, defaulting to `warn` and
promoted to `enforce` by `HELDAR_DEPLOYMENT_MODE=production*`. Under `warn` a ticketless batch is
accepted exactly as before, with one log line and one `ingest_unleased` event per credential per
hour — so an operator gets the list of clients that would break *before* flipping the switch.

Only the **ticket requirement** is staged. The attribute rewrite, the reserved-event denylist and the
severity clamp are unconditional in every tier including auth-off, because no legitimate client ever
depended on asserting any of them. Staging those too would have meant shipping the vulnerability
behind a flag.

## Consequences

- **Auth-disabled boxes (the LAN default) are unaffected.** `Principal::system_admin()` holds every
  capability, and the synthetic principal leases, gets tickets and ingests like any other.
- **Existing deployed keys are not bricked.** A ticketless client keeps working under the default
  tier, including its own `frame_id` — dropping that would silently disable outbox dedup and let an
  at-least-once redelivery add a second ANPR vote for a frame seen once.
- **`GET /api/v1/ai/tasks` is unchanged**, so old workers and every existing validation script keep
  working. The reference worker prefers `/ai/leases` and falls back on `404`, so a new worker also
  runs against an old kernel.
- **Recording gains no failure mode.** Every lease read is fallible and every caller degrades to "no
  lease" → "no ticket" → (under `warn`) today's behaviour. Ticket issuance never fails a frame pull,
  and no background service on the recorder path touches any of this.
- **Tickets do not survive a restart** (the signing key is random per boot). Harmless: the worker
  re-pulls a frame, and therefore a fresh ticket, every cycle.
- **Forensics are queryable.** `detections.provenance`, `outbox.provenance` and `outbox.produced_by`
  answer "which credential produced this?" in SQL; `gate_opened` carries the provenance of the reads
  that voted it open, which is the first time a barrier opening is attributable to a credential.

## Alternatives considered

- **Stamp-only (no leases).** Rewrite `attributes.source` server-side and stop there. It closes the
  forgery but leaves defects 2 and 3: a key could still post at any camera under any task type, and
  still pre-claim frame ids. Rejected as half a fix.
- **Existence check only.** Require that a matching enabled `ai_task` exists. Cheaper, but it
  authorizes by *coincidence* — any key could ingest for any camera that happened to have a task —
  and does nothing about the client-named `frame_id`.
- **Per-frame lease rows.** Strongest binding, unacceptable write amplification against the
  recorder's writer. The HMAC ticket buys the same per-frame property for no writes.
- **A configured signing secret** for tickets. Rejected: a per-boot random key needs no key file, no
  rotation story and no new secret in the deployment surface, and the only cost is that tickets do not
  outlive a restart — which nothing needs.
