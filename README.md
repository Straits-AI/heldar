# Heldar Core

Heldar is a visual event-intelligence operating system for physical spaces. It turns camera streams
into structured events, events into workflows, and workflows into operational intelligence. Rather
than wrapping an existing DVR/NVR or starting from AI features, it builds its own **media kernel**
first (camera registry, RTSP ingest, recording, playback, live view), then layers perception, an
event engine, and apps on top as *consumers*. Owning the kernel means owning the metadata model, the
event engine, and the product logic, without re-implementing codecs (FFmpeg and MediaMTX do the
low-level media work).

The platform is **open-core**: an Apache-2.0 kernel plus generic reference apps, with vertical and
client products as separate proprietary crates. See [LICENSING.md](./LICENSING.md).

## Documentation

Full documentation lives at **https://straits-ai.github.io/heldar/**. It covers the quickstart,
deployment, the architecture and its public seams, the open-core boundary, and the guides for
building your own app or AI worker against the kernel.

In-repo references: [ARCHITECTURE.md](./ARCHITECTURE.md) (the kernel seams and every stage's design),
[ROADMAP.md](./ROADMAP.md) (stage status), [LICENSING.md](./LICENSING.md) (the open-core boundary),
and the operator/integrator guides in [`docs/`](./docs).

## Quickstart

**Prerequisites:** Rust (via `rustup`), FFmpeg + ffprobe on `PATH`, `curl`. Node.js for the
dashboard; Python 3 for the AI worker.

```bash
rustup update                        # the project tracks latest stable
cargo build --workspace
cp .env.example .env                 # defaults work out of the box; never commit .env
scripts/setup_mediamtx.sh            # fetch the MediaMTX live-view gateway
scripts/run_stack.sh                 # MediaMTX + core (http://localhost:8000) + web (Vite on :5173)
```

The core serves the built dashboard at `http://localhost:8000` when `HELDAR_WEB_DIR` points at
`apps/web/dist` (one binary, one URL). `scripts/run_stack.sh` also runs the Vite dev server at
`http://localhost:5173` for frontend work.

Onboard a camera (you supply the address and credentials; the RTSP URL is built from the vendor
template):

```bash
curl -X POST http://localhost:8000/api/v1/cameras -H 'content-type: application/json' -d '{
  "id":"gate_a","name":"Gate A","vendor":"hikvision",
  "address":"192.168.0.2","username":"admin","password":"YOUR_PASSWORD"}'

curl http://localhost:8000/api/v1/system                     # uptime, camera/segment counts
curl http://localhost:8000/api/v1/cameras/gate_a/timeline    # recorded ranges
```

> Do not brute-force camera credentials. HikVision devices lock out after failed attempts.

Run the reference AI worker against an AI-enabled camera:

```bash
cd apps/ai && python3 -m venv .venv && .venv/bin/pip install -r requirements.txt
HELDAR_API=http://localhost:8000 .venv/bin/python worker.py
```

See the [Quickstart](https://straits-ai.github.io/heldar/docs/getting-started/quickstart) for
enabling detection tasks, drawing zones, and configuring alerting.

### Default ports

| Port | Service |
| --- | --- |
| 8000 | Heldar Core HTTP API + dashboard |
| 5173 | Web dashboard (Vite dev server) |
| 8554 / 8888 / 8889 | MediaMTX RTSP / HLS / WebRTC |
| 9997 | MediaMTX control API (loopback) |
