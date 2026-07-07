# Heldar Core — Access Control (Stage 4) Operator & Integrator Guide

This is the definitive guide to the **Access Control** app **as actually built** in
`crates/heldar-entry`: RBAC authentication, a registered-vehicle
/ visitor-pass / watchlist registry, an **ANPR temporal-voting engine** that turns
per-frame plate reads into one authoritative entry/exit event, a guard
confirm/reject workflow, and daily/exception/audit reports.

Implementation: `services/anpr.rs` (engine), `auth.rs` + `routes/auth.rs` (RBAC),
`routes/entry.rs` (registry + events + reports), `migrations/0005_entry.sql`
(schema), `config.rs` (knobs). The ANPR *worker* side (vehicle→plate→OCR) is the
`AnprAnalyzer` in `apps/ai/worker.py`, documented in
[`docs/AI-WORKERS.md`](AI-WORKERS.md) §12. The kernel architecture is in
[`ARCHITECTURE.md`](../ARCHITECTURE.md) §17.

Stage 4 builds **entirely on the Stage 2/3 contract** — the ANPR worker posts
detections to the **unchanged** `POST /api/v1/ai/events`; the kernel routes `anpr`
task results into the entry engine. No new ingest path, no new decode.

---

## 1. Overview

```
   gate camera (RTSP)
        │
        ▼
   media kernel — sampler decodes sub-stream @ budgeted fps → frames/<cam>/latest.jpg
        │                                                          ▲
        │ (Stage 2 frame pull)                                     │
        ▼                                                          │
   AI worker: AnprAnalyzer  (YOLO+ByteTrack vehicles → color → OCR plate, per frame)
        │ POST /api/v1/ai/events { task_type:"anpr", detections:[{track_id, attributes:{plate,…}}] }
        ▼
   routes/ai.rs::ingest ── task_type=="anpr" ──► AnprEngine.process()
        │
        ▼
   services/anpr.rs:  temporal plate voting (per camera|track)  ──► winning plate
        │                                                            │
        │  identity resolution (watchlist→vehicle→pass→vip→unmatched)│
        ▼                                                            ▼
   entry_events row (canonical event + evidence frame)        events log "entry_<status>"
        │                                                      (warning/critical → Stage 1 webhook)
        ▼
   guard workflow:  GET /entry-events  →  confirm / reject       reports: entry-log / exceptions / audit
```

The product stance is wired into the engine: **plate/pass is the
primary identity anchor**; vehicle attributes (type/color/make/model) are
**secondary verification and search metadata only** — an attribute mismatch raises an
*exception for guard review*, never an automatic rejection, and make/model is never a
hard access decision without local benchmarking.

---

## 2. The entry pipeline

### 2.1 Worker ANPR reads (per frame)

The `AnprAnalyzer` (worker side, [`docs/AI-WORKERS.md`](AI-WORKERS.md) §12) detects +
tracks vehicles with YOLOv8 + ByteTrack, estimates a coarse color, and — when an OCR
backend is installed — reads the plate from each vehicle crop. It emits **one
detection per vehicle box per frame**, carrying a stable `track_id` and an
`attributes` object. It **never fabricates a plate**: with no OCR backend it simply
omits the plate field and emits vehicle attributes only.

### 2.2 Core temporal voting (`services/anpr.rs`)

`AnprEngine.process(camera_id, site_id, detections)` consolidates the noisy per-frame
reads of one vehicle into **one** authoritative event. Like the zone engine, **all
timing is driven by server time** (`Utc::now()`), and state is held in memory keyed
per **`(camera, track)`** (when a detection has no `track_id`, the key falls back to
`plate:<normalized>` so repeated reads of the same plate still consolidate within the
window).

For each detection in the batch:

- **Normalize** the plate to its lookup key — `normalize_plate`: ASCII-alphanumerics
  only, uppercased (`"W-XY 88.88"` → `WXY8888`).
- **Vote** — increment the per-track vote count for that normalized plate and add its
  confidence to a running sum.
- **Observe attributes** — keep the **highest-confidence** observation for each of
  `vehicle_type`, `color`, `make`, `model`; latch `direction` (`inbound`/`outbound`)
  and `model_versions` from the attributes.

**Winning plate** for a track = the plate with the most votes, tie-broken by summed
confidence — but **plausible plates are preferred over implausible ones**, so a noisy
digits-only OCR misread can't mask a real plate; the overall vote leader is used only
when no candidate is plausible. A plate is *plausible* (`is_plausible_plate`) when it
is 3–10 chars **and** mixes at least one letter and one digit (Malaysian plate shape).

**Commit triggers** (a track commits its winning plate **once**):

1. **Vote threshold** — the winning plate has reached `HELDAR_ANPR_MIN_VOTES`
   reads (default 3). Voting is on the *plate*, not the raw detection count, so a
   single noisy read or a plateless track can't trip the gate.
2. **Commit-on-prune** — a track not seen for `STATE_TTL_SECS = 30` s is pruned;
   if it never reached the threshold **but did produce at least one plate read**, it
   commits on the way out (a vehicle that passed too quickly to accumulate votes is
   still logged). Tracks that **never** yielded any plate (pure background vehicle
   detections) are dropped silently, so the entry log is not flooded with "unmatched"
   events for every transient car in frame.

If the entry-event insert fails, the track's `committed` flag is cleared so a
still-live track retries on the next batch (no silent drop).

### 2.3 Identity resolution (precedence)

A committed plate is classified against the registry by `AnprEngine.resolve`, in this
**strict precedence order** (first match wins):

| # | Lookup | `auth_status` | Notes |
|---|---|---|---|
| 0 | **Unreadable** plate (empty / not plausible) | `unmatched` | `note: no_plate_read` or `plate_unreadable`; nothing to look up — emit for guard review |
| 1 | **Block watchlist** (`active`, `kind='block'`) | `blocked` | Security-critical; **fails closed** — a DB error here becomes an `exception` (`note: watchlist_lookup_failed`), never a silent allow |
| 2 | **Registered vehicle** (`active`) | `matched` / `exception` | Validity window + attribute check, below |
| 3 | **Visitor pass** currently in its validity window (`status IN active,checked_in`) | `matched` / `exception` | Auto-checks-in an `active` pass on an inbound match |
| 3b | A pass exists for the plate but is **outside** its window | `exception` | `note: pass_outside_validity_window` |
| 4 | **VIP watchlist** (`active`, `kind='vip'`) | `matched` | Informational allow — only reached when not registered/passed |
| 5 | **Alert watchlist** (`active`, `kind='alert'`) on an otherwise-unknown plate | `exception` | Flag-for-review, no block |
| — | none of the above | `unmatched` | unknown plate, not flagged |

**Block-watchlist precedence is absolute** — a blocked plate is `blocked` even if it
is also a registered vehicle. The block lookup is the only branch that **fails closed**.

**Registered-vehicle detail:**

- Outside the `valid_from … valid_until` window (when set) → `exception`
  (`outside_validity_window`).
- **Attribute check** — the engine compares **`color` and `vehicle_type` only**
  (make/model is assistive metadata, never a mismatch trigger). A
  mismatch is recorded **only when both sides are known and differ**
  (case-insensitive); any mismatch → `exception` carrying the `mismatches` list. For
  example: *registered White Myvi `ABC1234`, detected Black SUV
  `ABC1234` → exception for guard review.*
- A clean match that is **also alert-listed** is **downgraded** from `matched`/`auto`
  to `exception`/`pending`.

**Visitor-pass detail:** the currently-valid pass is selected in SQL
(`valid_from <= now <= valid_until`, newest `valid_until` first, so a future-dated
pass can't mask a presently-valid one). An `active` pass matched on an **inbound**
read is auto-flipped to `checked_in`.

### 2.4 Canonical entry event + evidence

On commit the engine writes one `entry_events` row (the canonical event, §6
below). `event_type` is `vehicle_exit` when `direction == "outbound"`, else
`vehicle_entry`. It captures an **evidence frame** by copying the camera's latest
sampled frame (preferring `latest_main.jpg`, falling back to `latest_sub.jpg`) to
`/media/snapshots/entryevt_<id>.jpg` — a cheap file copy, no decode, reusing the
Stage 2 sampler's always-current frame.

It also **mirrors** the event into the kernel `events` log as `entry_<auth_status>`
(e.g. `entry_blocked`, `entry_exception`) at the resolution's severity, so a
`warning`/`critical` entry event flows straight into the **Stage 1 alert
notifier/webhook** (`docs/OBSERVABILITY.md`) with no extra wiring.

### 2.5 Guard workflow

Every committed event carries a `workflow_status`:

| `workflow_status` | Meaning |
|---|---|
| `auto` | a clean automatic match (registered vehicle / valid pass / VIP) — no guard action needed |
| `pending` | needs guard review — every `blocked` / `exception` / `unmatched`, and any alert-downgraded match |
| `confirmed` | a guard confirmed it (`POST …/confirm`), or a manual visitor check-in/out |
| `rejected` | a guard rejected it (`POST …/reject`) |

A guard works the queue via `GET /api/v1/entry-events?workflow_status=pending`, then
`POST /api/v1/entry-events/{id}/confirm` or `…/reject` (optional `{ "note": "…" }`).
Resolving stamps `resolved_by` / `resolved_by_id` / `resolved_at` (+ `note`) into the
event's `workflow` JSON and writes an audit-log entry.

A guard **check-in/out** of a visitor pass (`POST /api/v1/passes/{id}/checkin|checkout`)
also writes a manual `visitor_checkin` / `visitor_checkout` entry event (direction
`inbound`/`outbound`, `auth_status: matched`, `workflow_status: confirmed`) into the
same canonical feed, so the daily log is complete whether entry was automatic (ANPR)
or manual (booth).

---

## 3. Authorization status reference

`auth_status` is a denormalized column (and `subject.authorization.status`) on every
entry event. Four values:

| `auth_status` | Set when | Default `workflow_status` | Default severity |
|---|---|---|---|
| `matched` | registered vehicle (clean), valid visitor pass, or VIP watchlist | `auto` | `info` |
| `exception` | attribute mismatch, outside validity window, alert listing, watchlist-lookup failure | `pending` | `warning` |
| `unmatched` | unknown plate, or unreadable/no plate | `pending` | `warning` |
| `blocked` | active block-watchlist hit | `pending` | `critical` (or the watchlist entry's `severity`) |

The `authorization` JSON additionally records the deciding `source`
(`registered_vehicle` / `visitor_pass` / `watchlist` / `system` / `none`) and any
`vehicle_id` / `pass_id` / `kind` / `reason` / `mismatches` / `note`.

---

## 4. RBAC model

Two principal kinds carry a **role**: interactive **users** (password login → opaque
bearer session) and machine **API keys** (worker ingest + external integration).
There is also a synthetic **system** principal used when auth is disabled (§5).

Five roles, mapped to five capabilities (`auth.rs`):

| Capability (method) | What it gates | admin | manager | guard | viewer | integration |
|---|---|:---:|:---:|:---:|:---:|:---:|
| `can_view` | read the entry surface (vehicles, passes, watchlist, entry-events, entry-log + exception reports) | ✅ | ✅ | ✅ | ✅ | ✅ |
| `can_operate_gate` | create passes, check-in/out, confirm/reject entry events | ✅ | ✅ | ✅ | ❌ | ❌ |
| `can_manage_registry` | register/edit/delete vehicles + watchlist, delete passes, reinstate revoked passes, **read the audit log** | ✅ | ✅ | ❌ | ❌ | ❌ |
| `can_ingest` | post perception/ANPR events into the entry pipeline (`POST /api/v1/ai/events`) | ✅ | ❌ | ❌ | ❌ | ✅ |
| `can_admin` | manage users + API keys | ✅ | ❌ | ❌ | ❌ | ❌ |

Notes that match the code:

- `can_view` is **true for every authenticated principal** — including `integration`
  and `viewer`. The split is between *reading*, *operating the gate*, *managing the
  registry*, *ingesting*, and *administering*.
- The **audit log** (`GET /api/v1/audit`) requires `manager+`, not just view — it can
  reveal operator activity.
- A **`revoked` pass is terminal**: a guard cannot resurrect it by editing status;
  reinstating requires `manager+` (`can_manage_registry`).
- **Last-admin protection**: the API refuses to demote/disable/delete the last active
  admin, and refuses self-deletion.

A handler asserts a capability with `principal.require(principal.can_…(), "action")`,
which returns **403** (`role 'guard' is not permitted to …`) when denied.

---

## 5. Authentication setup

### 5.1 Default: open LAN appliance (`HELDAR_AUTH_ENABLED=false`)

Auth is **off by default**. With `auth_enabled=false` the `Principal` extractor yields
a synthetic **system admin** for every request, so the entire Stage 0–4 API behaves
exactly as an unauthenticated single-tenant LAN appliance — existing tooling, the
worker, and the web UI keep working with no credentials. This matches the Stage 0
posture ("No auth on the API — local/LAN dev").

### 5.2 Enabling auth (`HELDAR_AUTH_ENABLED=true`)

When enabled, every Stage 4 entry/admin handler requires a valid bearer token
(session **or** API key). A request with no token → **401 `authentication required`**;
an invalid/expired token → **401 `invalid or expired credentials`**. Roles are then
enforced per §4.

### 5.3 Bootstrap admin via env

On first run with auth enabled and **no users yet**, `ensure_bootstrap` seeds one
admin from the environment:

```bash
HELDAR_AUTH_ENABLED=true
HELDAR_BOOTSTRAP_ADMIN_USER=admin
HELDAR_BOOTSTRAP_ADMIN_PASSWORD=change-me-now   # must be >= 8 chars
```

If the password is shorter than 8 chars, **no admin is created** (logged as an error).
If the vars are unset, the server logs a warning that login is impossible until a user
is seeded — set them and restart. Bootstrap is a no-op once any user exists.

### 5.4 Sessions (interactive users)

`POST /api/v1/auth/login` exchanges `{username, password}` for an opaque bearer token
prefixed **`vos_`** and its `expires_at`. The token is a random 256-bit value; only its
SHA-256 is stored (a DB leak exposes no usable credential). Passwords are **argon2id**
PHC hashes; login runs argon2 verification even for unknown/disabled users (against a
dummy hash) so response latency can't reveal whether an account exists. Session
lifetime is `HELDAR_SESSION_TTL_HOURS` (default 12). `POST /api/v1/auth/logout`
revokes the presented token; disabling a user revokes all their sessions.

Use it as a **Bearer** token:

```http
Authorization: Bearer vos_3f2a9c1b…
```

### 5.5 API keys (worker / integration)

`POST /api/v1/api-keys` (admin only) mints a key prefixed **`vok_`**; the full key is
returned **exactly once** (only its hash and a short `key_prefix` are stored). Give the
worker an `integration`-role key (the default role for new keys) so it can `can_ingest`
but cannot operate the gate or read the registry. Present it via either header:

```http
X-API-Key: vok_a1b2c3…
# — or —
Authorization: Bearer vok_a1b2c3…
```

`token_from_headers` accepts `Authorization: Bearer <t>` (or `bearer`) and falls back
to `X-API-Key`. A key whose stored role is unparseable, or that is inactive, is denied
(never failed-open to a capability-bearing default).

> When auth is enabled, configure the AI worker with the key. The worker only needs
> `can_ingest` (it just POSTs to `/ai/tasks` discovery + `/ai/events`); an
> `integration` key is the least-privilege fit.

---

## 6. Canonical entry-event JSON

The `entry_events` row serializes to the canonical event model. Denormalized columns
(`plate`, `auth_status`, `workflow_status`, `direction`, `timestamp`) back fast
queries/reports; the rich `subject` / `authorization` / `evidence` / `workflow` /
`audit` blocks are JSON. Example (ANPR-produced `vehicle_entry`):

```json
{
  "id": "evt_8c1f2a9c1b4d4f6a8b0c1d2e3f405162",
  "site_id": "campus_01",
  "camera_id": "gate_a_01",
  "event_type": "vehicle_entry",
  "timestamp": "2026-06-13T08:15:31Z",
  "direction": "inbound",
  "plate": "ABC1234",
  "plate_confidence": 0.92,
  "subject": {
    "type": "vehicle",
    "plate": "ABC 1234",
    "plate_confidence": 0.92,
    "plate_valid": true,
    "vehicle_type": "car",
    "color": "white",
    "make_model": null
  },
  "authorization": {
    "status": "matched",
    "source": "visitor_pass",
    "pass_id": "pass_1023",
    "alert": false
  },
  "auth_status": "matched",
  "evidence": { "snapshot_path": "/media/snapshots/entryevt_evt_8c1f….jpg" },
  "workflow_status": "auto",
  "workflow": { "status": "auto" },
  "audit": {
    "created_by": "system",
    "model_versions": { "anpr": "anpr_v0.1_paddleocr", "vehicle_attr": "heuristic_v0.1", "detector": "yolov8n.pt" }
  },
  "track_id": "t17",
  "created_at": "2026-06-13T08:15:31Z"
}
```

Mapping to the canonical event model and honest deltas as built:

| Canonical field | As built |
|---|---|
| `event_id` | `id` (`evt_<uuid-simple>`) |
| `tenant_id` | not on the event (site-scoped today; `tenants` table exists for later) |
| `site_id`, `camera_id`, `event_type`, `timestamp` | columns; `event_type ∈ vehicle_entry, vehicle_exit, visitor_checkin, visitor_checkout` |
| `subject.{plate, plate_confidence, vehicle_type, color, make_model}` | present; adds `plate_valid` (plausibility). `plate` is the raw read; the denormalized top-level `plate` is the **normalized** key. `make_model` is composed from make+model when present (the reference worker emits type+color, not make/model — so usually `null`) |
| `location.{zone_id, direction}` | `direction` is a top-level column; `zone_id` is not attached to entry events |
| `authorization.{status, source, pass_id, vehicle_id, …}` | present (+ `kind`/`reason`/`mismatches`/`alert`/`note`) |
| `evidence.{snapshot_id, clip_id, recording_segment_ids}` | only `snapshot_path` today (the copied frame); clip/segment refs deferred |
| `workflow.{status, assigned_to, resolved_by, resolved_at, note}` | `status` always; `resolved_by`/`resolved_by_id`/`resolved_at`/`note` on guard resolve |
| `audit.{created_by, model_versions}` | present; `model_versions` is whatever the worker stamped |

---

## 7. Reports

All three are `GET` and require `can_view` (audit requires `manager+`). Time window:
either `date=YYYY-MM-DD` (a UTC day, the default = today) **or** explicit
`from`/`to` (RFC3339); `from`/`to` override `date`.

### 7.1 Daily entry log — `GET /api/v1/reports/entry-log`

Every entry event in the window plus an `auth_status` histogram.
`?date=` | `?from=&to=` | `?limit=` (default 1000, ≤10000).

```json
{
  "from": "2026-06-13T00:00:00Z",
  "to":   "2026-06-14T00:00:00Z",
  "total": 412,
  "by_auth_status": { "matched": 380, "exception": 18, "unmatched": 12, "blocked": 2 },
  "events": [ /* EntryEvent[] newest-first */ ]
}
```

### 7.2 Exception report — `GET /api/v1/reports/exceptions`

Everything that is **not** a clean automatic match:
`auth_status IN ('blocked','exception','unmatched') OR workflow_status='rejected'`.
Same window/limit params. Returns `{ from, to, total, events }`. This is the
plate/vehicle-mismatch report (mismatches surface as `exception`s with a
`mismatches` list in `authorization`).

### 7.3 Audit report — `GET /api/v1/audit` (manager+)

The immutable operator+system action log. Filters: `from`, `to`, `actor`, `action`,
`limit` (default 200, ≤5000), newest-first. Every registry mutation, pass operation,
user/key change, login, and entry confirm/reject appends a row
(`actor`, `actor_name`, `role`, `action`, `target_type`, `target_id`, `detail`).

---

## 8. Retention

`HELDAR_ENTRY_RETENTION_DAYS` (default **365**) governs the Stage 4 sweep inside the
Stage 0/1 retention loop. Older than the cutoff, it prunes: **entry events** (and their
evidence JPEGs from `/media/snapshots`), **audit-log** rows, and the **mirrored
`events`-log** rows; **expired sessions** are pruned every sweep regardless of the TTL.
Recording segments keep their own per-camera age policy + size/disk caps + evidence
lock (Stage 0/1) — Stage 4 retention only touches the entry domain.

---

## 9. Stage 4 HTTP API reference

All paths are under `/api/v1`. "Role required" is the minimum capability; reads are
open to any authenticated principal. When `HELDAR_AUTH_ENABLED=false` every caller
is the synthetic system admin (all rows below are satisfied).

### Authentication & administration (`routes/auth.rs`)

| Method | Path | Role required | Purpose |
|---|---|---|---|
| POST | `/auth/login` | none | Exchange `{username,password}` → `{token (vos_…), expires_at, user}` |
| POST | `/auth/logout` | (bearer token) | Revoke the presented session → `204` |
| GET | `/auth/me` | any authenticated | Report the caller `{id,name,role,kind}` |
| GET | `/users` | admin | List users (`UserView[]`) |
| POST | `/users` | admin | Create user (password ≥8) → `201` |
| PATCH | `/users/{id}` | admin | Update role/password/display/active (last-admin guard) |
| DELETE | `/users/{id}` | admin | Delete (no self-delete; last-admin guard) → `204` |
| GET | `/api-keys` | admin | List API keys (hashes never returned) |
| POST | `/api-keys` | admin | Mint a key → `201` `{…,key}` (**shown once**) |
| DELETE | `/api-keys/{id}` | admin | Revoke a key → `204` |

### Registry — vehicles (`routes/entry.rs`)

| Method | Path | Role required | Purpose |
|---|---|---|---|
| GET | `/vehicles` | view | List/search (`plate`, `owner_type`, `q`, `limit≤2000`) |
| POST | `/vehicles` | manage_registry | Register a vehicle → `201` |
| GET | `/vehicles/{id}` | view | Read one |
| PATCH | `/vehicles/{id}` | manage_registry | Partial update |
| DELETE | `/vehicles/{id}` | manage_registry | Delete → `204` |

`owner_type ∈ student\|staff\|resident\|contractor\|visitor`. `plate` is required and
normalized to the unique `plate_norm` key.

### Registry — visitor passes (`routes/entry.rs`)

| Method | Path | Role required | Purpose |
|---|---|---|---|
| GET | `/passes` | view | List/search (`status`, `q`, `limit`) |
| POST | `/passes` | operate_gate | Create a pass (auto `code` `V-XXXXXX`, default 24 h window) → `201` |
| GET | `/passes/{id}` | view | Read one |
| PATCH | `/passes/{id}` | operate_gate | Update (reinstating a `revoked` pass needs manage_registry) |
| DELETE | `/passes/{id}` | manage_registry | Delete → `204` |
| POST | `/passes/{id}/checkin` | operate_gate | Mark `checked_in` + write `visitor_checkin` entry event |
| POST | `/passes/{id}/checkout` | operate_gate | Mark `checked_out` + write `visitor_checkout` entry event |

`status ∈ active\|checked_in\|checked_out\|expired\|revoked`.

### Registry — watchlist (`routes/entry.rs`)

| Method | Path | Role required | Purpose |
|---|---|---|---|
| GET | `/watchlist` | view | List all entries (newest-first) |
| POST | `/watchlist` | manage_registry | Add a plate → `201` |
| PATCH | `/watchlist/{id}` | manage_registry | Update kind/reason/severity/active |
| DELETE | `/watchlist/{id}` | manage_registry | Delete → `204` |

`kind ∈ block\|vip\|alert`; `severity ∈ info\|warning\|critical`.

### Entry events + guard workflow (`routes/entry.rs`)

| Method | Path | Role required | Purpose |
|---|---|---|---|
| GET | `/entry-events` | view | Query (`from`,`to`,`plate`,`auth_status`,`workflow_status`,`event_type`,`limit≤5000`), newest-first |
| GET | `/entry-events/{id}` | view | Read one canonical event |
| POST | `/entry-events/{id}/confirm` | operate_gate | Guard confirm (optional `{note}`) → `workflow_status: confirmed` |
| POST | `/entry-events/{id}/reject` | operate_gate | Guard reject (optional `{note}`) → `workflow_status: rejected` |

### Reports + audit (`routes/entry.rs`)

| Method | Path | Role required | Purpose |
|---|---|---|---|
| GET | `/reports/entry-log` | view | Daily log + `by_auth_status` counts (`date` or `from`/`to`) |
| GET | `/reports/exceptions` | view | Blocked/exception/unmatched/rejected in the window |
| GET | `/audit` | manage_registry | Immutable action log (`from`,`to`,`actor`,`action`,`limit≤5000`) |

### Ingest (the worker's path into the engine — `routes/ai.rs`, Stage 2)

| Method | Path | Role required | Purpose |
|---|---|---|---|
| POST | `/ai/events` | ingest | Post detections; `task_type:"anpr"` feeds `AnprEngine.process()` |

---

## 10. Honest scope

- **Direction is a per-camera config hint**, not a calibrated entry/exit *line*. The
  worker's `direction: inbound\|outbound` task config sets the event direction; there
  is no homography/line-crossing primitive yet (gate cameras are usually
  single-direction). Deferred.
- **OCR / make-model accuracy is not benchmarked.** The engineering — voting,
  resolution, workflow, schema, API — is production-grade and unit-tested, but plate
  OCR and vehicle attributes need evaluation on **local Malaysian gate footage**
  before any hard claim. The reference worker emits **type + color**
  (no make/model classifier), and the engine treats attributes as **review
  exceptions, never auto-rejections**.
- **Auth currently guards the Stage 4 (and ingest) surface.** Extending the
  `Principal` guard to the legacy Stage 0–3 routes (cameras, recordings, zones, …) is
  follow-up work; today those remain open even with auth enabled.

See also: [`ARCHITECTURE.md`](../ARCHITECTURE.md) §17 (Stage 4 implementation),
[`docs/AI-WORKERS.md`](AI-WORKERS.md) §12 (the ANPR analyzer), [`ROADMAP.md`](../ROADMAP.md)
Stage 4.
