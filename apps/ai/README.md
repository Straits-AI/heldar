# VisionOps AI Worker (Stage 2 reference)

A small, production-grade **reference AI worker** for VisionOps Core. It proves
and documents the Stage 2 worker contract end-to-end with zero heavy
dependencies (no GPU, no model). Stage 3 swaps in a real model (e.g. YOLO) by
implementing **one class** — see [Plugging in a real model](#stage-3-plugging-in-a-real-model).

The worker does **not** touch RTSP. VisionOps Core samples each camera and
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
- **anything else** (e.g. `detection`) — a **safe placeholder**. It pulls and
  decodes the frame (exercising the full frame-pull/heartbeat path) but emits
  **no detections** and logs, rate-limited, that a real model must be wired in.
  It never fabricates results.

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

# 3. Run (point it at a running VisionOps Core)
python worker.py --api http://localhost:8000
# or rely on the env default:
VISIONOPS_API=http://localhost:8000 python worker.py
```

Stop with `Ctrl-C` — it drains and exits cleanly.

### Docker

```bash
docker build -t visionops-ai-worker apps/ai
docker run --rm -e VISIONOPS_API=http://host.docker.internal:8000 visionops-ai-worker
```

## Configuration

Every flag has an environment-variable equivalent. CLI flags override env vars,
which override the built-in defaults.

| Flag | Env var | Default | Meaning |
|------|---------|---------|---------|
| `--api` | `VISIONOPS_API` | `http://localhost:8000` | VisionOps Core base URL |
| `--poll-interval` | `VISIONOPS_AI_POLL_INTERVAL` | `10` | Seconds between `/ai/tasks` re-polls |
| `--http-timeout` | `VISIONOPS_HTTP_TIMEOUT` | `10` | Per-request timeout (s) |
| `--http-max-retries` | `VISIONOPS_HTTP_MAX_RETRIES` | `5` | Retries for transient HTTP failures |
| `--backoff-base` | `VISIONOPS_HTTP_BACKOFF_BASE` | `0.5` | Initial backoff (s) |
| `--backoff-cap` | `VISIONOPS_HTTP_BACKOFF_CAP` | `15` | Max backoff (s) |
| `--log-level` | `VISIONOPS_LOG_LEVEL` | `INFO` | `DEBUG`/`INFO`/`WARNING`/`ERROR` |
| `--log-format` | `VISIONOPS_LOG_FORMAT` | `text` | `text` or `json` |

### Per-task `config` (from the task's `config` JSON)

The `motion` analyzer reads these keys (all optional):

| Key | Default | Meaning |
|-----|---------|---------|
| `threshold` | `0.02` | Min fraction of changed pixels to fire |
| `pixel_delta` | `25` | Per-pixel grayscale delta counted as "changed" |
| `scale_width` | `320` | Width the frame is downscaled to before diffing |

The placeholder analyzer reads `log_interval_s` (default `60`) to rate-limit its
"no real model" warning.

## Stage 3: plugging in a real model

The extension point is the `Analyzer` base class in `worker.py`. Adding a real
model is a self-contained change — the polling, threading, frame-pull, retry,
and ingest plumbing all stay the same.

```python
# in worker.py (or a sidecar module that imports worker)
from worker import Analyzer, AnalysisResult, Detection, FrameContext, register

class YoloAnalyzer(Analyzer):
    name = "yolo"

    def __init__(self, config, log):
        super().__init__(config, log)
        import ultralytics
        self.model = ultralytics.YOLO(config.get("weights", "yolov8n.pt"))
        self.conf = float(config.get("threshold", 0.25))

    def analyze(self, frame: FrameContext) -> AnalysisResult:
        img = frame.image()                 # PIL.Image
        w, h = img.size
        results = self.model(img, conf=self.conf, verbose=False)
        dets = []
        for r in results:
            for b in r.boxes:
                x1, y1, x2, y2 = b.xyxy[0].tolist()
                dets.append(Detection(
                    label=self.model.names[int(b.cls)],
                    confidence=float(b.conf),
                    bbox=[x1 / w, y1 / h, (x2 - x1) / w, (y2 - y1) / h],  # normalized
                ))
        return AnalysisResult(detections=dets)

# Map the task_type that should use it (replaces the placeholder for "detection"):
register("detection", YoloAnalyzer)
```

Key contract for any `Analyzer`:

- One instance is created **per task thread**, so per-camera state (a previous
  frame, a tracker) can live on the instance.
- `analyze(frame)` is called on the task's cadence and must be reasonably fast.
- `bbox` is always `[x, y, w, h]` **normalized to 0..1**.
- **Never fabricate detections** — return an empty `AnalysisResult()` when
  there's nothing to report.
```
