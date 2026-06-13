# VisionOps Core

**A visual event intelligence operating system for physical spaces.** VisionOps Core turns camera
streams into structured events, events into workflows, and workflows into operational intelligence —
the opposite of a camera-centric VMS. Rather than starting with AI features or wrapping an existing
DVR/NVR, we build our own **media kernel** first (camera registry, ingest, recording, playback, live
view) and treat AI perception, event intelligence, and vertical apps (campus entry, security,
retail/BakerySense) as consumers layered on top. Owning the kernel means owning the metadata model,
the event engine, and the product logic — without re-implementing codecs (we lean on FFmpeg and
MediaMTX for the low-level media work).

This repository currently implements **Stage 0 — the media kernel**.

---

## Stage 0 feature set (implemented)

The Rust control plane in `apps/core` provides:

- **Camera registry** — CRUD over cameras with per-camera recording/retention policy; RTSP URLs are
  either supplied explicitly or built from a vendor template (HikVision / Dahua).
- **RTSP ingest + recorder supervisor** — one FFmpeg process per camera, one supervised Tokio task
  each, with automatic reconnect (exponential backoff up to 30s) and live status tracking.
- **Segment recording** — `-c copy` (no decode / no re-encode), audio dropped in Stage 0, written as
  time-segmented **fragmented MP4** files.
- **Timeline index** — a background indexer `ffprobe`s new segment files and records
  start/end/duration/codec/dimensions/size; a timeline endpoint coalesces them into availability
  ranges (gaps > 2s split a range).
- **Playback** — list segment files (with browser-playable URLs) and query the coalesced timeline.
- **Clip export** — concatenate the segments overlapping a time window and trim with `-c copy`
  (keyframe-aligned precision), served as a downloadable MP4.
- **Snapshots** — JPEG frame either from recorded footage at a timestamp (`?at=`) or grabbed live.
- **Live view** — registers the camera as a [MediaMTX](https://github.com/bluenviron/mediamtx) path
  server-side (credentials never reach the browser) and returns HLS / WebRTC / RTSP playback URLs.
- **Camera health & events** — per-camera state (`recording` / `connecting` / `offline` / `error` /
  `disabled`), reconnect counts, last segment/error, plus a generic event log.
- **Retention** — per-camera age-based deletion **and** a global size cap; locked (evidence)
  segments are never deleted.

> Not yet implemented (later stages): AI frame sampler, detection/tracking/zones, ANPR, ReID,
> semantic search, and the vertical apps. See [What's next](#whats-next).

---

## Tech stack

| Layer        | Technology                                                                       |
| ------------ | -------------------------------------------------------------------------------- |
| Core / API   | Rust — [Axum](https://github.com/tokio-rs/axum) 0.8 · Tokio · [SQLx](https://github.com/launchbadge/sqlx) 0.8 |
| Database     | SQLite (default, zero-setup; embedded migrations)                                |
| Media engine | FFmpeg + ffprobe (record / clip / snapshot) · MediaMTX (live-view gateway)        |
| Frontend     | React + Vite + TypeScript (`apps/web`, scaffolded)                                |
| AI workers   | Python (planned, later stages)                                                   |

SQLite is the implemented Stage 0 store (`VISIONOPS_DATABASE_URL=sqlite://./data/visionops.db`).
The `.env.example` also lists a PostgreSQL URL as the production-style direction.

---

## Repository layout

```text
cctv/
├── apps/
│   ├── core/            # Rust media-kernel control plane (Axum + Tokio + SQLx)
│   │   ├── src/
│   │   │   ├── routes/      # HTTP handlers (cameras, recordings, playback, liveview, health, system)
│   │   │   ├── services/    # recorder, indexer, retention, health, clip, snapshot, mediamtx
│   │   │   ├── camera_url.rs # vendor RTSP URL building + credential masking
│   │   │   ├── config.rs / db.rs / models.rs / repo.rs / state.rs / util.rs
│   │   │   └── main.rs
│   │   └── migrations/      # 0001_init.sql (SQLite schema)
│   └── web/             # React + Vite + TS frontend (scaffolded: src/{components,lib,pages})
├── infra/
│   └── mediamtx/        # MediaMTX binary (fetched) + mediamtx.yml
├── scripts/             # dev.sh, setup_mediamtx.sh, synth_camera.sh, validate.sh
├── docs/                # documentation
├── data/                # runtime: SQLite db, recordings/, clips/, snapshots/ (gitignored)
├── memo.md              # product vision + build roadmap
└── research.md          # background research
```

---

## Quickstart

**Prerequisites:** Rust (via `rustup`), FFmpeg + ffprobe on `PATH`, and `curl` (for MediaMTX setup).
Node.js is needed only for the frontend.

```bash
# 0. Keep the Rust toolchain current (the project tracks latest stable)
rustup update

# 1. Build the core control plane
cargo build --manifest-path apps/core/Cargo.toml

# 2. (optional) configure — defaults work out of the box
cp .env.example .env   # edit if you want; never commit .env (holds camera credentials)

# 3. Download the MediaMTX live-view gateway binary into infra/mediamtx/
scripts/setup_mediamtx.sh

# 4. Run the dev stack (MediaMTX + the Rust core on http://localhost:8000)
scripts/dev.sh
```

In a second terminal, publish a synthetic RTSP camera so you can exercise the kernel without real
hardware or credentials:

```bash
scripts/synth_camera.sh                 # publishes rtsp://127.0.0.1:8554/cam_test (1280x720 @ 15fps)
```

Then onboard it as a camera:

```bash
curl -X POST http://localhost:8000/api/v1/cameras \
  -H 'content-type: application/json' \
  -d '{"id":"cam_test","name":"Synthetic Test Camera","vendor":"generic",
       "main_stream_url":"rtsp://127.0.0.1:8554/cam_test","segment_seconds":5}'

curl http://localhost:8000/api/v1/system                       # stats
curl http://localhost:8000/api/v1/cameras/cam_test/timeline    # recorded ranges
```

`scripts/validate.sh` runs this whole flow end-to-end (MediaMTX → synthetic camera → core →
every Stage 0 endpoint, including reconnect) and writes a report to `data/validate_report.txt`.

**Frontend** (React + Vite + TS, scaffolded in `apps/web`):

```bash
cd apps/web && npm install && npm run dev
```

The dev server defaults to `http://localhost:5173`, which is the default allowed CORS origin.

### Default ports

| Port | Service                         |
| ---- | ------------------------------- |
| 8000 | VisionOps Core HTTP API         |
| 5173 | Web frontend (Vite dev server)  |
| 8554 | MediaMTX RTSP                   |
| 8888 | MediaMTX HLS                    |
| 8889 | MediaMTX WebRTC                 |
| 9997 | MediaMTX control API            |

---

## Onboarding a real HikVision camera

For vendor cameras you don't need to know the RTSP path — supply the address and credentials and the
URL is built from the vendor template (HikVision `…/Streaming/Channels/101` for main, `102` for sub):

```bash
curl -X POST http://localhost:8000/api/v1/cameras \
  -H 'content-type: application/json' \
  -d '{
    "id": "gate_a_01",
    "name": "Gate A Camera 1",
    "vendor": "hikvision",
    "address": "192.168.0.2",
    "username": "admin",
    "password": "YOUR_PASSWORD",
    "record_stream": "main"
  }'

curl -X POST http://localhost:8000/api/v1/cameras/gate_a_01/test   # probe reachability + codec
```

> The real test cameras live at **192.168.0.2 – 192.168.0.12** and require valid credentials.
> **Do not brute-force them** — HikVision devices lock out after failed attempts. Use the actual
> credentials provided for the site.

---

## API reference

Base URL: `http://localhost:8000`. All bodies and responses are JSON unless noted.

| Method        | Path                                   | Description                                                          |
| ------------- | -------------------------------------- | ------------------------------------------------------------------- |
| `GET`         | `/healthz`                             | Liveness probe.                                                     |
| `GET`         | `/api/v1/system`                       | System info: uptime, camera/segment counts, recording footprint.   |
| `GET`         | `/api/v1/cameras`                      | List cameras.                                                       |
| `POST`        | `/api/v1/cameras`                      | Create / onboard a camera.                                          |
| `GET`         | `/api/v1/cameras/{id}`                 | Get one camera.                                                     |
| `PATCH`       | `/api/v1/cameras/{id}`                 | Partial update (re-reconciles the recorder).                        |
| `DELETE`      | `/api/v1/cameras/{id}`                 | Delete camera, stop its recorder, remove its footage.               |
| `GET`/`POST`  | `/api/v1/cameras/{id}/test`            | Probe the stream via ffprobe; returns reachability + codec/size.    |
| `GET`         | `/api/v1/cameras/{id}/segments`        | List recorded segment files (`?from&to&limit`), each with a URL.    |
| `GET`         | `/api/v1/cameras/{id}/timeline`        | Coalesced recorded ranges (`?from&to`).                             |
| `POST`        | `/api/v1/cameras/{id}/clip`            | Export an MP4 clip for a `{from,to}` window (`-c copy`).             |
| `GET`         | `/api/v1/cameras/{id}/snapshot`        | JPEG frame; `?at=<rfc3339>` for recorded, omit for live.            |
| `GET`/`POST`  | `/api/v1/cameras/{id}/liveview`        | Register MediaMTX path; returns HLS / WebRTC / RTSP URLs.           |
| `GET`         | `/api/v1/health/cameras`               | Status of all cameras.                                              |
| `GET`         | `/api/v1/cameras/{id}/health`          | Status of one camera.                                               |
| `GET`         | `/api/v1/events`                       | Event log (`?camera_id&event_type&severity&limit`).                 |
| `GET`         | `/media/recordings/{camera}/{file}`    | Static segment files.                                               |
| `GET`         | `/media/clips/{file}`                  | Static exported clips.                                              |
| `GET`         | `/media/snapshots/{file}`              | Static snapshots.                                                   |

---

## Recording model

```text
RTSP (over TCP)
   → FFmpeg (-c copy, no decode, audio dropped)
   → segment muxer → fragmented MP4 files (strftime-named: %Y%m%d_%H%M%S.mp4)
   → timeline indexer (ffprobe) → segments table
   → retention sweeper (age + size cap)
```

- **Copy codec, no decode.** The recorder remuxes compressed packets straight to disk
  (`-rtsp_transport tcp`, `-c copy -an`), so CPU stays low and the original codec/quality is
  preserved. Decoding is reserved for snapshots and (later) AI.
- **Fragmented MP4 segments.** Each segment uses
  `movflags=+frag_keyframe+empty_moov+default_base_moof` so in-progress segments remain playable.
  Segment length is per-camera `segment_seconds` (default 60s; clamped 2–3600).
- **Timeline index.** A background indexer (`VISIONOPS_INDEXER_INTERVAL_S`, default 10s) ffprobes
  closed segment files and writes one `segments` row each (path, start/end, duration, codec, w/h,
  bytes). The timeline endpoint merges contiguous segments into availability ranges.
- **Clip export** concatenates the segments overlapping the requested window and trims with
  `-c copy` — fast, lossless, keyframe-aligned (max 1h per clip).
- **Retention + size cap.** Per-camera age policy (`retention_hours`, default 24) deletes old
  segments; a global soft cap (`VISIONOPS_MAX_RECORDINGS_GB`, default 20 GB) prunes the oldest
  **unlocked** segments under disk pressure. Locked/evidence segments are never deleted.
- **Reliability.** On stream loss the supervisor logs a `camera_offline` event, bumps
  `reconnect_count`, and retries with exponential backoff (reset after a healthy run).

---

## Security notes (Stage 0)

- **Credentials are stored in plaintext** in the `cameras` table (SQLite). This is a Stage 0
  shortcut — move to a secret store / encryption-at-rest before any real deployment. Never commit
  `.env` or the `data/` directory (both are gitignored).
- **Credentials are masked** (`***@host`) in logs, error messages, and the `test` endpoint output.
- **Live view is brokered server-side** through MediaMTX, so camera credentials never reach the
  browser.
- **CORS** is configurable via `VISIONOPS_CORS_ORIGINS` (default `http://localhost:5173`); setting it
  to `*` or leaving it empty allows any origin.
- **No authentication** sits in front of the API in Stage 0, and it binds `0.0.0.0:8000` by default.
  Keep it on a trusted network and add an auth layer before exposing it.

---

## What's next

Stage 0 is the foundation. The staged plan — observability/reliability, AI frame sampler,
detection/tracking/zone kernel, campus entry app, BakerySense Vision, ReID/movement intelligence,
and semantic video search — is in **[ROADMAP.md](./ROADMAP.md)**. Full product vision and background
live in [`memo.md`](./memo.md) and [`research.md`](./research.md).
