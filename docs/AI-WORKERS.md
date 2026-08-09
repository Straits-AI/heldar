# Heldar Core — AI Worker Integration Guide (Stages 2–4)

This is the integration guide for **AI workers** against the Heldar Core media
kernel. It documents the Stage 2 **frame sampler** and the **worker contract**
(§§1–10) plus the Stage 3 **detection + tracking analyzer and zone engine** (§11)
exactly as built in `crates/heldar-kernel` (`services/sampler.rs`, `services/zones.rs`,
`routes/ai.rs`, `routes/zones.rs`, `models.rs`, `config.rs`, and the AI + zones tables
in `migrations/0001_init.sql`).

Stage 2 delivers the **frame sampler and AI task scheduler**. Its success criterion is:

> *"AI consumes frames without breaking recording/live view."*

Stage 2 ships the **substream sampler, the global fps budget + backpressure, the
AI-task model, and the full pull-based worker contract.** **Stage 3 (§11) now ships
the real perception on top of that contract:** a **YOLO + ByteTrack** analyzer
behind the §8 `Analyzer` seam that posts *tracked* detections (`track_id` per
object), and a kernel-side **zone engine** that turns those tracked detections into
`enter` / `exit` / `dwell` **zone events** with evidence. Stage 3 added a detector
subclass + tracker in the worker and a `track_id`-aware consumer in the kernel —
**with no change to the §5 HTTP contract.**

---

## 1. Core principle: workers never own RTSP

```
   camera (RTSP)                          AI worker (Python / any HTTP client)
        │                                            ▲   │
        ▼                                            │   │ GET /api/v1/ai/tasks      (discover)
   media kernel (Rust)                               │   │ GET <frame_url>           (pull frame)
   ┌──────────────────────────┐                      │   │ POST /api/v1/ai/events    (post results)
   │ recorder  (-c copy, 24/7) │  never decoded ─────┘   │
   │ sampler   (decode @ fps)  │  frames/<cam>/latest.jpg │
   └──────────────┬────────────┘            ▲             ▼
                  │ ffmpeg -vf fps,scale     │       detections + events → SQLite
                  ▼                          │
          frames/<cam>/latest.jpg ───────────┘ (served by GET /api/v1/cameras/{id}/frame)
```

The kernel is the **only** thing that talks to cameras. The recorder keeps the
24/7 compressed-segment path **decode-free**; the sampler is the
*only* component that decodes, and it decodes the **sub-stream** at a budgeted
frame rate to a single JPEG per camera. Workers are pure HTTP clients: they
**discover** tasks, **pull** the latest frame on their own cadence, and **post**
detections back. A crashing, slow, or absent worker can never stall ingest or
recording — the sampler writes frames regardless of whether anyone reads them.

This is the capture/edge/AI split made concrete:

```
Cameras capture.  Edge processes (kernel decodes + samples).  AI consumes normalized frames.
```

---

## 2. The fps budget and backpressure

The host has finite decode capacity, so frame sampling is governed by a **single
global frame-per-second budget** shared across every AI-enabled camera. As you
enable AI on more cameras, each camera's sample rate **degrades** rather than the
host overloading. This is the Stage 2 realization of the backpressure
policy.

The budget is computed in `SamplerManager::rebalance` (`services/sampler.rs`):

```
active          = number of enabled cameras that have ≥1 enabled AI task
budget          = HELDAR_AI_MAX_TOTAL_FPS  (default 40, floored at 1.0)
per_camera_cap  = budget / active
effective_fps   = clamp( min(task_fps, per_camera_cap), MIN_FPS=0.5, … )
```

Key facts, grounded in the code:

- **One sampler process per camera, not per task.** A camera's `task_fps` and
  `width` are the **MAX** across all of that camera's *enabled* tasks
  (`SELECT MAX(t.fps), MAX(t.width) … GROUP BY c.id`). All tasks on a camera
  therefore share **one** ffmpeg and **one** `latest.jpg`. If a camera runs
  `detection @5fps/1280` and `anpr @10fps/1920`, the sampler decodes once at
  `10 fps / 1920px` and both workers pull the same frame.
- **Per-camera fps = `min(task_fps, budget/active)`.** With the default budget of
  40 fps: 4 AI cameras → up to 10 fps each; 8 → 5 fps each; 20 → 2 fps each.
  A camera never samples *faster* than it asked for, even if budget is spare.
- **`MIN_FPS = 0.5` floor.** Effective fps is never driven below 0.5 fps. With a
  very large camera count this floor can push the *summed* rate slightly above
  the configured budget — the floor protects each camera from starving to zero
  and wins over the strict budget.
- **Reconcile = rebalance.** Any AI-task create/update/delete (and boot) calls
  `sampler.reconcile()`, which stops **all** samplers, recomputes the active set
  + per-camera cap, and restarts them. It is serialized by an internal
  `rebalance_lock` so concurrent edits can't race into overlapping ffmpegs.
- **Master switch.** `HELDAR_AI_ENABLED=false` makes `rebalance` a no-op (no
  samplers run at all), independent of whether tasks exist.

### What the sampler actually runs

For each active camera it spawns (paraphrased from `services/sampler.rs`):

```
ffmpeg -nostdin -hide_banner -loglevel warning
       -rtsp_transport tcp -timeout 15000000
       -i <sub-stream URL, else record URL>
       -an -vf "fps=<effective_fps>,scale=<width>:-2" -q:v 5
       -f image2 -update 1 -y  <frames_dir>/<camera_id>/latest.jpg
```

- **Sub-stream first.** The source is `stream_url(cam, "sub")`, falling back to
  the record URL (`record_url(cam)`). The lighter sub-stream is preferred so the
  decode cost is low. (Note: the sampler currently always biases to the
  sub-stream; the per-task `stream_profile` field is stored, returned in
  discovery, and validated, but is **advisory** to the sampler today — see §10.)
- **`-update 1` → one file, overwritten in place.** There is no growing frame
  directory and no per-frame id; `latest.jpg` is the always-current frame
  (last-value). Workers pull it whenever they like and use the
  `x-frame-age-ms` header to judge staleness.
- **`scale=<width>:-2`** keeps aspect ratio (height auto, even).
- **Supervised with backoff.** On ffmpeg exit the camera goes `offline`, a
  `sampler_offline` warning event is logged (masked detail), and it retries with
  exponential backoff (doubling, capped at 30 s). On stop it is killed cleanly
  (`kill_on_drop`).

Sampler states (surfaced via `/api/v1/ai/samplers`): `connecting` → `sampling`,
or `offline` / `error` / `stopped`.

---

## 3. The AI task model

A row in `ai_tasks` (`migrations/0001_init.sql`, `models.rs::AiTask`) declares
*what perception to run on a camera*. Workers consume tasks; the kernel only uses
`fps`/`width`/`enabled` to drive the sampler.

| Field | Type | Notes |
|---|---|---|
| `id` | text PK | `ai_<uuid-simple>`, server-assigned |
| `camera_id` | text FK | → `cameras(id)` `ON DELETE CASCADE` |
| `task_type` | text | **free-form** — `detection` / `anpr` / `tracking` / … (the worker decides what it means) |
| `enabled` | bool | default `true`; only enabled tasks on enabled cameras sample or appear in discovery |
| `stream_profile` | text | `sub` \| `main` (default `sub`); validated on write, advisory to the sampler today |
| `fps` | real | requested sample rate, **clamped 0.1 … 30** on write (budget may reduce the effective rate) |
| `width` | int | target sample width px, **clamped 160 … 3840**; height keeps aspect |
| `config` | JSON | free-form blob: model params, class filter, zones, thresholds (default `{}`) |
| `created_at` / `updated_at` | RFC3339 | |

`fps`/`width` defaults when omitted on create come from
`HELDAR_DEFAULT_AI_FPS` (5) and `HELDAR_DEFAULT_AI_WIDTH` (1280).

A **detection** (`detections` table, `models.rs::Detection`) is one result a
worker posts back:

| Field | Type | Notes |
|---|---|---|
| `id` | text PK | `det_<uuid-simple>`, server-assigned |
| `camera_id`, `task_type` | text | echo of the ingest envelope |
| `timestamp` | RFC3339 | from the ingest envelope (or server `now()` if omitted) |
| `label` | text? | e.g. `person`, `car`, plate string |
| `confidence` | real? | 0…1 |
| `bbox` | JSON? | **`[x, y, w, h]` normalized 0…1** (top-left origin) — see §7 |
| `track_id` | text? | stable id across frames for tracking |
| `attributes` | JSON | free-form (color, zone, OCR text, …); default `{}` |
| `created_at` | RFC3339 | server-assigned |

---

## 4. AI task lifecycle

```
create ──► enabled=1 ──► sampler.reconcile() ──► rebalance ──► sampler starts/adjusts
  │                                                                     │
  ├─ PATCH enabled=false ──► reconcile ──► camera drops out of budget ──┘
  ├─ PATCH fps/width      ──► reconcile ──► camera re-sampled at new max
  └─ DELETE               ──► reconcile ──► sampler stops if no enabled tasks remain
```

Every mutation handler in `routes/ai.rs` calls `st.sampler.reconcile()` after
the DB write, so the running samplers always reflect the task table. Enabling the
first task on a camera starts a sampler; disabling/deleting the last enabled task
stops it (and frees its share of the budget for the others).

**Management endpoints** (operator/admin side; not part of the worker loop):

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/v1/cameras/{id}/ai-tasks` | list a camera's tasks (incl. disabled) |
| POST | `/api/v1/cameras/{id}/ai-tasks` | create a task → `201` + the task |
| PATCH | `/api/v1/ai-tasks/{task_id}` | partial update (any subset of fields) |
| DELETE | `/api/v1/ai-tasks/{task_id}` | delete → `204` (`404` if unknown) |

Create request:

```http
POST /api/v1/cameras/gate_a_01/ai-tasks
Content-Type: application/json

{
  "task_type": "detection",
  "stream_profile": "sub",
  "fps": 5,
  "width": 1280,
  "config": { "classes": ["person", "car"], "min_confidence": 0.4 },
  "enabled": true
}
```

`201 Created`:

```json
{
  "id": "ai_3f2a9c1b4d5e4f6a8b0c1d2e3f405162",
  "camera_id": "gate_a_01",
  "task_type": "detection",
  "enabled": true,
  "stream_profile": "sub",
  "fps": 5.0,
  "width": 1280,
  "config": { "classes": ["person", "car"], "min_confidence": 0.4 },
  "created_at": "2026-06-13T08:15:00Z",
  "updated_at": "2026-06-13T08:15:00Z"
}
```

Disable without deleting (frees budget, keeps config):

```http
PATCH /api/v1/ai-tasks/ai_3f2a9c1b4d5e4f6a8b0c1d2e3f405162
{ "enabled": false }
```

---

## 5. The worker contract

A worker needs only these four core endpoints (the semantic-embedding endpoints in §5.6–§5.8 are optional). All live under `/api/v1` and return
JSON (except the frame, which is `image/jpeg`).

> **Provenance, in one paragraph.** A batch posted over this API is *always*
> recorded as `attributes.source = "worker"`. The kernel **rewrites** `source` and
> `_prov` on every detection from the credential and lease the request arrived
> under, discarding whatever the body said. There is no request a client can make
> that produces `source = "camera_native"` — that value is reserved for the
> kernel's own camera-native ANPR poller, which is a closed, in-process producer
> (`Provenance::Kernel`). This matters because `heldar-entry` treats a
> camera-native plate read as authoritative for the barrier; see §5.0 and §12.4.

### 5.0 Leases and frame tickets (the provenance chain)

Three server-side facts bind a posted detection to a frame the worker actually held:

1. **A lease** (`POST /api/v1/ai/leases`, §5.1a) binds `(credential, worker_id)` to
   a set of tasks. One row per task, renewed about once a minute — never per frame.
2. **A frame ticket** is issued with the JPEG when the worker pulls a frame *for a
   task it holds a live lease on* (`?task=<ai_task_id>`, §5.2). It is a stateless
   HMAC naming the task and the frame's capture time, bound to the calling
   credential and the lease's camera. Zero writes; nothing to expire or sweep.
3. **Ingest** (§5.3) verifies the ticket and then **derives** `camera_id`,
   `task_type` and `frame_id` from it. Values still present in the body are
   cross-checked, not trusted: a disagreement is `409`.

Because `frame_id` becomes `"{task_id}:{captured_ms}"` — server-derived — a client
can no longer name a frame it never held. That closes a suppression trick: the
outbox is first-writer-wins on `(camera_id, frame_id)`, so pre-claiming the id the
real worker was about to use used to make its genuine detection a silent no-op.

**Enforcement is staged** via `HELDAR_INGEST_PROVENANCE`:

| Tier | Ticketless ingest | Notes |
| --- | --- | --- |
| `off` | accepted | no notice |
| `warn` | accepted | **default.** Logs once per credential per hour and raises one `ingest_unleased` event, so you get the list of clients that would break |
| `enforce` | `401 frame_ticket_required` | promoted automatically by `HELDAR_DEPLOYMENT_MODE=production*` |

The attribute rewrite, the reserved-event denylist and the severity clamp are
**unconditional in every tier**, including auth-off — no legitimate client ever
depended on asserting them. Only the *ticket requirement* is staged.

### 5.1 Discover — `GET /api/v1/ai/tasks`

Returns every **enabled** task on an **enabled** camera, each carrying the
`frame_url` to pull. This is the worker's whole work list.

```json
[
  {
    "id": "ai_3f2a9c1b4d5e4f6a8b0c1d2e3f405162",
    "camera_id": "gate_a_01",
    "task_type": "detection",
    "stream_profile": "sub",
    "fps": 5.0,
    "width": 1280,
    "config": { "classes": ["person", "car"], "min_confidence": 0.4 },
    "frame_url": "/api/v1/cameras/gate_a_01/frame"
  }
]
```

The worker should **re-discover periodically** (e.g. every few seconds) so it
picks up newly enabled/disabled tasks. Note `fps` here is the task's *requested*
rate; the *effective* sampled rate after budgeting is reported by
`/api/v1/ai/samplers` (§5.4).

**Sharding across multiple workers on one node.** Pass a stable
`?worker_id=<id>` and the kernel returns only **this worker's slice** of the tasks
(a deterministic modulo shard over the live worker set), so N workers split the load
instead of every worker redoing every task and burning N× GPU for 1× throughput. The
poll doubles as a liveness heartbeat; a worker that stops polling for >60s is dropped
and its tasks reassigned to the survivors on their next poll. **Omit `worker_id` and
you get the whole list** — a single worker needs nothing. The reference worker
(`apps/ai/worker.py`) sends `worker_id` by default (`<hostname>:<pid>`, override with
`HELDAR_AI_WORKER_ID`), so launching two worker processes on one host splits the
tasks automatically. (Idempotency still holds during a rebalance: a task briefly
analyzed by two workers is deduped by the outbox `frame_id`.)

### 5.1a Lease tasks — `POST /api/v1/ai/leases`

Acquire **and** renew are the same call, so a worker's poll loop needs no state
machine: call it every tick and run whatever comes back. Requires `ai:tasks`.

Request:

```json
{ "worker_id": "box-1:1421", "ttl_secs": 60,
  "task_types": ["anpr"], "max_tasks": 32 }
```

`task_types` and `max_tasks` are optional filters. `ttl_secs` is clamped to
`15..=300` (default 60) and must comfortably exceed your poll interval.

Response `200 OK`:

```json
{
  "lease_id": "lse_9f1c…",
  "worker_id": "box-1:1421",
  "expires_at": "2026-06-13T08:16:31.120+00:00",
  "tasks": [ { "id": "ai_3f2a…", "camera_id": "gate_a_01", "…": "…",
               "frame_url": "/api/v1/cameras/gate_a_01/frame?profile=sub&task=ai_3f2a…" } ]
}
```

A task is leased to **one** holder at a time. A second credential asking while the
lease is live gets an empty `tasks` array; the row becomes claimable again once the
lease lapses (expiry is a predicate at claim time — there is no reaper task, so
nothing new competes with the recorder for SQLite's single writer).

`DELETE /api/v1/ai/leases/{lease_id}` releases early on graceful shutdown, so a
restart does not wait out the TTL. It is scoped to the holding credential: a lease
id is not a capability on its own.

`GET /api/v1/ai/tasks` (§5.1) is **unchanged** and still works for a worker that
never leases — it simply gets no frame tickets, which under `warn` behaves exactly
as it always has.

### 5.2 Pull a frame — `GET /api/v1/cameras/{id}/frame`

Serves the latest sampled JPEG for a camera (the worker's input). The worker pulls
this on its own cadence — typically at (or just under) the task fps.

Pass **`?task=<ai_task_id>`** (the `frame_url` from a lease already includes it) to
receive a frame ticket alongside the image. Omit it — as the dashboard does — and
no ticket header is emitted and nothing else changes.

Response `200 OK`:

```
Content-Type: image/jpeg
Cache-Control: no-store
x-frame-age-ms: 142
x-frame-captured-at: 2026-06-13T08:15:31.120+00:00
x-frame-ticket: f1.ai_3f2a….1786269688550.1786269808.E9-8SZSo…

<JPEG bytes>
```

- **`x-frame-age-ms`** — milliseconds since the frame file was last written
  (derived from its mtime). Use it to skip stale frames: if the sampler is
  `offline`, age climbs and the worker should not waste compute on a frozen
  frame.
- **`x-frame-captured-at`** — RFC3339 timestamp of that write; echo it back as the
  detection `timestamp` so detections align to capture time, not post time.
- **`x-frame-ticket`** — present only when `?task=` was passed *and* the caller
  holds a live lease on that task. **Opaque**: carry it back on the POST that
  describes *this* frame, and never parse, cache or reuse it. Every failure mode
  (no lease, a lease on another camera, a lease-table error) degrades to "no
  header" — the frame itself is always served, so perception never goes down
  because leasing had a bad day. Default lifetime 120s
  (`HELDAR_FRAME_TICKET_TTL_SECS`, clamped `10..=900`); tickets do not survive a
  kernel restart, which is harmless because the next pull issues a fresh one.

`404 Not Found` when no frame exists yet (no enabled AI task for the camera, or
the sampler hasn't produced its first frame):

```json
{ "error": "no sampled frame yet (is an AI task enabled for this camera?)" }
```

The `{id}` path segment is validated against `/`, `\`, and `..` (path-traversal
defense), returning `400` for anything suspicious.

### 5.3 Post results — `POST /api/v1/ai/events`

The worker posts a batch of detections for one camera/task, optionally with a
single derived **event** (an alert/incident) in the same call.

Request:

```json
{
  "frame_ticket": "f1.ai_3f2a….1786269688550.1786269808.E9-8SZSo…",
  "camera_id": "gate_a_01",
  "task_type": "detection",
  "timestamp": "2026-06-13T08:15:31.120Z",
  "detections": [
    {
      "label": "person",
      "confidence": 0.92,
      "bbox": [0.41, 0.30, 0.08, 0.22],
      "track_id": "t-17",
      "attributes": { "zone": "entry_lane_a" }
    },
    {
      "label": "car",
      "confidence": 0.81,
      "bbox": [0.10, 0.55, 0.30, 0.40]
    }
  ],
  "event": {
    "event_type": "person_in_red_zone",
    "severity": "warning",
    "payload": { "zone": "red_a", "track_id": "t-17" }
  }
}
```

Field rules (`models.rs::AiIngest`):

- `frame_ticket` — the `x-frame-ticket` from the frame this batch describes.
  Required under `HELDAR_INGEST_PROVENANCE=enforce` (`401 frame_ticket_required`
  without it); optional under `warn`/`off`. When present and valid, `camera_id`,
  `task_type` and `frame_id` are **derived from it**, and the body's own values are
  only cross-checked — `409` if they disagree. Keep sending them anyway: they are a
  useful consistency check and they are what an older kernel needs.
- `camera_id` (**required**) must exist, else `404`.
- `task_type` (**required**) is stored on each detection row.
- `timestamp` optional RFC3339; if omitted/unparseable the server uses `now()`.
  It applies to **all** detections in the batch.
- `detections` optional (defaults to `[]`) — send `[]` to post only an event.
  Every field inside a detection is optional except its position in the array.
- `event` optional. `event_type` is **required** when present; `severity`
  defaults to `info` (use `warning` to trigger the Stage 1 alert
  webhook); `payload` defaults to `{}`. The event is written to the **same
  `events` table** the kernel uses, so AI alerts flow through the existing
  alert/notifier path for free.
- `attributes.source` and `attributes._prov` are **server-owned** and are stripped
  from anything you send. Do not set them.

**Event-type rules for a worker credential.** `event_type` must match
`^[a-z0-9_]{1,64}$` and must not begin with a reserved kernel-domain prefix —
`gate_`, `entry_`, `zone_`, `camera_`, `disk_`, `raid_` — else `400`. It is a
denylist, not an allowlist, so a third-party sidecar's own event types keep
working; what it stops is a forged `gate_opened` reaching webhooks and operator
email, where it would be indistinguishable from a real barrier actuation. Worker
severity is clamped to `info`/`warning`: a worker cannot self-escalate to
`critical`. Kernel-internal producers are unrestricted — they *are* the domain.

Response `200 OK`:

```json
{ "detections_ingested": 2, "ticketed": true }
```

`ticketed` tells you whether the batch was bound to a server-issued frame, without
having to know the box's tier. A worker that expected `true` and reads `false`
has lost its lease and should re-acquire.

A redelivery of an already-ingested frame is a no-op and answers
`{ "detections_ingested": 0, "duplicate": true, "ticketed": … }`.

### 5.4 Sampler status — `GET /api/v1/ai/samplers`

Per-camera sampler state and **effective** (budgeted) fps. Use it for dashboards
and to confirm the kernel is actually producing frames.

```json
[
  { "camera_id": "gate_a_01", "state": "sampling", "fps": 5.0 },
  { "camera_id": "gate_b_02", "state": "offline",  "fps": 2.0 }
]
```

`state` ∈ `connecting` | `sampling` | `offline` | `error` | `stopped`.

### 5.5 Query detections — `GET /api/v1/cameras/{id}/detections`

Read back what has been ingested (UI, audit, downstream consumers).

Query params: `from`, `to` (RFC3339), `label`, `limit` (default 200, clamped
1…5000). Ordered newest-first.

```
GET /api/v1/cameras/gate_a_01/detections?label=person&limit=50
```

```json
[
  {
    "id": "det_a1b2c3d4e5f6...",
    "camera_id": "gate_a_01",
    "task_type": "detection",
    "timestamp": "2026-06-13T08:15:31.120Z",
    "label": "person",
    "confidence": 0.92,
    "bbox": [0.41, 0.30, 0.08, 0.22],
    "track_id": "t-17",
    "attributes": { "zone": "entry_lane_a" },
    "created_at": "2026-06-13T08:15:31.205Z"
  }
]
```

The three endpoints below (§§5.6–5.8) were added for **semantic retrieval**
(§14). They are strictly **additive**: a worker that never runs an `embedding`
task can ignore them, and `GET /api/v1/ai/tasks` (§5.1) is **UNCHANGED** — it
still returns a bare array, so deployed workers keep working. Query embeddings
deliberately do **not** piggyback on the tasks poll: tasks are re-discovered
roughly every 10 s, but the semantic-search route holds the operator's HTTP
request open for only ~3 s waiting for the query vector — so the worker runs a
separate **fast (~1 s) poll** against §5.7 instead. All three require the same
ingest capability as §5.3.

### 5.6 Post embeddings — `POST /api/v1/ai/embeddings`

The worker posts a batch of crop **embeddings** for one camera — the write half
of semantic retrieval, produced by the `embedding` analyzer (§14). Body limit
24 MiB.

Request:

```json
{
  "camera_id": "gate_a_01",
  "model": "open_clip/ViT-B-32-quickgelu/openai",
  "dim": 512,
  "frame_id": "ai_3f2a9c1b...:2026-07-16T10:00:00Z",
  "items": [
    {
      "track_id": "t17",
      "detection_id": null,
      "label": "car",
      "timestamp": "2026-07-16T10:00:00Z",
      "bbox": [0.31, 0.40, 0.22, 0.30],
      "vec": [0.0132, -0.0871, 0.0455],
      "thumb_b64": "<base64 JPEG crop>"
    }
  ]
}
```

Field rules (`services/embeddings.rs`):

- `camera_id` (**required**) must exist, else `404`.
- `model` (**required**, non-empty) names the embedding space the vectors live in
  (e.g. `open_clip/ViT-B-32-quickgelu/openai`); search only ranks vectors against a query
  from the same space.
- `dim` (**required**, 1…4096). Every item's `vec` must be **exactly `dim`
  finite floats**, else `400`.
- `items` — 1…128 per request (`400` beyond that; the reference worker chunks
  larger sets client-side).
- `frame_id` — optional **idempotency key** shared by the whole batch (the
  reference worker uses `"{task_id}:{captured_at}"`). Rows insert
  `ON CONFLICT DO NOTHING` against a unique `(camera_id, frame_id, track_id)`
  index, so a **redelivered batch is deduped silently** — the same at-least-once
  posting semantics as §5.3.
- Per item, `track_id`, `detection_id`, `label`, `bbox` (normalized `[x,y,w,h]`,
  §7), and `timestamp` (RFC3339; default server `now()`) are all optional.
- `thumb_b64` — optional JPEG crop thumb, ≤ 131 072 base64 chars (~96 KB, `400`
  beyond). Decoded and written to the snapshots dir as `emb_<id>.jpg` **only for
  rows actually inserted**, served at `/media/snapshots/emb_<id>.jpg` — these
  are the ranked crops the search UI shows. Invalid base64 skips the thumb but
  keeps the row.

Response `200 OK`:

```json
{ "embeddings_ingested": 2 }
```

The count is rows **actually inserted** — deduped redeliveries are not counted.

### 5.7 Claim query embeddings — `GET /api/v1/ai/embed-queries?worker_id=<id>`

The pull-only **query queue**. When an operator runs a semantic search, the
kernel enqueues the query text/image and blocks the search request (~3 s,
`HELDAR_SEARCH_EMBED_TIMEOUT_MS`) waiting for a worker to embed it — which is
why the worker polls this endpoint **fast** (default every 1 s, §14.3): the
~10 s §5.1 cadence could never answer inside that window.

- Atomically **claims** up to 4 pending queries (`pending → claimed`, stamping
  `claimed_at` / `claimed_by = worker_id`).
- **Read-only when the queue is empty** — the overwhelmingly common idle poll
  performs no write.
- Queries older than 60 s are expired and never delivered (the searcher long
  since got its `503`).

`200 OK` — an **object**, not a bare array, so the shape can grow:

```json
{ "queries": [ { "id": "embq_7c1f...", "kind": "text", "payload": "red pickup truck" } ] }
```

`kind` is `text` (payload = the query string) or `image` (payload = a base64
JPEG/PNG).

### 5.8 Answer a query — `POST /api/v1/ai/embed-queries/{id}/result`

Post the query vector back (body limit 1 MiB) — or an error, so the waiting
search fails fast instead of timing out:

```json
{ "vec": [0.0132, -0.0871, 0.0455], "model": "open_clip/ViT-B-32-quickgelu/openai", "dim": 512 }
```

```json
{ "error": "clip backend unavailable" }
```

`vec` is validated like ingest (length == `dim`, all finite, `dim` 1…4096), and a
success result **must name its `model`** (400 without it): same-dim vectors from
different CLIP checkpoints are incomparable spaces, and the model id is the search
prefilter that keeps them apart.
**First result wins**: the update applies only while the query is still
`pending`/`claimed`; a late duplicate is a `200` no-op. Response:

```json
{ "updated": true }
```

(`false` when another worker already answered or the row expired.)

---

## 6. The worker loop (pseudocode)

```
# Supervisor tick, every few seconds. Acquire and renew are the same call.
tasks = POST /api/v1/ai/leases { worker_id, ttl_secs }   -> .tasks
      | on 404 (kernel predates leases): GET /api/v1/ai/tasks

for each task (own thread / async task):
    loop at ~task.fps:
        resp = GET task.frame_url                # frame_url already carries &task=
        if resp is 404:        sleep, continue   # no frame yet
        if x-frame-captured-at == last_seen: continue   # unchanged frame; skip
        # (optionally also skip when x-frame-age-ms is too high → sampler frozen)
        ticket = resp.headers["x-frame-ticket"]  # may be absent — that is fine
        dets, event = analyze(task, resp.body)
        if dets or event:
            POST /api/v1/ai/events { frame_ticket = ticket,
                                     camera_id, task_type, timestamp,
                                     detections = dets, event = event }
            # 401 frame_ticket_required / 409  -> drop this batch, re-acquire the
            # lease, and carry on with the NEXT frame. Never resend the same body
            # under a different ticket: a ticket names one specific captured frame.
```

Because `latest.jpg` is last-value, pulling faster than the sampler writes just
returns the same frame; the reference worker **dedupes on `x-frame-captured-at`**
to avoid re-analyzing an unchanged frame. Pulling slower than the sampler simply
drops intermediate frames — acceptable for detection/tracking at these rates.

---

## 7. Conventions

- **`bbox` is `[x, y, w, h]` normalized 0…1**, top-left origin (per the
  `detections` comment in `migrations/0001_init.sql`). Normalizing means detections survive any
  later change to `width` and are resolution-independent for the UI. Stored as
  raw JSON; the kernel does not validate the shape, so the worker owns
  correctness.
- **`stream_profile`** — `sub` (default, light, for continuous detection/tracking)
  or `main` (heavier, for plate/face crops). Stored and surfaced to workers; the
  sampler currently always samples the sub-stream (§2/§10). For a true high-res
  grab today, a worker can hit the Stage 0 snapshot endpoint
  `GET /api/v1/cameras/{id}/snapshot` (live main/sub frame) on a trigger.
- **`config` blob** — opaque to the kernel; the worker's contract with itself.
  Conventions to adopt: `{"classes": [...], "min_confidence": 0.4, "zones":
  [...], "model": "...", "model_version": "..."}`. Keep model versions here so
  detections are reproducible/auditable (`audit.model_versions`).
- **`task_type`** — free-form string; it is echoed onto every detection and is
  how `/detections?label=` consumers and downstream stages distinguish pipelines
  (`detection`, `anpr`, `tracking`, `vehicle_attr`, …).
- **Timestamps** — always RFC3339 UTC. The ingest `timestamp` is optional; if
  omitted the server stamps `now()`. The reference worker posts `now()` and uses
  `x-frame-captured-at` only to dedupe unchanged frames; if you want detections to
  align to *capture* time rather than post time, echo `x-frame-captured-at` as the
  ingest `timestamp` instead.

---

## 8. Writing your own worker

A worker is any process that can speak the §5 HTTP contract. The **reference
implementation** ships at **`apps/ai/worker.py`** (with `apps/ai/README.md`,
`requirements.txt`, and a `Dockerfile`) — a small, production-shaped Python worker
that proves the whole contract end-to-end. Its only deps are `requests`, `Pillow`,
and `numpy` (no GPU, no model). It runs:

- a **supervisor** thread that polls `/ai/tasks` every `--poll-interval` seconds
  (default 10) and reconciles a set of **per-task threads** — starting new tasks,
  stopping removed ones, and restarting a task whose `signature()` (type / fps /
  width / config / frame_url) changed;
- one **`TaskRunner`** thread per task looping at the task's `fps`: pull frame →
  run its `Analyzer` → POST results;
- a `CoreClient` with capped exponential backoff + jitter (4xx are *not* retried,
  5xx / connection errors are), and graceful `SIGINT`/`SIGTERM` shutdown
  (every sleep/backoff is interruptible).

It **dedupes unchanged frames** on `x-frame-captured-at` (the worker fps may
exceed the sampler's), and a `404` from the frame endpoint is treated as "no
frame yet" — a skipped cycle, not an error.

### The `Analyzer` interface — the Stage 3 seam

`worker.py` defines an abstract base class where models plug in. **One instance is
created per task thread**, so per-camera state (a previous frame, a tracker) can
live on `self`:

```python
class Analyzer(ABC):
    name: str = "analyzer"
    def __init__(self, config: dict, log): ...      # config = the task's `config` blob
    @abstractmethod
    def analyze(self, frame: FrameContext) -> AnalysisResult: ...
```

- **`FrameContext`** carries `frame.task`, `frame.raw` (JPEG bytes),
  `frame.captured_at`, `frame.age_ms`, and lazy decode helpers `frame.image()`
  (a `PIL.Image`) and `frame.gray_array(width)` (downscaled grayscale `numpy`).
- **`AnalysisResult`** = `{ detections: list[Detection], event: Event | None }`.
  `Detection(label, confidence, bbox=[x,y,w,h] 0…1, track_id, attributes)` and
  `Event(event_type, severity, payload)` serialize to the §5.3 ingest shapes.
- Analyzers are registered by `task_type` via `register(task_type, cls)`; an
  unknown `task_type` falls back to a **`PlaceholderAnalyzer`** that pulls/decodes
  the frame (exercising the path) but **never fabricates detections** — it just
  warns, rate-limited, that a real model must be wired in.

### Ships with a working model-free analyzer

The reference registers a real **`MotionAnalyzer`** for `task_type = "motion"`:
frame-differencing (grayscale downscale → abs-diff vs the previous frame → changed-
pixel fraction vs `config.threshold`, default 0.02), emitting a `motion` detection
with the changed-region bbox plus a `motion` event. So you can validate the entire
sampler → worker → ingest → events path **with no model and no GPU** by creating an
`ai_task` with `task_type: "motion"` on a camera.

### Run it

```bash
cd apps/ai
python3 -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
HELDAR_API=http://localhost:8000 python worker.py
# or: python worker.py --api http://localhost:8000 --log-format json
```

Worker-side config (CLI flag / env var): `--api`/`HELDAR_API`
(default `http://localhost:8000`), `--poll-interval`/`HELDAR_AI_POLL_INTERVAL`
(10), `--http-timeout`, `--http-max-retries`, `--backoff-base`, `--backoff-cap`,
`--log-level`, `--log-format`. Full table in `apps/ai/README.md`.

### Stage 3 adds the real models

Stage 2 deliberately stops at the **contract + sampler + reference loop + a
model-free motion analyzer.** The actual perception — **person/vehicle detection
(YOLO / RT-DETR), multi-object tracking (ByteTrack / BoT-SORT), zones, and the
canonical event model** — arrives in **Stage 3** and slots in behind the
`Analyzer` interface with no change to the kernel or the HTTP contract (see
`ROADMAP.md` Stage 3). Concretely, Stage 3 adds a subclass
and one `register(...)` call:

```python
from worker import Analyzer, AnalysisResult, Detection, FrameContext, register

class YoloAnalyzer(Analyzer):
    name = "yolo"
    def __init__(self, config, log):
        super().__init__(config, log)
        import ultralytics
        self.model = ultralytics.YOLO(config.get("weights", "yolov8n.pt"))
        self.conf = float(config.get("threshold", 0.25))
    def analyze(self, frame: FrameContext) -> AnalysisResult:
        img = frame.image(); w, h = img.size
        dets = []
        for r in self.model(img, conf=self.conf, verbose=False):
            for b in r.boxes:
                x1, y1, x2, y2 = b.xyxy[0].tolist()
                dets.append(Detection(
                    label=self.model.names[int(b.cls)], confidence=float(b.conf),
                    bbox=[x1/w, y1/h, (x2-x1)/w, (y2-y1)/h]))   # normalized 0..1
        return AnalysisResult(detections=dets)

register("detection", YoloAnalyzer)   # replaces the placeholder for task_type "detection"
```

---

## 9. Configuration

All via `HELDAR_*` env vars (`config.rs`):

| Var | Default | Meaning |
|---|---|---|
| `HELDAR_AI_ENABLED` | `true` | master switch for frame sampling; `false` runs no samplers |
| `HELDAR_AI_MAX_TOTAL_FPS` | `40` | global fps budget split across AI-enabled cameras (floored at 1.0) |
| `HELDAR_DEFAULT_AI_FPS` | `5` | default `fps` for a task that omits it (clamped 0.1…30 on write) |
| `HELDAR_DEFAULT_AI_WIDTH` | `1280` | default `width` for a task that omits it (clamped 160…3840) |
| `HELDAR_FRAMES_DIR` | `<DATA_DIR>/frames` | where `latest.jpg` per camera is written (`frames/<camera_id>/latest.jpg`) |

The **worker** side (`apps/ai/worker.py`) is configured separately: `HELDAR_API`
(base URL of the core, default `http://localhost:8000`),
`HELDAR_AI_POLL_INTERVAL`, and HTTP/backoff/logging knobs — full table in
`apps/ai/README.md`.

---

## 10. What's built vs. deferred (honest scope)

| Stage 2 item | Status in Stage 2 | Notes |
|---|---|---|
| Substream sampler (decode only sampled frames) | ✅ | one ffmpeg per camera, `-vf fps,scale`, decode-free recording untouched |
| FPS budgeting + task scheduler | ✅ | global `HELDAR_AI_MAX_TOTAL_FPS` split; per-camera = `min(task fps, budget/active)`, `MIN_FPS=0.5` floor |
| Frame queue / frame-sample object | ◑ | realized as a **single `latest.jpg` per camera** (last-value), not a multi-frame queue or `frame_id` stream; staleness via `x-frame-age-ms` |
| Backpressure policy | ◑ | **static** proportional fps reduction as cameras are added (graceful fps degradation). The dynamic *resolution* ladder (720p→480p) + auto-recovery from live load is **deferred** (Stage 3+) |
| High-res snapshot on trigger | ◑ | not in the sampler; a worker can use the Stage 0 `GET /api/v1/cameras/{id}/snapshot` for a main-stream grab on trigger. Per-task `stream_profile=main` is stored/validated but the sampler currently always samples the sub-stream |
| Worker contract (discover/pull/post/query) | ✅ | full `routes/ai.rs` surface, this guide |
| Detection / tracking / zone models | ⬜ | **Stage 3** — slots into the `Analyzer` seam (§8) |
| Semantic embeddings + retrieval (issue #38) | ✅ (added post-Stage 4) | three **additive** endpoints — `POST /ai/embeddings` + the `embed-queries` claim/result queue (§§5.6–5.8) — plus the `embedding` analyzer and query worker (§14); the §5.1 tasks response is untouched. VLM interpretation, ANN indexes, and person/face re-id embeddings are **deliberately deferred** (§14) |

**Success criterion met:** the sampler is a separate set of supervised ffmpeg
processes decoding only the sub-stream at a bounded total fps, with crash/backoff
isolation; the recorder's 24/7 `-c copy` path and the MediaMTX live view are
completely independent of it. AI consuming frames cannot break recording or live
view.

---

## 11. Stage 3 — detection + tracking analyzer and the zone engine

Stage 3 turns frames into **events**. It has two halves that meet at the **unchanged
§5.3 ingest contract**:

1. a worker-side **YOLO + ByteTrack analyzer** that posts *tracked* detections, and
2. a kernel-side **zone engine** that turns tracked detections into zone events.

Kernel implementation: `services/zones.rs`, `routes/zones.rs`, and the zones
tables in `migrations/0001_init.sql` (see [`ARCHITECTURE.md`](../ARCHITECTURE.md) §16).

### 11.1 The YOLO + ByteTrack analyzer (worker side)

Stage 3 registers a real `Analyzer` for the `detection` task type — the seam §8
already defined. Nothing in §§1–10 changes: the worker still discovers tasks, pulls
`latest_<profile>.jpg`, and POSTs to `/api/v1/ai/events`. The analyzer just fills in
the optional **`track_id`** on each detection.

- **Detector (YOLO / RT-DETR)** — runs on each pulled frame, producing
  class-labelled boxes (`person`, `car`, `truck`, `motorcycle`, …). Boxes are
  emitted as `bbox = [x, y, w, h]` **normalized 0…1**, top-left origin (the §7
  convention) so they are resolution-independent.
- **Tracker (ByteTrack)** — associates boxes across consecutive frames
  (including low-confidence ones) into continuous tracks and assigns a **stable
  `track_id`** per object. Because §8 creates **one `Analyzer` instance per task
  thread**, the tracker's per-camera state (Kalman filters, active tracks) lives on
  `self` and naturally persists across that camera's frame stream — no global state,
  no cross-camera bleed.
- **Anonymous by default** — `track_id` is a per-session track handle,
  **not** an identity. Cross-camera ReID is Stage 6.
- It is registered exactly like any analyzer:

  ```python
  from worker import Analyzer, AnalysisResult, Detection, FrameContext, register

  class YoloByteTrackAnalyzer(Analyzer):
      name = "yolo+bytetrack"
      def __init__(self, config, log):
          super().__init__(config, log)
          from ultralytics import YOLO
          self.model = YOLO(config.get("weights", "yolov8n.pt"))
          self.conf  = float(config.get("threshold", 0.25))
          self.classes = config.get("classes")          # e.g. ["person","car"]; None = all
          self.tracker = config.get("tracker", "bytetrack.yaml")

      def analyze(self, frame: FrameContext) -> AnalysisResult:
          img = frame.image(); w, h = img.size
          # persist=True keeps ByteTrack state on this per-task instance across frames
          res = self.model.track(img, persist=True, conf=self.conf,
                                 classes=self.classes, tracker=self.tracker, verbose=False)
          dets = []
          for r in res:
              for b in r.boxes:
                  if b.id is None:        # not yet confirmed by the tracker
                      continue
                  x1, y1, x2, y2 = b.xyxy[0].tolist()
                  dets.append(Detection(
                      label=self.model.names[int(b.cls)],
                      confidence=float(b.conf),
                      bbox=[x1/w, y1/h, (x2-x1)/w, (y2-y1)/h],   # normalized 0..1
                      track_id=f"t{int(b.id)}"))
          return AnalysisResult(detections=dets)

  register("detection", YoloByteTrackAnalyzer)
  ```

  The exact import/model is an implementation detail; what the kernel relies on is
  only the posted shape: `{label, confidence, bbox:[x,y,w,h] 0..1, track_id}`. Keep
  `model` / `model_version` in the task `config` for reproducibility (`audit.model_versions`).

> **Engineering vs. accuracy.** The *plumbing* — detector +
> tracker behind the seam, posting tracked detections that drive zone events — is
> production-grade. Model **accuracy** is **not** yet validated on local footage:
> Malaysian vehicle mix, plate/camera angles, motorcycles, night-IR and rain,
> and ReID/association degradation in crowds and across sites. Use
> type + color first, treat make/model and any identity-like match as top-5
> assistive candidates with human review, benchmark on local gate/shop footage, and
> never make a hard access decision on model recognition until it's locally
> benchmarked.

### 11.2 What "tracked" buys you — driving the zone engine

When a posted detection has **both** a `track_id` and a `bbox`, the kernel feeds it
to the **zone engine** synchronously inside `POST /api/v1/ai/events` (right after the
detections are committed). Detections without a `track_id`/`bbox` are still stored,
but cannot drive zone events. So a `motion` analyzer (§8, no track ids) populates
`detections` but raises no zone events; the `detection` analyzer above does both.

End-to-end, per camera:

```
ai_task {task_type:"detection"}  →  sampler decodes sub-stream → frames/<cam>/latest_sub.jpg
        worker: pull frame → YOLO boxes → ByteTrack track_ids
        worker: POST /api/v1/ai/events { detections:[{label,confidence,bbox,track_id}], ts }
                │
   kernel: insert detections (tx)  →  detections table
   kernel: ZoneEngine.process(camera_id, ts, detections)
        for each tracked detection:
          ground point = bbox bottom-center [x+w/2, y+h]
          point-in-polygon vs each enabled zone (label filter applied)
          per-(camera,zone,track) state machine → enter / exit / dwell
                │                                      │
                ▼                                      ▼
        zone_events row (+ evidence frame)     events log "zone_{enter,exit,dwell}"
                                               (severity = zone.severity → §5.3 alert webhook)
```

The engine uses the bbox's **bottom-center** (ground contact — feet/tyres), not its
centroid, so "is this object inside the floor region?" is correct. It holds per-track
membership state in memory keyed by `camera|zone|track`, emits **`enter`** on
crossing in, **`dwell`** once when `now − entered ≥ zone.dwell_seconds` (if armed),
and **`exit`** on crossing out; track state not seen for 120 s is pruned. Full state
machine in [`ARCHITECTURE.md`](../ARCHITECTURE.md) §16.2.

### 11.3 Zones API — `routes/zones.rs`

A **zone** is a polygon region on a camera (the `zones` table in
`migrations/0001_init.sql`, `models.rs::Zone`). Coordinates are **normalized 0…1**, matching the detection
`bbox`, so a zone drawn on the UI overlay maps directly onto detections regardless of
sample resolution.

| Field | Type | Notes |
|---|---|---|
| `id` | text PK | `zone_<uuid>`, server-assigned |
| `camera_id` | text FK | → `cameras(id)` `ON DELETE CASCADE` |
| `name` | text | **required** |
| `kind` | text | default `region`; free-form (`region`/`restricted`/`count`/…) — your app's semantics, opaque to the engine |
| `polygon` | JSON | `[[x,y], …]` normalized 0…1; **≥3 points** (validated on write) |
| `dwell_seconds` | real | default 0; `>0` arms a `dwell` event past this threshold |
| `labels` | JSON | array of detection labels that count toward this zone (**empty = all labels**) |
| `severity` | text | `info`/`warning`/`critical` — stamped on emitted events (`warning`/`critical` → alert webhook) |
| `config` | JSON | free-form per-zone blob (default `{}`) |
| `enabled` | bool | default `true`; only enabled zones are evaluated |
| `created_at` / `updated_at` | RFC3339 | |

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/v1/cameras/{id}/zones` | list a camera's zones (incl. disabled), oldest-first |
| POST | `/api/v1/cameras/{id}/zones` | create a zone → `201` + the zone |
| PATCH | `/api/v1/zones/{zone_id}` | partial update (any subset of fields) |
| DELETE | `/api/v1/zones/{zone_id}` | delete → `204` (`404` if unknown) |
| GET | `/api/v1/cameras/{id}/zone-events` | query zone events (filters below) |

Create a restricted-area zone that only reacts to people and fires a `dwell` alert
after 30 s:

```http
POST /api/v1/cameras/gate_a_01/zones
Content-Type: application/json

{
  "name": "loading_dock_restricted",
  "kind": "restricted",
  "polygon": [[0.10,0.55],[0.45,0.55],[0.45,0.95],[0.10,0.95]],
  "labels": ["person"],
  "dwell_seconds": 30,
  "severity": "warning"
}
```

`201 Created` echoes the stored zone (with `id`, `enabled:true`, timestamps). A
`polygon` with fewer than 3 points or an invalid `severity` is rejected `400`.

### 11.4 Zone events the engine raises

The engine raises three event types per `(zone, track)`:

| `event_type` | When | Carries |
|---|---|---|
| `enter` | a tracked object's ground point crosses **into** the polygon | `track_id`, `label`, `timestamp`, **`evidence_path`** (entry frame) |
| `dwell` | object has stayed inside `≥ zone.dwell_seconds` (only if armed) | above **plus `dwell_seconds`** (measured); fires **once** per visit |
| `exit` | the ground point crosses **out** of the polygon | `track_id`, `label`, `timestamp` |

Each event is written **twice**: a row in **`zone_events`** (queryable below) **and**
an entry in the kernel **`events`** log as `zone_enter` / `zone_dwell` / `zone_exit`
at the zone's `severity`. The latter means a `warning`/`critical` zone event flows
through the **Stage 1 alert notifier/webhook** with zero extra wiring — exactly like
a worker-posted `event` (§5.3). **Evidence** is captured **only on `enter`**: the
kernel copies the camera's latest sampled sub-stream frame to
`/media/snapshots/zoneevt_<id>.jpg` (a cheap file copy, no decode) and stores that
URL as `evidence_path`.

### 11.5 Query zone events — `GET /api/v1/cameras/{id}/zone-events`

Read back what crossed which zones (UI timeline, audit, reports).

Query params: `from`, `to` (RFC3339), `zone_id`, `event_type`
(`enter`|`exit`|`dwell`), `limit` (default 200, clamped 1…5000). Newest-first.

```
GET /api/v1/cameras/gate_a_01/zone-events?zone_id=zone_ab12...&event_type=dwell&limit=50
```

```json
[
  {
    "id": "zev_7c1f...",
    "camera_id": "gate_a_01",
    "zone_id": "zone_ab12...",
    "zone_name": "loading_dock_restricted",
    "track_id": "t17",
    "event_type": "dwell",
    "label": "person",
    "timestamp": "2026-06-13T08:15:31.120Z",
    "dwell_seconds": 31.4,
    "evidence_path": null,
    "created_at": "2026-06-13T08:15:31.205Z"
  }
]
```

`zone_name` is denormalized onto the event, so zone events stay self-describing even
after the zone is renamed or deleted (`zone_events` has no FK back to `zones`).

### 11.6 Putting it together (operator flow)

1. Create an `ai_task` with `task_type:"detection"` on a camera (§4). The sampler
   starts producing `latest_sub.jpg`.
2. Run a worker with the §11.1 YOLO+ByteTrack analyzer registered for `detection`.
   It pulls frames and posts tracked detections.
3. Draw one or more **zones** on the camera (§11.3), with `labels` / `dwell_seconds`
   / `severity` as needed.
4. Tracked objects crossing those polygons now raise `enter`/`exit`/`dwell` **zone
   events** with evidence; `warning`/`critical` ones alert via the Stage 1 webhook.
5. Query history via `/zone-events` (and the raw boxes via `/detections`, §5.5).

---

## 12. Stage 4 — the ANPR analyzer

Stage 4 adds an `anpr` task type and registers a real `Analyzer` for it
(`AnprAnalyzer` in `apps/ai/worker.py`) — again **with no change to §§1–10**: the
worker still discovers tasks, pulls `latest.jpg`, and POSTs to `/api/v1/ai/events`.
The kernel routes `task_type == "anpr"` results into the **entry engine**
(`services/anpr.rs`), which does temporal plate voting + registry resolution. The
engine and its event model are documented in [`docs/ACCESS-CONTROL.md`](ACCESS-CONTROL.md)
and [`ARCHITECTURE.md`](../ARCHITECTURE.md) §17; this section is the **worker** half.

### 12.1 The vehicle → plate → OCR pipeline

`AnprAnalyzer` shares the Stage 3 backbone (**YOLOv8 + ByteTrack**), restricted to
vehicle classes for speed, and emits **one detection per vehicle box per frame**,
each with a stable `track_id`:

```
frame → YOLO vehicle boxes → ByteTrack track_id      (per task thread, state on self)
            │
            ├─ vehicle_type   = YOLO class (car/truck/bus/motorcycle/…)
            ├─ color          = coarse HSV heuristic over the box centre (assistive)
            └─ plate          = OCR over the vehicle crop  ── IF an OCR backend is installed
```

Per-task `config` keys (all optional): `weights` (default `yolov8n.pt`), `threshold`
(min vehicle confidence, default `0.3`), `ocr` (force a backend), `direction`
(`inbound`/`outbound`), `device` (default auto), `min_box_area` (ignore boxes smaller
than this fraction of the frame), `imgsz`.

### 12.2 OCR backends are OPTIONAL (and never fabricate)

Plate reading uses a lazy `_OcrBackend` that tries **PaddleOCR** then **EasyOCR** (or
exactly the one named in `config.ocr`). **Both are optional Python packages.** If
neither is installed:

- the analyzer **stays enabled** and keeps emitting vehicles **with attributes but
  WITHOUT a plate** — it **never fabricates a plate**;
- the core engine still receives the vehicle reads and will log unreadable-/no-plate
  events (`auth_status: unmatched`, `note: no_plate_read`) for guard review.

When a backend *is* present, `read_plate` keeps the **most plate-like** token: it
normalizes each OCR candidate to uppercase alphanumerics and accepts it only if it is
**3–10 chars and mixes a letter and a digit** (the same plausibility gate the core
applies), returning the highest-confidence survivor as `(text, confidence)`. Install
them only if you want plate reads (see `apps/ai/requirements.txt`):

```bash
pip install paddleocr      # or: pip install easyocr
```

### 12.3 Color heuristic + direction config

- **Color** (`_estimate_color`) is a crude dominant-color estimate over the central
  50 % of the vehicle box → one of `black/white/gray/red/orange/yellow/green/blue/
  purple` or none. The names match what an operator types when registering a vehicle,
  so the core's **case-insensitive** mismatch check lines up. It is **assistive
  metadata only**, never an access decision, and real accuracy needs
  local benchmarking.
- **Direction** is a **per-camera config hint**, not geometry: `config.direction =
  "inbound" | "outbound"`. There is **no calibrated line-crossing** in the worker or
  kernel yet, so a single-direction gate camera supplies its direction this way; the
  core uses it to choose `vehicle_entry` vs `vehicle_exit` and to gate visitor-pass
  auto-check-in.

### 12.4 The per-frame `attributes` contract the engine consumes

Each ANPR detection is the standard §5.3 shape (`label` = vehicle type, `confidence`,
`bbox` normalized `[x,y,w,h]`, `track_id`) with an `attributes` object the core ANPR
engine reads:

| `attributes` key | Type | Emitted when | Engine use |
|---|---|---|---|
| `plate` | string | OCR backend present **and** a plausible token read | normalized → the voted identity key |
| `plate_confidence` | number 0…1 | with `plate` | vote tie-break + stored `plate_confidence` |
| `vehicle_type` | string | always (YOLO class) | secondary mismatch check vs registered vehicle |
| `color` | string | when the heuristic returns one | secondary mismatch check (case-insensitive) |
| `make` | string | *(not emitted by the reference worker — no make classifier)* | assistive only; **never** a mismatch trigger |
| `model` | string | *(not emitted by the reference worker)* | assistive only |
| `direction` | `"inbound"`/`"outbound"` | when `config.direction` is set | event type + pass auto-check-in |
| `model_versions` | object | always | stamped into the event's `audit.model_versions` |

`model_versions` from the reference worker looks like
`{"anpr": "anpr_v0.1_<paddleocr|easyocr|noocr>", "vehicle_attr": "heuristic_v0.1",
"detector": "yolov8n.pt"}`. The engine keeps the **highest-confidence** observation of
each attribute across the track's frames and votes the **plate** across frames — so a
single noisy read is outvoted (see [`docs/ACCESS-CONTROL.md`](ACCESS-CONTROL.md) §2.2).

#### Server-owned keys: `source` and `_prov`

Two keys in `attributes` are **written by the kernel, never by you**, and any value
you send for them is discarded before the row is persisted:

| Key | Value | Meaning |
|---|---|---|
| `source` | `"worker"` | the batch arrived over the HTTP ingest API |
| `source` | `"camera_native"` | the batch came from the kernel's own camera-native ANPR poller — **unreachable from the API** |
| `_prov` | `{"key","task","worker"}` | which credential/lease produced it (worker batches) |
| `_prov` | `{"producer":"native_anpr"}` | which in-process kernel producer produced it |

This is load-bearing for the barrier. `heldar-entry` weights a `camera_native` read
at the full configured vote threshold — the device already consolidated multiple
frames itself — so one such read commits an entry event immediately. A worker read
carries one vote and needs `anpr_min_votes` of them. Since `source` used to arrive
inside client-supplied attributes, anything holding an integration key could claim
to be the camera's on-board engine; now the value is a function of *how the batch
entered the kernel*, and the external API cannot express the authoritative one.

Two further gate hardenings ride alongside it:

- **One vote per frame.** A batch is one frame, so it contributes at most one vote
  per `(track, plate)`. Repeating the same detection N times in a single body no
  longer reaches the threshold; N distinct tickets — i.e. N distinct sampled frames
  off that physical camera — are required.
- **Commit-on-prune never actuates.** When a track ages out below the vote
  threshold, the entry event is still written (the audit record is the point) but
  it is marked `workflow_status = "review"`, a `gate_review_not_actuated` event is
  raised, and the barrier is **not** opened. Previously a single accepted read still
  opened the gate about 30 seconds later, which left the gate capability intact even
  with the vote path hardened.

The entry event's `audit` block now names the real producer instead of the old
hard-coded `"system"`, and carries `evidence: { votes, min_votes, source, key,
actuated }`. A `gate_opened` event carries the same `provenance` block — the first
time a barrier opening is attributable to a specific credential.

Example posted detection:

```json
{
  "label": "car",
  "confidence": 0.86,
  "bbox": [0.31, 0.40, 0.22, 0.30],
  "track_id": "t17",
  "attributes": {
    "vehicle_type": "car",
    "color": "white",
    "direction": "inbound",
    "plate": "ABC1234",
    "plate_confidence": 0.91,
    "model_versions": { "anpr": "anpr_v0.1_paddleocr", "vehicle_attr": "heuristic_v0.1", "detector": "yolov8n.pt" }
  }
}
```

> **Accuracy needs local benchmarking.** As with Stage 3, the ANPR *engineering* is
> production-grade, but plate OCR, color, and (future) make/model **accuracy** is not
> validated on local Malaysian gate footage. Treat attributes as
> assistive, surface mismatches as **guard-review exceptions**, and never make a hard
> access decision on recognition until it is locally benchmarked.

---

See also: [`ARCHITECTURE.md`](../ARCHITECTURE.md) §15 (Stage 2 implementation), §16
(Stage 3 detection/tracking/zone kernel), and §17 (Stage 4 Access Control),
[`docs/ACCESS-CONTROL.md`](ACCESS-CONTROL.md) (the entry engine + RBAC + reports),
[`ROADMAP.md`](../ROADMAP.md) Stages 2–4 (checklists),
[`docs/OBSERVABILITY.md`](OBSERVABILITY.md) (Stage 1 metrics/alerts the AI + zone +
entry events feed into).


## 13. ANPR accuracy benchmarking + DIY make/model classifier (issue #37)

Accuracy claims only mean anything on **your** plates and lighting, so the loop is DIY:
collect → label → score, with `apps/ai/anpr_bench.py`:

```bash
# 1) Collect vehicle crops + side-by-side OCR reads (images dir, video file, or live kernel frames)
python3 anpr_bench.py collect --kernel http://127.0.0.1:8000 --camera cam7 \
    --api-key vok_... --duration 300 --out bench/

# 2) Label: fill the `truth` column in bench/manifest.csv (crops are in bench/crops/)

# 3) Score: exact accuracy, character (Levenshtein) accuracy, read rate — per OCR backend
python3 anpr_bench.py score --out bench/
```

Every installed OCR backend (PaddleOCR, EasyOCR) is run on every crop so the manifest compares
them directly; backends that aren't installed simply don't appear. The optional `truth_make_model`
column scores the classifier below the same way.

**DIY make/model classifier**: the ANPR analyzer accepts an ONNX image classifier over vehicle
crops via task config — `make_model_onnx` (path), `make_model_labels` (one "Make Model" per line),
`make_model_min_conf` (default 0.5), `make_model_input` (default 224, ImageNet normalization).
Bring your own weights (e.g. a ResNet fine-tuned on Stanford Cars / VMMRdb / local footage;
`pip install onnxruntime`). Predictions populate the `make`/`model` attributes the entry engine
already consumes — and by policy those are SECONDARY assist only (a mismatch against the registry
raises a guard-review exception, never an auto-reject), so imperfect weights degrade to extra
reviews, not wrong gate decisions. The benchmark's `--make-model-onnx/--make-model-labels` flags
run the same classifier during collection so its accuracy is measured before it is trusted.

---

## 14. Semantic retrieval — the embedding analyzer + query worker (issue #38)

Semantic retrieval adds an `embedding` task type and registers a real `Analyzer`
for it (`EmbeddingAnalyzer` in `apps/ai/worker.py`) — again **with no change to
§§1–10's frame path**: the worker still discovers tasks and pulls `latest.jpg`.
Unlike every previous analyzer, though, it does **not** POST to
`/api/v1/ai/events` — it posts vectors through the three §§5.6–5.8 endpoints.
The kernel indexes them (the `embeddings` table, pruned on the detections TTL
and shed first by the DB size-cap), and `heldar-search` ranks them at query time
(`POST /api/v1/search/semantic`, brute-force cosine top-k). This section is the
**worker** half: the write path (14.1) and the query-answer path (14.3).

### 14.1 The detect → crop → CLIP pipeline

`EmbeddingAnalyzer` shares the Stage 3 backbone (**YOLOv8 + ByteTrack**,
`model.track(persist=True)`, state on `self` per task thread) and, once per
`(track, stride bucket)`, crops the box and embeds it with **open_clip**:

```
frame → YOLO boxes → ByteTrack track_id             (per task thread, state on self)
            │
            ├─ stride gate  = embed on FIRST sight of a track, then every stride_seconds
            ├─ crop → CLIP image encoder             (all due crops in ONE batched forward pass)
            └─ JPEG thumb (≤ thumb_max_px, quality 70)  — the ranked crop the search UI shows
        POST /api/v1/ai/embeddings { model, dim, frame_id, items: [{track_id, label, bbox, vec, thumb_b64}] }
```

- **Stride + dedup semantics.** A per-track monotonic-clock gate on the instance
  embeds each track on first sight and then every `stride_seconds` (default 10) —
  **but only while it moves**: once the stride elapses, a track whose bbox has
  shifted less than `static_epsilon` since its last embed is skipped (static
  suppression, mirroring the zone engine's knob), so a parked car doesn't accrete
  thousands of near-identical vectors and thumbs per day. Even a static track is
  re-embedded every `static_refresh_seconds` (default 1 h) so a permanently
  parked object stays in the index after its older rows age out on the retention
  TTL. Idle gate entries are pruned. **Untracked boxes are skipped** — without a
  track identity there is nothing to stride against.
  Redelivery dedup is kernel-side via the unique `(camera, frame_id, track_id)`
  index (§5.6), so retries never double-index a crop.
- **Posts NO detections — by design.** `analyze()` returns an empty detection
  list: embedding is an *indexing* task, and a second analyzer re-posting the
  same boxes through §5.3 would double-fire the zone/entry consumers (§11.2).
  Embeddings ride a separate `embeddings` list on `AnalysisResult`, which the
  task runner POSTs via `CoreClient.post_embeddings` — mirroring `post_results`:
  ≤ 128 items per batch, `frame_id` idempotency, 4xx not retried, and a `404`
  from an old kernel (no embeddings endpoint) is logged once and posting is
  disabled for the task.
- **One CLIP per process.** Model instances are shared through a locked
  process-wide singleton keyed `(clip_model, clip_pretrained, device)`, so N
  camera tasks plus the query worker (14.3) load a single copy.

Per-task `config` keys (all optional):

| Key | Default | Meaning |
|---|---|---|
| `weights` | `yolov8n.pt` | detector weights (safe-path-guarded, as everywhere) |
| `conf` | `0.35` | min box confidence to consider a crop |
| `classes` | `[1, 2, 3, 5, 7]` | COCO classes to embed — bicycle/car/motorcycle/bus/truck. **Person (0) is deliberately excluded by default** (privacy posture; person/face re-id embeddings are an explicit v1 non-goal) |
| `stride_seconds` | `10` | re-embed cadence per track |
| `static_suppression` | `true` | skip the stride refresh while the track hasn't moved (max abs bbox delta < `static_epsilon`) |
| `static_epsilon` | `0.02` | normalized-bbox movement threshold for "static" (same default as zones) |
| `static_refresh_seconds` | `3600` | slow re-embed floor for static tracks (keeps them in the index across the retention TTL) |
| `min_box_px` | `24` | skip boxes narrower/shorter than this many pixels (tiny crops embed to noise) |
| `clip_model` | `ViT-B-32-quickgelu` | open_clip architecture |
| `clip_pretrained` | `openai` | open_clip pretrained tag |
| `device` | auto | torch device |
| `thumb_max_px` | `320` | max dimension of the JPEG crop thumb; `0` disables thumbs |
| `imgsz` | model default | detector inference size |

`clip_model`/`clip_pretrained` must match the query side (14.3): the kernel only
ranks stored vectors from the query's embedding space, so mismatched checkpoints
mean empty results, not wrong ones.

### 14.2 CLIP is OPTIONAL (and degrades safely)

`open_clip` (and the torch it shares with ultralytics) is an **optional** extra —
`apps/ai/requirements-embed.txt`:

```bash
pip install -r requirements.txt -r requirements-embed.txt
```

The imports are lazy (`import torch, open_clip` in `__init__`). Without them:

- `embedding` tasks degrade to the **`PlaceholderAnalyzer`** (§8) — no vectors
  indexed, nothing fabricated;
- the query worker (14.3) answers any claimed query with
  `{"error": "clip backend unavailable"}`, warns once, and stops polling — so
  `POST /api/v1/search/semantic` returns a fast `503` ("embedding worker
  offline") instead of hanging the dashboard.

The default checkpoint (`ViT-B-32-quickgelu`/`openai`, ~350 MB) downloads on
first use. The architecture default is `ViT-B-32-quickgelu` (not plain
`ViT-B-32`): open_clip warns that the `openai` weights were trained with
QuickGELU activations, so the plain variant is a slight numerical mismatch.
Because the emitted `model` id encodes the architecture
(`open_clip/ViT-B-32-quickgelu/openai`), flipping this default changes the id —
vectors indexed under the old `ViT-B-32` default stop matching new queries.
Operators upgrading re-index for free: just let the `embedding` task's stride
repopulate, and the stale vectors age out on the retention TTL.

### 14.3 The `EmbedQueryWorker` thread — answering searches

`main` starts a second daemon thread alongside the Supervisor (§8). It polls
`GET /api/v1/ai/embed-queries?worker_id=...` (§5.7) every
`HELDAR_AI_EMBED_POLL_INTERVAL` seconds (default `1.0`,
`--embed-poll-interval`; the wait is shutdown-interruptible). The poll is fast
on purpose: the semantic-search route holds the operator's request open for
only ~3 s, which the ~10 s tasks poll could never meet (§5.6 intro).

- `kind: "text"` → CLIP **text tower** (tokenizer); `kind: "image"` →
  base64-decode → PIL → CLIP **image tower**. The vector is POSTed back via
  §5.8; **any** failure POSTs `{"error": ...}` instead, so the search fails
  fast rather than timing out.
- CLIP loads lazily on the first query, through the same shared singleton as
  14.1, using `HELDAR_AI_CLIP_MODEL` / `HELDAR_AI_CLIP_PRETRAINED` (defaults
  `ViT-B-32-quickgelu` / `openai`) for the query side.
- **Keep the checkpoints in sync.** The search prefilters stored embeddings by
  the query result's `model` id, so a task whose `clip_model`/`clip_pretrained`
  config differs from the worker's query-side envs indexes vectors that no
  query will ever match — searches then honestly return zero hits. If you
  override one, override the other; after switching checkpoints, old vectors
  age out on the detections TTL rather than polluting results.
- The first `404` means the kernel predates the embed-queries endpoint: the
  thread info-logs once and stops cleanly (old-kernel compatibility). A
  transient CLIP load failure (e.g. a checkpoint-download blip) only fails the
  queries in hand and is retried on the next claim; only a genuine
  `ImportError` (deps not installed) stops the thread for the run.

Worker-side knobs (env / CLI): `HELDAR_AI_EMBED_POLL_INTERVAL` /
`--embed-poll-interval` (1.0 s), `HELDAR_AI_CLIP_MODEL` / `--clip-model`
(`ViT-B-32-quickgelu`), `HELDAR_AI_CLIP_PRETRAINED` / `--clip-pretrained` (`openai`).

> **Deliberate v1 deferrals.** VLM interpretation over retrieved moments, ANN
> indexes (the brute-force cosine scan is measured first — it streams
> newest-first under a 100k-candidate cap and reports `truncated` honestly),
> and person/face re-id embeddings (person class excluded by default) are out
> of scope for v1. Semantic hits are **similarity-ranked candidates, not
> facts** — the search response records the ranking as a fallible inference,
> and the UI frames it the same way.
