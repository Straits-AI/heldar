# VisionOps Core — AI Worker Integration Guide (Stage 2)

This is the integration guide for **AI workers** against the VisionOps Core media
kernel. It documents the Stage 2 **frame sampler** and the **worker contract**
exactly as built in `apps/core` (`services/sampler.rs`, `routes/ai.rs`,
`models.rs`, `config.rs`, `migrations/0003_ai.sql`).

Stage 2 maps to **memo §5 Layer 4 ("Frame sampler and AI task scheduler")** and
**memo §14 "Stage 2 — AI frame sampler."** Its success criterion is:

> *"AI consumes frames without breaking recording/live view."*

Stage 2 ships the **substream sampler, the global fps budget + backpressure, the
AI-task model, and the full pull-based worker contract.** It does **not** ship
detection/tracking models — those are Stage 3. The detector slots into the
`Analyzer` seam of the reference worker (`apps/ai/worker.py`, §8).

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
24/7 compressed-segment path **decode-free** (memo §6.1); the sampler is the
*only* component that decodes, and it decodes the **sub-stream** at a budgeted
frame rate to a single JPEG per camera. Workers are pure HTTP clients: they
**discover** tasks, **pull** the latest frame on their own cadence, and **post**
detections back. A crashing, slow, or absent worker can never stall ingest or
recording — the sampler writes frames regardless of whether anyone reads them.

This is the memo §4.3 split made concrete:

```
Cameras capture.  Edge processes (kernel decodes + samples).  AI consumes normalized frames.
```

---

## 2. The fps budget and backpressure

The host has finite decode capacity, so frame sampling is governed by a **single
global frame-per-second budget** shared across every AI-enabled camera. As you
enable AI on more cameras, each camera's sample rate **degrades** rather than the
host overloading. This is the Stage 2 realization of memo §5's backpressure
policy.

The budget is computed in `SamplerManager::rebalance` (`services/sampler.rs`):

```
active          = number of enabled cameras that have ≥1 enabled AI task
budget          = VISIONOPS_AI_MAX_TOTAL_FPS  (default 40, floored at 1.0)
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
- **Master switch.** `VISIONOPS_AI_ENABLED=false` makes `rebalance` a no-op (no
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

A row in `ai_tasks` (`migrations/0003_ai.sql`, `models.rs::AiTask`) declares
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
`VISIONOPS_DEFAULT_AI_FPS` (5) and `VISIONOPS_DEFAULT_AI_WIDTH` (1280).

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

A worker needs only these four endpoints. All live under `/api/v1` and return
JSON (except the frame, which is `image/jpeg`).

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

### 5.2 Pull a frame — `GET /api/v1/cameras/{id}/frame`

Serves the latest sampled JPEG for a camera (the worker's input). The worker pulls
this on its own cadence — typically at (or just under) the task fps.

Response `200 OK`:

```
Content-Type: image/jpeg
Cache-Control: no-store
x-frame-age-ms: 142
x-frame-captured-at: 2026-06-13T08:15:31.120+00:00

<JPEG bytes>
```

- **`x-frame-age-ms`** — milliseconds since the frame file was last written
  (derived from its mtime). Use it to skip stale frames: if the sampler is
  `offline`, age climbs and the worker should not waste compute on a frozen
  frame.
- **`x-frame-captured-at`** — RFC3339 timestamp of that write; echo it back as the
  detection `timestamp` so detections align to capture time, not post time.

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

- `camera_id` (**required**) must exist, else `404`.
- `task_type` (**required**) is stored on each detection row.
- `timestamp` optional RFC3339; if omitted/unparseable the server uses `now()`.
  It applies to **all** detections in the batch.
- `detections` optional (defaults to `[]`) — send `[]` to post only an event.
  Every field inside a detection is optional except its position in the array.
- `event` optional. `event_type` is **required** when present; `severity`
  defaults to `info` (use `warning`/`critical` to trigger the Stage 1 alert
  webhook); `payload` defaults to `{}`. The event is written to the **same
  `events` table** the kernel uses, so AI alerts flow through the existing
  alert/notifier path for free.

Response `200 OK`:

```json
{ "detections_ingested": 2 }
```

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

---

## 6. The worker loop (pseudocode)

```
tasks = GET /api/v1/ai/tasks                     # refresh every few seconds
for each task (own thread / async task):
    loop at ~task.fps:
        resp = GET task.frame_url
        if resp is 404:        sleep, continue   # no frame yet
        if x-frame-captured-at == last_seen: continue   # unchanged frame; skip
        # (optionally also skip when x-frame-age-ms is too high → sampler frozen)
        dets, event = analyze(task, resp.body)
        if dets or event:
            POST /api/v1/ai/events { camera_id, task_type, timestamp,
                                     detections = dets, event = event }
```

Because `latest.jpg` is last-value, pulling faster than the sampler writes just
returns the same frame; the reference worker **dedupes on `x-frame-captured-at`**
to avoid re-analyzing an unchanged frame. Pulling slower than the sampler simply
drops intermediate frames — acceptable for detection/tracking at these rates.

---

## 7. Conventions

- **`bbox` is `[x, y, w, h]` normalized 0…1**, top-left origin (per the
  `migrations/0003_ai.sql` comment). Normalizing means detections survive any
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
  detections are reproducible/auditable (memo §8.1 `audit.model_versions`).
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
VISIONOPS_API=http://localhost:8000 python worker.py
# or: python worker.py --api http://localhost:8000 --log-format json
```

Worker-side config (CLI flag / env var): `--api`/`VISIONOPS_API`
(default `http://localhost:8000`), `--poll-interval`/`VISIONOPS_AI_POLL_INTERVAL`
(10), `--http-timeout`, `--http-max-retries`, `--backoff-base`, `--backoff-cap`,
`--log-level`, `--log-format`. Full table in `apps/ai/README.md`.

### Stage 3 adds the real models

Stage 2 deliberately stops at the **contract + sampler + reference loop + a
model-free motion analyzer.** The actual perception — **person/vehicle detection
(YOLO / RT-DETR), multi-object tracking (ByteTrack / BoT-SORT), zones, and the
canonical event model** — arrives in **Stage 3** and slots in behind the
`Analyzer` interface with no change to the kernel or the HTTP contract (see
`ROADMAP.md` Stage 3, memo §7.1–7.2 / §14). Concretely, Stage 3 adds a subclass
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

All via `VISIONOPS_*` env vars (`config.rs`):

| Var | Default | Meaning |
|---|---|---|
| `VISIONOPS_AI_ENABLED` | `true` | master switch for frame sampling; `false` runs no samplers |
| `VISIONOPS_AI_MAX_TOTAL_FPS` | `40` | global fps budget split across AI-enabled cameras (floored at 1.0) |
| `VISIONOPS_DEFAULT_AI_FPS` | `5` | default `fps` for a task that omits it (clamped 0.1…30 on write) |
| `VISIONOPS_DEFAULT_AI_WIDTH` | `1280` | default `width` for a task that omits it (clamped 160…3840) |
| `VISIONOPS_FRAMES_DIR` | `<DATA_DIR>/frames` | where `latest.jpg` per camera is written (`frames/<camera_id>/latest.jpg`) |

The **worker** side (`apps/ai/worker.py`) is configured separately: `VISIONOPS_API`
(base URL of the core, default `http://localhost:8000`),
`VISIONOPS_AI_POLL_INTERVAL`, and HTTP/backoff/logging knobs — full table in
`apps/ai/README.md`.

---

## 10. What's built vs. deferred (honest scope)

| Memo §5/§14 Stage 2 item | Status in Stage 2 | Notes |
|---|---|---|
| Substream sampler (decode only sampled frames) | ✅ | one ffmpeg per camera, `-vf fps,scale`, decode-free recording untouched |
| FPS budgeting + task scheduler | ✅ | global `VISIONOPS_AI_MAX_TOTAL_FPS` split; per-camera = `min(task fps, budget/active)`, `MIN_FPS=0.5` floor |
| Frame queue / frame-sample object | ◑ | realized as a **single `latest.jpg` per camera** (last-value), not a multi-frame queue or `frame_id` stream; staleness via `x-frame-age-ms` |
| Backpressure policy | ◑ | **static** proportional fps reduction as cameras are added (graceful fps degradation). The dynamic *resolution* ladder (720p→480p) + auto-recovery from live load is **deferred** (Stage 3+) |
| High-res snapshot on trigger | ◑ | not in the sampler; a worker can use the Stage 0 `GET /api/v1/cameras/{id}/snapshot` for a main-stream grab on trigger. Per-task `stream_profile=main` is stored/validated but the sampler currently always samples the sub-stream |
| Worker contract (discover/pull/post/query) | ✅ | full `routes/ai.rs` surface, this guide |
| Detection / tracking / zone models | ⬜ | **Stage 3** — slots into the `Analyzer` seam (§8) |

**Success criterion met:** the sampler is a separate set of supervised ffmpeg
processes decoding only the sub-stream at a bounded total fps, with crash/backoff
isolation; the recorder's 24/7 `-c copy` path and the MediaMTX live view are
completely independent of it. AI consuming frames cannot break recording or live
view (memo §14 Stage 2).

See also: [`ARCHITECTURE.md`](../ARCHITECTURE.md) §15 (implementation),
[`ROADMAP.md`](../ROADMAP.md) Stage 2 (checklist), [`docs/OBSERVABILITY.md`](OBSERVABILITY.md)
(Stage 1 metrics/alerts the AI events feed into).
