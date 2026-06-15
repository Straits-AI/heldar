---
id: quickstart
title: Quickstart
sidebar_label: Quickstart
sidebar_position: 1
---

# Quickstart

Bring up Heldar locally, onboard a real camera, and run the reference AI worker.

## Prerequisites

- **Rust** (via `rustup`) - the project tracks latest stable.
- **FFmpeg + ffprobe** on `PATH` - record, clip, snapshot, and frame sampling
  all shell out to them. The server does a media-binary preflight at boot and
  fails fast if they are missing.
- **curl** for the API calls below.
- **Node.js** for the React dashboard (`apps/web`).
- **Python 3** for the AI worker (`apps/ai`).

## Build and run

```bash
rustup update
cargo build --workspace
cp .env.example .env                 # defaults work out of the box; never commit .env
scripts/setup_mediamtx.sh            # fetch the MediaMTX live-view gateway
scripts/run_stack.sh                 # MediaMTX + core (http://localhost:8000) + Vite dashboard
```

`scripts/run_stack.sh` starts three processes: the MediaMTX live-view gateway,
the Heldar Core server on `http://localhost:8000`, and the Vite dev server for
the dashboard on `http://localhost:5173`.

### Two ways to view the dashboard

- **Single binary (one URL).** Build the dashboard and point the server at it:

  ```bash
  cd apps/web && npm install && npm run build      # writes apps/web/dist
  ```

  Set `HELDAR_WEB_DIR=./apps/web/dist` in `.env`. The core then serves the SPA
  at `http://localhost:8000` alongside the API. The `/api/*`, `/media/*`,
  `/healthz`, `/readyz`, and `/metrics` routes keep precedence; everything else
  falls back to the SPA so client-routed deep links work. See
  [Deploy](./deploy.md).
- **Vite dev server (hot reload).** `scripts/run_stack.sh` runs `npm run dev`
  and serves the dashboard at `http://localhost:5173`, talking to the API on
  `:8000`. Use this while developing the frontend.

## Add a camera

The RTSP URL is built from the vendor template, so you only supply the address
and credentials:

```bash
curl -X POST http://localhost:8000/api/v1/cameras -H 'content-type: application/json' -d '{
  "id":"gate_a","name":"Gate A","vendor":"hikvision",
  "address":"192.168.0.2","username":"admin","password":"YOUR_PASSWORD"}'

curl http://localhost:8000/api/v1/system                     # uptime, camera/segment counts
curl http://localhost:8000/api/v1/cameras/gate_a/timeline    # recorded ranges
```

The recorder spawns one decode-free FFmpeg process per recordable camera and
starts writing segments. The indexer turns closed segment files into timeline
rows a few seconds later.

> Do not brute-force camera credentials. HikVision devices lock out after failed
> attempts.

## Run the AI worker

Perception runs in a separate worker process that pulls sampled frames over
HTTP. First enable a detection task on the camera (via the dashboard, or with
the API):

```bash
curl -X POST http://localhost:8000/api/v1/cameras/gate_a/ai-tasks \
  -H 'content-type: application/json' \
  -d '{"task_type":"detection","fps":5,"width":1280,"enabled":true}'
```

Enabling the first task starts a frame sampler for that camera. Then run the
reference worker:

```bash
cd apps/ai
python3 -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
HELDAR_API=http://localhost:8000 python worker.py
```

The worker discovers tasks, pulls each camera's latest frame, runs its analyzer,
and posts detections back to `POST /api/v1/ai/events`. See
[Build an AI worker](../develop/ai-worker.md) for the full contract. To validate
the whole sampler to worker to events path with no model and no GPU, create a
task with `task_type: "motion"` instead - the reference worker ships a working
frame-differencing analyzer for it.

## Configure detection, zones, and alerting in the UI

With a camera and an AI task running, use the dashboard to:

- **Detection** - create or tune AI tasks per camera (`task_type`, requested
  `fps`, sample `width`, and a free-form `config` blob the worker reads).
- **Zones** - draw polygon regions on a camera. Coordinates are normalized
  0..1, matching detection boxes. Set `labels` to filter which detections count,
  `dwell_seconds` to arm a dwell alert, and a `severity`. Tracked detections
  crossing a zone raise `enter` / `exit` / `dwell` zone events with an evidence
  frame.
- **Alerting** - point the alert notifier at a webhook (`HELDAR_ALERT_WEBHOOK_URL`
  or the UI). `warning` and `critical` events, including zone events and
  worker-posted events, are delivered to it.

## Next

- [Deploy](./deploy.md) for the single-binary production layout.
- [Architecture](../concepts/architecture.md) for how the kernel and apps fit
  together.
