# Heldar AI Worker (Stage 2 reference)

A small, production-grade **reference AI worker** for Heldar Core. It proves
and documents the Stage 2 worker contract end-to-end with zero heavy
dependencies (no GPU, no model). Stage 3 swaps in a real model (e.g. YOLO) by
implementing **one class** — see [Plugging in a real model](#stage-3-plugging-in-a-real-model).

The worker does **not** touch RTSP. Heldar Core samples each camera and
writes the latest frame to disk; the worker pulls those frames over HTTP and
posts results back.

## What it does

1. **Discovers work** by polling `GET {API}/api/v1/ai/tasks`, which returns one
   entry per enabled AI task on an enabled camera:
   `{ id, camera_id, task_type, stream_profile, fps, width, config, frame_url }`.
2. For each task, on a loop at the task's `fps`, it **pulls the latest frame**
   from `{API}{frame_url}` (JPEG bytes). A `404` means "no frame sampled yet"
   and is treated as a skip, not an error.
3. It runs the **analyzer** chosen by `task_type` and **POSTs results** to
   `{API}/api/v1/ai/events`.
4. It **re-polls** `/ai/tasks` every `--poll-interval` seconds to pick up new,
   changed, or removed tasks (reconciling the set of per-task threads).

### Analyzers

- **`motion`** — pure frame-differencing, no model. Decodes the JPEG to a
  downscaled grayscale array (Pillow → numpy), takes the absolute difference
  against the previous frame for that camera, and if the fraction of changed
  pixels exceeds `config.threshold` (default `0.02`) it posts:
  - a detection `{ label: "motion", confidence: <changed_fraction>,
    bbox: [x, y, w, h] }` with the bbox of the changed region normalized 0..1,
  - an event `{ event_type: "motion", severity: "info" }`.
- **`detection`** / **`yolo`** — **real** object detection + tracking via
  [Ultralytics](https://docs.ultralytics.com/) YOLOv8 (nano, `yolov8n.pt`) with
  **ByteTrack**. The `YoloAnalyzer` loads the model once and calls
  `model.track(img, persist=True, tracker="bytetrack.yaml")` on every frame, so
  boxes carry **stable track ids** across frames. Each box becomes a detection
  `{ label: <COCO class>, confidence: <box.conf>, bbox: [x, y, w, h]
  (normalized 0..1), track_id: <ByteTrack id> }`. When a person or vehicle
  class appears it also raises an `object_detected` event. Requires the
  `ultralytics` dependency (see GPU note below); if that import or the model
  load fails, the worker logs the reason and falls back to the safe placeholder
  rather than crashing.
- **anything else** — a **safe placeholder**. It pulls and decodes the frame
  (exercising the full frame-pull/heartbeat path) but emits **no detections**
  and logs, rate-limited, that a real model must be wired in. It never
  fabricates results.

### Production qualities

- Supervisor thread + one worker thread per task; clean reconcile loop.
- Graceful `SIGINT`/`SIGTERM` shutdown (all sleeps and retry backoffs are
  interruptible, so it stops promptly).
- HTTP retry with capped exponential backoff + jitter; `4xx` client errors are
  not retried.
- Structured logging (text or JSON) with per-task `camera_id`/`task_id` context.
- Config via environment variables and/or CLI flags.

## Running

Requires Python 3.10+.

```bash
cd apps/ai

# 1. Create and activate a virtualenv
python3 -m venv .venv
source .venv/bin/activate        # Windows: .venv\Scripts\activate

# 2. Install dependencies
pip install -r requirements.txt

# 3. Run (point it at a running Heldar Core)
python worker.py --api http://localhost:8000
# or rely on the env default:
HELDAR_API=http://localhost:8000 python worker.py
```

Stop with `Ctrl-C` — it drains and exits cleanly.

### Docker

```bash
docker build -t heldar-ai-worker apps/ai
docker run --rm -e HELDAR_API=http://host.docker.internal:8000 heldar-ai-worker
```

## Configuration

Every flag has an environment-variable equivalent. CLI flags override env vars,
which override the built-in defaults.

| Flag | Env var | Default | Meaning |
|------|---------|---------|---------|
| `--api` | `HELDAR_API` | `http://localhost:8000` | Heldar Core base URL |
| `--poll-interval` | `HELDAR_AI_POLL_INTERVAL` | `10` | Seconds between `/ai/tasks` re-polls |
| `--http-timeout` | `HELDAR_HTTP_TIMEOUT` | `10` | Per-request timeout (s) |
| `--http-max-retries` | `HELDAR_HTTP_MAX_RETRIES` | `5` | Retries for transient HTTP failures |
| `--backoff-base` | `HELDAR_HTTP_BACKOFF_BASE` | `0.5` | Initial backoff (s) |
| `--backoff-cap` | `HELDAR_HTTP_BACKOFF_CAP` | `15` | Max backoff (s) |
| `--log-level` | `HELDAR_LOG_LEVEL` | `INFO` | `DEBUG`/`INFO`/`WARNING`/`ERROR` |
| `--log-format` | `HELDAR_LOG_FORMAT` | `text` | `text` or `json` |

### Per-task `config` (from the task's `config` JSON)

The `motion` analyzer reads these keys (all optional):

| Key | Default | Meaning |
|-----|---------|---------|
| `threshold` | `0.02` | Min fraction of changed pixels to fire |
| `pixel_delta` | `25` | Per-pixel grayscale delta counted as "changed" |
| `scale_width` | `320` | Width the frame is downscaled to before diffing |

The `detection`/`yolo` analyzer (`YoloAnalyzer`) reads these keys (all optional):

| Key | Default | Meaning |
|-----|---------|---------|
| `weights` | `yolov8n.pt` | Ultralytics weights file/name (keep nano for speed) |
| `threshold` | `0.25` | Minimum box confidence to keep |
| `classes` | _(all)_ | Allowlist of class names and/or COCO indices to detect |
| `imgsz` | model default | Inference image size |
| `device` | `auto` | Force a device (`"cpu"`, `0`, …); `auto` = GPU if CUDA else CPU |
| `emit_events` | `true` | Emit an `object_detected` event for person/vehicle classes |
| `alert_classes` | person + vehicles | Class names that trigger the alert event |

The placeholder analyzer reads `log_interval_s` (default `60`) to rate-limit its
"no real model" warning.

## Stage 3: the real model (YOLOv8 + ByteTrack)

Stage 3 is **already wired in**: the `YoloAnalyzer` class in `worker.py` is
registered for the `detection` (and `yolo`) task types. The polling, threading,
frame-pull, retry, and ingest plumbing from Stage 2 are unchanged — the model is
the *only* new piece.

How it works:

- On construction (once per task thread) it loads `YOLO("yolov8n.pt")` and picks
  a device automatically: GPU when `torch.cuda.is_available()`, else CPU.
- On every frame it runs
  `model.track(img, persist=True, tracker="bytetrack.yaml", verbose=False)`.
  `persist=True` keeps ByteTrack state across calls, so each box has a **stable
  `track_id`**. A model is created **per camera/task** so track ids never collide
  between cameras.
- Each box maps to a Heldar detection: `label` from `model.names`,
  `confidence` from `box.conf`, `bbox` = `[x, y, w, h]` **normalized to 0..1** by
  the frame width/height, and `track_id = str(int(box.id))` when present. These
  are POSTed to `/api/v1/ai/events` with the task's `camera_id` + `task_type`.
- When a person or vehicle class is detected it also raises an `object_detected`
  event (`severity: warning` for people, `info` otherwise) so Core can correlate
  it with zone rules (e.g. the "Restricted-Right" zone) and surface alerts.

Install the model dependency (large — pulls torch/torchvision/opencv):

```bash
cd apps/ai
source .venv/bin/activate
pip install ultralytics            # or: pip install -r requirements.txt
```

### GPU note

YOLO runs on **CPU by default and that is fine** for the reference worker —
yolov8n is small enough for real-time CPU inference at the sampled frame rate.
If `torch` detects a compatible CUDA GPU it is used automatically; otherwise the
analyzer logs `device=cpu` and proceeds. To pin a device, set `device` in the
task `config` (e.g. `"device": "cpu"` or `"device": 0`). Note that a GPU build
of torch still falls back to CPU if the installed NVIDIA driver is older than the
CUDA toolkit the wheel was compiled against.

### Adding another model

The extension point is the `Analyzer` base class. Subclass it, implement
`analyze(frame) -> AnalysisResult`, and register it for a task type:

```python
from worker import Analyzer, AnalysisResult, Detection, FrameContext, register

class MyAnalyzer(Analyzer):
    name = "my-model"
    def analyze(self, frame: FrameContext) -> AnalysisResult:
        ...
        return AnalysisResult(detections=[...])

register("my_task_type", MyAnalyzer)
```

Key contract for any `Analyzer`:

- One instance is created **per task thread**, so per-camera state (a previous
  frame, a tracker) can live on the instance.
- `analyze(frame)` is called on the task's cadence and must be reasonably fast.
- `bbox` is always `[x, y, w, h]` **normalized to 0..1**.
- **Never fabricate detections** — return an empty `AnalysisResult()` when
  there's nothing to report.
```
