# Heldar Core

**A visual event-intelligence operating system for physical spaces.** Heldar turns camera streams
into structured events, events into workflows, and workflows into operational intelligence — the
opposite of a camera-centric VMS. Rather than starting from AI features or wrapping an existing
DVR/NVR, it builds its own **media kernel** first (camera registry, RTSP ingest, recording, playback,
live view), then layers perception, an event engine, and vertical apps on top as *consumers*. Owning
the kernel means owning the metadata model, the event engine, and the product logic — without
re-implementing codecs (FFmpeg + MediaMTX do the low-level media work).

The platform is **open-core**: an Apache-2.0 kernel + generic reference apps, with vertical/client
products as separate proprietary crates. See [LICENSING.md](./LICENSING.md).

---

## Status

Roadmap **Stages 0–7 are shipped** (see [ROADMAP.md](./ROADMAP.md)):

| Stage | Capability |
| --- | --- |
| 0 | Media kernel — registry, RTSP ingest, segment recording (`-c copy`), timeline, playback, clip, snapshot, live view, camera health |
| 1 | Observability & reliability — health/metrics/events APIs, alert webhook, recording-gap tracking |
| 2 | AI frame sampler — bounded sub-stream frame extraction feeding a pluggable worker, isolated from recording |
| 3 | Detection / tracking / zones — `DetectionConsumer` ingest, zone enter/exit/dwell engine |
| 4 | Access Control / access control — ANPR temporal-voting plate resolution, vehicle/visitor/watchlist registry, guard workflow, RBAC |
| 5 | BakerySense — anonymous retail behaviour analytics (footfall / queue / dwell / occupancy) |
| 6 | Movement intelligence — multi-signal cross-camera ReID *candidates* (human-reviewed) + red-zone breach alerts |
| 7 | Semantic search — deterministic query over event facts + LLM-as-*planner* (offline rule parser default) + a proof/claim ladder |

The full perception pipeline has been **validated end-to-end on real HikVision cameras**
(`RTSP → sampler → YOLO+ByteTrack worker → detections → zone events → breach alerts`). Per-stage
*accuracy* benchmarking (precision/recall) is still pending labelled ground truth.

---

## Architecture (open-core)

```text
                    ┌─────────────────────────────────────────────┐
   cameras ──RTSP──▶│  heldar-kernel  (Apache-2.0)             │
                    │  media/DVR · perception ingest + sampler ·   │
                    │  zone engine · auth/RBAC · observability ·   │
                    │  retention · remote-access overlay status ·  │
                    │  the DetectionConsumer + worker SDK seams     │
                    └───────────────┬──────────────┬──────────────┘
   AI worker ──/ai/events──▶ (perception)          │ composed by heldar-server
   (apps/ai, YOLO)                                 │
                    ┌──────────────────────────────┴──────────────┐
   OPEN generic apps (Apache-2.0)        PROPRIETARY verticals     │
   heldar-entry  (access control)     heldar-bakery          │
   heldar-movement (ReID/breach)      heldar-campus-* (future)│
   heldar-search (semantic search)                              │
                    └───────────────────────────────────────────────┘
```

Apps plug into the kernel only through public seams — the `DetectionConsumer` trait, `Router<AppState>`
merging, a self-installed schema, and the auth primitive — so the kernel has **no** dependency on any
app. A deployment is *composed* from the kernel + whichever apps a client needs (single-tenant per
deployment). See [ARCHITECTURE.md](./ARCHITECTURE.md).

---

## Tech stack

| Layer | Technology |
| --- | --- |
| Core / API | Rust — [Axum](https://github.com/tokio-rs/axum) 0.8 · Tokio · [SQLx](https://github.com/launchbadge/sqlx) 0.8 |
| Database | SQLite (default, zero-setup, WAL, embedded migrations) |
| Media engine | FFmpeg + ffprobe (record / clip / snapshot / sample) · [MediaMTX](https://github.com/bluenviron/mediamtx) (live-view gateway) |
| AI worker | Python — Ultralytics YOLO + ByteTrack (`apps/ai`); optional ANPR OCR backend (paddleocr/easyocr) |
| Frontend | React + Vite + TypeScript (`apps/web`) |
| Remote access | WireGuard overlay (Tailscale / NetBird), external daemon — see [docs/REMOTE-ACCESS.md](./docs/REMOTE-ACCESS.md) |

---

## Repository layout

```text
cctv/
├── crates/
│   ├── heldar-kernel/     # Apache-2.0 platform (media/DVR, perception, zones, auth, seams)
│   ├── heldar-entry/      # Apache-2.0 generic access control (ANPR, registry, guard workflow)
│   ├── heldar-movement/   # Apache-2.0 generic cross-camera ReID + breach engine
│   ├── heldar-search/     # Apache-2.0 generic semantic search (plan → execute → proof)
│   ├── heldar-bakery/     # PROPRIETARY retail-analytics vertical
│   └── heldar-server/     # composing binary `heldar-core` (verticals behind a Cargo feature)
├── apps/
│   ├── ai/                   # Python reference AI worker (YOLO/ByteTrack)
│   └── web/                  # React + Vite + TS dashboard
├── infra/mediamtx/           # MediaMTX binary (fetched) + mediamtx.yml
├── scripts/                  # run_stack.sh, setup_mediamtx.sh, validate_*.sh, prepare-open-repo.sh
├── docs/                     # per-stage guides + REMOTE-ACCESS + OPEN-CORE-SPLIT
├── ARCHITECTURE.md · ROADMAP.md · LICENSING.md   # top-level docs
└── data/                     # runtime: SQLite db, recordings/clips/snapshots/frames (gitignored)
```

---

## Quickstart

**Prerequisites:** Rust (via `rustup`), FFmpeg + ffprobe on `PATH`, `curl`. Node.js for the frontend;
Python 3 for the AI worker.

```bash
rustup update                        # the project tracks latest stable
cargo build --workspace
cp .env.example .env                 # defaults work out of the box; never commit .env
scripts/setup_mediamtx.sh            # fetch the MediaMTX live-view gateway
scripts/run_stack.sh                 # MediaMTX + core (http://localhost:8000) + web (Vite)
```

Onboard a real camera (the RTSP URL is built from the vendor template — you only supply address +
credentials):

```bash
curl -X POST http://localhost:8000/api/v1/cameras -H 'content-type: application/json' -d '{
  "id":"gate_a","name":"Gate A","vendor":"hikvision",
  "address":"192.168.0.2","username":"admin","password":"YOUR_PASSWORD"}'

curl http://localhost:8000/api/v1/system                     # uptime, camera/segment counts, remote_access
curl http://localhost:8000/api/v1/cameras/gate_a/timeline    # recorded ranges
```

> **Do not brute-force camera credentials** — HikVision devices lock out after failed attempts.

Run the AI worker (detection) against onboarded cameras:

```bash
# enable a detection task on a camera, then:
cd apps/ai && python -m venv .venv && .venv/bin/pip install -r requirements.txt
HELDAR_API=http://localhost:8000 .venv/bin/python worker.py
```

Per-stage validation scripts (`scripts/validate_*.sh`) exercise each stage end-to-end against a
running stack and write reports to `data/`.

### Default ports

| Port | Service |
| --- | --- |
| 8000 | Heldar Core HTTP API |
| 5173 | Web frontend (Vite dev server) |
| 8554 / 8888 / 8889 | MediaMTX RTSP / HLS / WebRTC |
| 9997 | MediaMTX control API (loopback) |

---

## Security & auth

- **Auth/RBAC is opt-in** (`HELDAR_AUTH_ENABLED`, default false = open LAN appliance). When enabled,
  the API requires a session (login) or `X-API-Key`, and enforces five roles
  (`admin` / `manager` / `guard` / `viewer` / `integration`) across capabilities; every mutation is
  written to an immutable audit log.
- **Sessions use an HttpOnly, SameSite=Strict cookie** (not JS-readable → not XSS-exfiltratable); the
  same-origin SPA sends it automatically, including to the media plane.
- **Camera credentials** are masked (`***@host`) in logs/errors, and live view is brokered server-side
  through MediaMTX so credentials never reach the browser. Credentials are stored in the DB; use
  disk/secret encryption at rest for sensitive deployments.
- **Remote access** for sites behind CGNAT is a WireGuard overlay (P2P-first, end-to-end encrypted, no
  proxy sees the video) — see [docs/REMOTE-ACCESS.md](./docs/REMOTE-ACCESS.md).

The codebase has been through adversarial production-readiness audits across the Rust crates, the
Python worker, and the frontend (correctness, concurrency, resource lifecycle, security, ops), with
the confirmed findings fixed.

---

## Documentation

- [ARCHITECTURE.md](./ARCHITECTURE.md) — the kernel seams + every stage's design
- [ROADMAP.md](./ROADMAP.md) — stage status and the research frontier
- [CONTRIBUTING.md](./CONTRIBUTING.md) — dev setup, the quality bar, and what belongs in this repo
- [LICENSING.md](./LICENSING.md) — the open-core boundary
- Operator / integrator guides in [`docs/`](./docs):
  - [Access Control](./docs/ACCESS-CONTROL.md) (ANPR / entry) · [Movement](./docs/MOVEMENT.md) (ReID + breach) · [Search](./docs/SEARCH.md)
  - [AI workers](./docs/AI-WORKERS.md) · [Observability](./docs/OBSERVABILITY.md) · [Remote access](./docs/REMOTE-ACCESS.md)
  - [Commissioning checklist](./docs/commissioning-checklist.md) · [Sizing](./docs/sizing.md)
  - [Open-core split + publishing](./docs/OPEN-CORE-SPLIT.md)
