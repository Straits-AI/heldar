# Heldar Core — Access Control (Stage 4) Operator & Integrator Guide

This is the definitive guide to the **Access Control** app **as actually built** in
`crates/heldar-entry`: RBAC authentication, a registered-vehicle
/ visitor-pass / watchlist registry, an **ANPR temporal-voting engine** that turns
per-frame plate reads into one authoritative entry/exit event, a guard
confirm/reject workflow, and daily/exception/audit reports.

Implementation: `services/anpr.rs` (engine), `auth.rs` + `routes/auth.rs` (RBAC),
`routes/entry.rs` (registry + events + reports), `crates/heldar-entry/migrations/0001_init.sql`
(the entry schema, self-installed via `db::run_app_migrations`; the RBAC tables are
kernel-owned in `migrations/0001_init.sql`), `config.rs` (knobs). The ANPR *worker* side (vehicle→plate→OCR) is the
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

### 2.1b Camera-native ANPR reads (on-board recognition)

Dedicated ANPR barrier cameras can feed the pipeline **directly from their on-board
plate engine** instead of (or alongside) the worker: enable **On-board ANPR** on the
camera's Device panel (`native_anpr_enabled`). The kernel's `native_anpr` poller
turns each device read into the same `task_type = "anpr"` detection batch, tagged
`attributes.source = "camera_native"` — the engine below weights such a read as
**authoritative** (one read meets the vote threshold; the device already consolidated
its own frames). Details, cursoring, and idempotency: [`CAMERA-CONTROLS.md`](CAMERA-CONTROLS.md) §3.
When using camera-native on a lane, disable the camera's server-side `anpr` AI task
to avoid double sources.

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

## 4b. Camera scope (per-credential camera restriction)

A capability says *what* a credential may do. A **camera scope** says *which cameras*
it may do it to. An API key created with a camera list is confined to those cameras;
a key created without one is **fleet-wide**, which is the default and the only thing
an interactive user session can be.

Scope is enforced independently of role: it is **not** waived by `admin`. A scoped
admin key is still scoped. Auth disabled (§5.1) yields the synthetic system principal,
which is fleet-wide — scope changes nothing about the open LAN appliance.

### What the scope covers

Confining a credential to a camera means more than 403-ing that camera's routes, so
enforcement has four shapes. The first is the obvious one; the other three exist
because per-route checks cannot see the escapes:

| Shape | Applies to | Behaviour for a scoped credential |
|---|---|---|
| **Per-route check** | anything keyed by a camera id (`…/cameras/{id}/…`, `{camera_id}`) | **403** on a camera outside the scope |
| **Read confinement** | fleet-wide lists (camera list, health/status, events, search) | results **filtered** to the scope — never a complete inventory disclosure |
| **Write confinement** | payloads carrying camera ids (backup policies, archive exports) | submitted ids are **intersected** with the scope before use, and re-applied when the policy later runs |
| **Fleet-only refusal** | credential management, egress config, fleet cursors, `/metrics` | **403** outright — see below |

### Capabilities that cannot be scoped

`events:read` and `identity:read` read tables with **no camera column** — there is no
predicate to filter them by — so pairing them with a camera scope would be a
boundary that silently does not hold. The API **refuses to mint** that combination,
and a stored key carrying it is **denied at authentication** (it predates the refusal
or was inserted out of band; re-mint it). `admin` implies every capability, so an
`admin` grant cannot carry a camera scope either.

The practical consequence: a camera-scoped credential cannot reach `/api/v1/events`,
the entry identity registry, or anything else gated on those capabilities. That is by
design, not a gap — the route matrix reports such routes as UNREACHABLE rather than
counting them as covered.

### Surfaces that refuse a scoped credential outright

Some surfaces have no coherent scoped answer, so they are refused rather than
filtered:

- **Credential management** (create/update/delete users and API keys). A scoped key
  that can mint keys can mint an unscoped one — scope would be self-removable.
- **Egress configuration** (backup destinations, webhooks). These send footage and
  events *out*; repointing a destination exfiltrates other cameras' data without ever
  reading them through a scoped route.
- **The outbox cursor** (`GET /api/v1/outbox`). `seq` is a monotonic fleet cursor;
  filtering it hands back a sequence with holes that the client reads as delivered.
- **The entry identity registry** (vehicles, watchlist, visitor passes). These tables
  have no camera column, and the ANPR pipeline matches them **by plate alone** before
  it can auto-open a barrier — so a row written there acts on every camera on the box.
  The direct gate actuators are scoped; this is the indirect path into the same relay.
- **Box-level settings and the module registry** (`/api/v1/system/*` writes, database
  status, module detail/unregister, plugin-registry refresh, backup destinations).
  Nothing about them is per-camera, and several are applied **later** by loops that
  hold no principal.
- **`GET /metrics`.** The exposition carries `heldar_camera_up{camera=…}` and friends
  for the whole fleet. A filtered scrape reads to Prometheus as cameras that ceased to
  exist, writing staleness gaps indistinguishable from real outages into the fleet's
  history. Scrape with a fleet-wide key.

### The audit log (`GET /api/v1/audit`)

Read confinement, but it could not be expressed the usual way. The owning camera is
routinely recorded in the free-form `detail` JSON under a *non-camera* `target_type` —
zones, AI tasks, record and snapshot schedules and recording gaps all do this — so a
predicate over `target_id` masked gate rows and let every one of those through. One
`?limit=5000` returned the fleet roster plus which cameras carry zones, AI tasks and
schedules. `detail` is `Json<Value>` with no schema and new call sites add keys freely:
it cannot be a scope boundary.

So camera identity was promoted into an indexed `audit_log.subject_camera_id` column
(kernel migration 0014), derived in `auth::audit` — the single writer — and backfilled
for rows already on the box. A scoped credential sees a row iff its subject is non-NULL
and in scope.

This is **fail-closed**: NULL means fleet-level or about no camera at all, and those
rows are hidden rather than shown. Multi-camera acts (an archive export, an API-key
mint, the `'*'` bulk device-config write) resolve to NULL deliberately — attributing
one to a single lane would both mislabel it and hand that lane's holder the other
camera ids sitting in the same `detail`. Audit is a manager+ surface where a hidden row
costs an accountability question and an extra row costs the roster, so hiding is the
conservative direction. Unscoped credentials — every human role, and every key minted
without a camera list — read the whole log unchanged.

### The recorded-media plane (`/media/*`)

`/media/*` serves the same footage the API gates, so it carries the same scope
(`services/media_scope.rs`). Two subtrees name their camera in the path
(`recordings/<camera_id>/…`, scheduled `snapshots/<camera_id>/…`) and are scoped by
string alone. The rest — exported clips, playback sessions, evidence frames, archives,
signed evidence bundles — are **flat**: their filenames carry no camera, so producers register each artifact
in the `media_artifacts` sidecar (migration 0013) and the guard resolves ownership
from it. Migration 0013 also carried existing zone and embedding evidence across;
**entry** migration 0004 does the same for gate evidence, which lives in the app crate
and was missed the first time — without it an upgraded box 403s a scoped credential on
its own pre-upgrade gate frame while the byte-identical zone frame beside it serves.

This fails closed in both directions: an artifact whose producer never registered it
is `Unattributed`, which is a 403 for a scoped credential (and unchanged for everyone
else), and a `/media/*` prefix the module does not recognise is refused for **every**
credential rather than served ungated.

Attribution rows are swept by the retention loop once their file leaves the disk — by
**existence**, not by age, because the kinds do not share a retention horizon and a
row dropped early would 403 a scoped credential on its own live evidence.

### How long a scope decision lasts

Scope is checked when a request arrives. Anything that reads that decision *later* — after the
response, or on a second request — needs a stated lifetime, because a credential can be re-scoped or
revoked in between (`PATCH /api/v1/api-keys/{id}`). Every surface that does so, and how long its
decision survives:

| Surface | When authorization is established | How long it lasts |
|---|---|---|
| Any `/api/v1` request | on the request | that request. `Principal` is resolved from the database every time; there is no principal cache |
| `/media/*` — clips, playback sessions, archives, evidence | on **every** fetch | until the next fetch. Re-scope → 403, revoke → 401, mid-scrub. A media URL is not a bearer capability |
| Backup / archive **job rows** | at trigger, confined into the job row | the job ships the confined list, never the policy's |
| Backup **transfer** (detached, off-box) | at trigger, **re-checked while running** | ≤ ~5 s after the credential is withdrawn (kernel migration 0015) |
| Archive export (`/api/v1/archive/export`) | on the request | the request — it runs inline and its `.zip` is re-guarded on every fetch |
| Site writes (`POST`/`PATCH`/`DELETE /api/v1/sites`) | on the request; fleet-only | the request. A site's timezone is what its cameras' schedules are read in, so changing it moves recording windows — the response reports how many cameras it moved, and a site with cameras cannot be deleted at all |
| Evidence bundle (`/api/v1/evidence/exports`) | on the request, against the camera the export actually reads — derived from `incident_id` where one is given, not the id supplied | the request; the bundle is re-guarded on every fetch like any other artifact. Once downloaded it is a file in someone's possession and no later scope change reaches it — which is the point of signing it |
| AI **leases** | on each acquire/renew (they are one call) | the lost camera stops being offered on the next renew; the stale row lapses on its TTL (≤ 300 s) and authorizes nothing on its own |
| AI **frame tickets** | at frame pull, bound to key id + camera | ≤ ticket TTL, and ingest re-checks `require_camera` against the live principal anyway |
| **Live view** token (MediaMTX) | at `/liveview`, **re-checked on every read** | HLS: seconds. WebRTC / RTSP readers: **immediately** when the change is made through the API, else within `HELDAR_LIVE_REAPER_INTERVAL_S` (default 15 s) |

**The detached backup transfer.** `services/backup::spawn_job` answers 202 and keeps copying for up
to `HELDAR_BACKUP_JOB_TIMEOUT_S` (default an hour), and for `sftp`/`ftp`/`s3` destinations the bytes
leave the box entirely, where no later guard of ours can reach them. Revoking the key is the operator
saying *this credential is compromised*; it used to do nothing to footage already in flight. The job
row now records **who ordered it** (`created_by`, `created_by_kind`) and the transfer re-asks before
the first byte and every few seconds after, aborting with the reason on the job. Withdrawal means
revoked, deactivated, deleted, expired — or re-scoped off any camera the job covers. Jobs with no
creator (the **scheduler**, which holds no principal, and pre-upgrade rows) and jobs created with auth
disabled are never withdrawn; a mechanism about revoking credentials must not touch deployments that
have none.

**Live view: the token names its subject.** The browser streams direct from MediaMTX, which has no
session to present, so `/internal/mediamtx-auth` authorizes on a signed, path-scoped URL token. That
token used to name no credential and the callback looked none up, so a revoked or re-scoped key kept
streaming to the TTL — measured at the time as `200 OK` on a replayed token. A `v2` token now carries
its **subject** inside the signed payload (api key, user, or site) and the callback re-resolves it on
every read: revoked, deactivated, deleted, expired, or re-scoped off that camera all stop the stream.

A transport is re-authorized as often as it **re-presents** the token, and HLS does so per segment.
WebRTC never re-presents — it negotiates once and then flows over the peer connection — and RTSP
readers are the same, so for those the bound token alone changed nothing.

Those are now ended from the other side. MediaMTX reports a session id on every auth callback and
exposes `POST /v3/{webrtcsessions,rtspsessions}/kick/{id}`, so the callback records who opened each
read (`live_sessions`, migration 0016) and withdrawal actually cuts the session: **immediately** when
the revocation, deactivation, deletion or re-scope is made through the API, and otherwise within
`HELDAR_LIVE_REAPER_INTERVAL_S` (default 15 s, `0` disables). The periodic sweep is the backstop for
what no API call announces — a key reaching `expires_at`, a row edited out of band, a kick that failed
and needs retrying.

Withdrawal is re-asked **per session**, so narrowing a scope cuts the cameras it lost and leaves the
ones it kept. Never cut: camera **publishes** (only the read arm is recorded, so the recorder is out of
reach by construction), sessions this box cannot attribute (they predate the table or survived a
database loss — kicking on ignorance would make a restart look like an outage), and `Site` subjects.

Two subjects are deliberately never withdrawn: **site** (the WebRTC rendezvous drives `ensure_live`
holding a site token, not a principal — and a remote viewer must not lose video because an unrelated
key was revoked) and, for **users**, the check reads `users.active` only, never sessions, so an
operator whose session idles out mid-watch keeps watching. A database read failure allows the read,
loudly: the recorder shares that SQLite, and a busy timeout must not black out the wall.
`HELDAR_LIVEVIEW_TOKEN_TTL_SECS` still bounds a token's life, and a kernel restart invalidates every
outstanding token (the signing key is per boot).
