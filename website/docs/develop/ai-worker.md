---
id: ai-worker
title: Build an AI Worker
sidebar_label: Build an AI Worker
sidebar_position: 2
---

# Build an AI worker

A Heldar AI worker is any process that can speak four HTTP endpoints. The kernel
samples frames and stores results; your worker is a pure HTTP client that
discovers work, pulls frames, runs a model, and posts detections back. Because
the contract is HTTP, a crashing or slow worker can never stall ingest or
recording.

The reference implementation is
[`apps/ai/worker.py`](https://github.com/Straits-AI/heldar/blob/main/apps/ai/worker.py)
(a small, dependency-light Python worker). The full integration guide, including
the zone and ANPR analyzers, is
[docs/AI-WORKERS.md](https://github.com/Straits-AI/heldar/blob/main/docs/AI-WORKERS.md).

## The contract

All endpoints live under `/api/v1` and return JSON, except the frame, which is
`image/jpeg`.

### 1. Discover work - `GET /api/v1/ai/tasks`

Returns every enabled task on an enabled camera, each carrying the `frame_url`
to pull. This is your whole work list. Re-poll every few seconds to pick up newly
enabled or disabled tasks.

```json
[
  {
    "id": "ai_3f2a9c1b...",
    "camera_id": "gate_a",
    "task_type": "detection",
    "stream_profile": "sub",
    "fps": 5.0,
    "width": 1280,
    "config": { "classes": ["person", "car"], "min_confidence": 0.4 },
    "frame_url": "/api/v1/cameras/gate_a/frame"
  }
]
```

`task_type` is a free-form string you define; the kernel uses `fps` / `width` /
`enabled` only to drive the sampler, and `config` is an opaque blob your worker
interprets. The `fps` here is the requested rate; the effective sampled rate
after budgeting is reported by `GET /api/v1/ai/samplers`.

### 2. Pull a frame - `GET /api/v1/cameras/{id}/frame`

Serves the latest sampled JPEG for a camera. Pull it on your own cadence,
typically at or just under the task `fps`.

```
200 OK
Content-Type: image/jpeg
Cache-Control: no-store
x-frame-age-ms: 142
x-frame-captured-at: 2026-06-13T08:15:31.120+00:00

<JPEG bytes>
```

- `x-frame-age-ms` - milliseconds since the frame was written. Use it to skip
  stale frames if the sampler has gone offline.
- `x-frame-captured-at` - the write timestamp. Dedupe on it so you do not
  re-analyze an unchanged frame, and optionally echo it back as the detection
  `timestamp` so detections align to capture time.

A `404` means no frame exists yet (no enabled AI task for the camera, or the
sampler has not produced its first frame). Treat it as a skipped cycle, not an
error.

### 3. Post results - `POST /api/v1/ai/events`

Post a batch of detections for one camera and task, optionally with a single
derived event in the same call.

```json
{
  "camera_id": "gate_a",
  "task_type": "detection",
  "timestamp": "2026-06-13T08:15:31.120Z",
  "detections": [
    {
      "label": "person",
      "confidence": 0.92,
      "bbox": [0.41, 0.30, 0.08, 0.22],
      "track_id": "t-17",
      "attributes": { "zone": "entry_lane_a" }
    }
  ],
  "event": {
    "event_type": "person_in_red_zone",
    "severity": "warning",
    "payload": { "zone": "red_a", "track_id": "t-17" }
  }
}
```

Field rules:

- `camera_id` is required and must exist, else `404`.
- `task_type` is required and is stored on each detection row.
- `timestamp` is optional RFC3339 and applies to the whole batch; omitted or
  unparseable falls back to server `now()`.
- `detections` is optional (defaults to `[]`); send `[]` to post only an event.
  Every field inside a detection is optional.
- `event` is optional. `event_type` is required when present; `severity` defaults
  to `info` (use `warning` or `critical` to trigger the alert webhook); `payload`
  defaults to an empty object.

The response is `{ "detections_ingested": N }`.

### 4. Sampler status - `GET /api/v1/ai/samplers`

Per-camera sampler state (`connecting` / `sampling` / `offline` / `error` /
`stopped`) and the effective budgeted fps. Useful for dashboards and for
confirming the kernel is actually producing frames.

## The bbox convention

`bbox` is `[x, y, w, h]` **normalized to 0..1**, top-left origin. Normalizing
keeps detections resolution-independent, so they survive any later change to the
sampled `width` and map directly onto normalized zone polygons. The kernel stores
the box as raw JSON and does not validate its shape, so your worker owns
correctness.

A detection with both a `track_id` and a `bbox` drives the kernel zone engine
(its ground point is the box bottom-center); detections without them are still
stored but cannot raise zone events.

## The worker loop

```text
tasks = GET /api/v1/ai/tasks                 # refresh every few seconds
for each task (own thread / async task):
    loop at ~task.fps:
        resp = GET task.frame_url
        if resp is 404:        sleep, continue          # no frame yet
        if x-frame-captured-at == last_seen: continue   # unchanged frame; skip
        dets, event = analyze(task, resp.body)
        if dets or event:
            POST /api/v1/ai/events { camera_id, task_type, timestamp,
                                     detections: dets, event: event }
```

Because the served frame is last-value, pulling faster than the sampler writes
returns the same bytes; dedupe on `x-frame-captured-at`. Pulling slower simply
drops intermediate frames, which is fine for detection and tracking at these
rates.

## Plugging a model in

The reference worker defines an `Analyzer` base class and creates one instance
per task thread, so per-camera state (a previous frame, a tracker) lives on
`self`. Analyzers register by `task_type`; an unknown type falls back to a
placeholder that exercises the frame path but never fabricates detections. A
working, model-free `MotionAnalyzer` is registered for `task_type: "motion"`, so
you can validate the whole sampler to worker to events path with no model and no
GPU.

A real detector slots in as one subclass and one `register(...)` call, with no
change to the kernel or the HTTP contract:

```python
from worker import Analyzer, AnalysisResult, Detection, FrameContext, register

class YoloAnalyzer(Analyzer):
    name = "yolo"
    def __init__(self, config, log):
        super().__init__(config, log)
        from ultralytics import YOLO
        self.model = YOLO(config.get("weights", "yolov8n.pt"))
        self.conf = float(config.get("threshold", 0.25))

    def analyze(self, frame: FrameContext) -> AnalysisResult:
        img = frame.image(); w, h = img.size
        dets = []
        for r in self.model(img, conf=self.conf, verbose=False):
            for b in r.boxes:
                x1, y1, x2, y2 = b.xyxy[0].tolist()
                dets.append(Detection(
                    label=self.model.names[int(b.cls)],
                    confidence=float(b.conf),
                    bbox=[x1/w, y1/h, (x2-x1)/w, (y2-y1)/h]))  # normalized 0..1
        return AnalysisResult(detections=dets)

register("detection", YoloAnalyzer)   # replaces the placeholder for "detection"
```

The kernel never touches your model. It only routes results to consumers by
`task_type`: `detection` results with track ids drive the zone engine, `anpr`
results feed the access-control engine, and so on. To add a new pipeline, pick a
new `task_type`, post its results, and write a consumer for it (see
[Build a module](./build-a-module.md)).

## Running the reference worker

```bash
cd apps/ai
python3 -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
HELDAR_API=http://localhost:8000 python worker.py
# or: python worker.py --api http://localhost:8000 --log-format json
```

Worker-side config (CLI flag or env var) covers the API base URL
(`--api` / `HELDAR_API`), the task poll interval
(`--poll-interval` / `HELDAR_AI_POLL_INTERVAL`), and HTTP/backoff/logging knobs.
The full table is in
[apps/ai/README.md](https://github.com/Straits-AI/heldar/blob/main/apps/ai/README.md).
