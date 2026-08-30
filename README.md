**English** · [简体中文](README.zh-Hans.md) · [Español](README.es.md)

# Heldar Core

Heldar is a visual event-intelligence operating system for physical spaces. It turns camera streams
into structured events, events into workflows, and workflows into operational intelligence. Rather
than wrapping an existing DVR/NVR or starting from AI features, it builds its own **media kernel**
first (camera registry, RTSP ingest, recording, playback, live view), then layers perception, an
event engine, and apps on top as *consumers*. Owning the kernel means owning the metadata model, the
event engine, and the product logic, without re-implementing codecs (FFmpeg and MediaMTX do the
low-level media work — the kernel spawns and supervises every ffmpeg itself: recording, AI sampling,
and the live-preview transcode, so no piece of the pipeline depends on another component's exec
environment).

The platform is **open-core**: an Apache-2.0 kernel plus generic reference apps, with vertical and
client products as separate proprietary crates. See [LICENSING.md](./LICENSING.md).

## Documentation

Full documentation lives at **https://heldar.swmengappdev.workers.dev/docs/**. It covers the quickstart,
deployment, the architecture and its public seams, the open-core boundary, and the guides for
building your own app or AI worker against the kernel.

In-repo references: [ARCHITECTURE.md](./ARCHITECTURE.md) (the kernel seams and every stage's design),
[DESIGN-PRINCIPLES.md](./docs/DESIGN-PRINCIPLES.md) (the values behind the design),
[ROADMAP.md](./ROADMAP.md) (stage status), [LICENSING.md](./LICENSING.md) (the open-core boundary),
and the operator/integrator guides in [`docs/`](./docs).

## Quickstart

**Fastest — Docker (pull & run):**

```bash
curl -fsSL https://heldar.swmengappdev.workers.dev/install.sh | sh
# already have the repo? just:  docker compose -f deploy/compose.yml up -d
```

Pulls the prebuilt **OPEN** images (kernel + generic apps) and starts MediaMTX + core + web — the
dashboard is then at `http://localhost:8080`. Add the reference AI worker with `--profile ai`; update
with `docker compose pull`. For production layer three overlays — they harden different things and are
separate on purpose:

- `compose.prod.yml` — the **application** posture: auth on, secure cookies, strict boot guardrails.
- `compose.hardened.yml` — the **container**: read-only root filesystems, all capabilities dropped,
  `no-new-privileges`, CPU/memory/PID ceilings, bounded logs. See `docs/PRODUCTION.md`.
- `compose.tls.yml` — terminates HTTPS.

```bash
docker compose -f deploy/compose.yml -f deploy/compose.prod.yml \
  -f deploy/compose.hardened.yml -f deploy/compose.tls.yml up -d
```

The hardening overlay marks the session cookie `Secure`, so it expects HTTPS in front of it — the TLS
overlay ([`deploy/TLS.md`](deploy/TLS.md)) supplies that with Caddy, in either automatic-Let's-Encrypt
mode for a public domain or self-signed mode for a LAN appliance. See
[`docs/PRODUCTION.md`](docs/PRODUCTION.md) for the full deployment-mode ladder (DEV → COMMISSIONING →
PRODUCTION-LAN → PRODUCTION-REMOTE) and [`docs/SUPPLY-CHAIN.md`](docs/SUPPLY-CHAIN.md) for the image
pinning policy — production deployments should set `HELDAR_VERSION` to a pinned release tag rather than
tracking `latest`. For a flashed DVR/appliance, use the native-systemd image instead
(`make appliance-image`, [`infra/systemd/`](infra/systemd/)).

**Build from source:**

**Prerequisites:** Rust (via `rustup`), FFmpeg + ffprobe on `PATH`, `curl`. Node.js for the
dashboard; Python 3 for the AI worker.

```bash
rustup update                        # the project tracks latest stable
cargo build --workspace
cp .env.example .env                 # defaults work out of the box; never commit .env
(cd apps/web && npm ci)              # dashboard dependencies
scripts/setup_mediamtx.sh            # fetch the MediaMTX live-view gateway
scripts/run_stack.sh                 # MediaMTX + core (http://localhost:8000) + web (Vite on :5173)
```

The core serves the built dashboard at `http://localhost:8000` when `HELDAR_WEB_DIR` points at
`apps/web/dist` (one binary, one URL). `scripts/run_stack.sh` also runs the Vite dev server at
`http://localhost:5173` for frontend work.

**Remote access** (from any network, no app, even behind CGNAT): the box dials OUT to a WebRTC
rendezvous and the full dashboard runs in a browser — live multi-camera, recorded playback, and config —
with a two-gate auth model where the kernel stays the sole RBAC authority. Opt-in + design:
[`docs/REMOTE-ACCESS.md`](docs/REMOTE-ACCESS.md); hardening for the public internet (auth, TLS, secrets,
lockout, credential encryption, Turnstile): [`docs/PRODUCTION.md`](docs/PRODUCTION.md).

**Camera device features** (day/night, lighting, relay outputs, PTZ) are dashboard-controlled via a
per-camera capability probe, ANPR barrier cameras can feed their **on-board plate recognition**
straight into the entry pipeline, and a matched entry can **open the barrier** by pulsing the lane
camera's relay (per-lane policy + guard manual-open + global kill-switch):
[`docs/CAMERA-CONTROLS.md`](docs/CAMERA-CONTROLS.md).

**Semantic search** finds stored footage by meaning: type a description ("red pickup truck") or drop
in a photo and get similarity-ranked detection crops with jump-to-playback. Ranking runs on CLIP
embeddings computed by the AI worker (optional `requirements-embed.txt` extra) — fully local, no
cloud: [`docs/SEARCH.md`](docs/SEARCH.md).

**Signed evidence bundles** make an export defensible after it leaves the box. A bundle carries the
clip, the events and detections in the window, the audit trail, the source segment hashes and the
recording gaps — under one Ed25519 signature, verifiable offline with `python3` and `openssl` alone:
[`docs/EVIDENCE.md`](docs/EVIDENCE.md). It states what it cannot prove, too: the appliance stamps its
own clock, so the signature attests to origin and integrity, never to time.

Onboard a camera (you supply the address and credentials; the RTSP URL is built from the vendor
template):

```bash
curl -X POST http://localhost:8000/api/v1/cameras -H 'content-type: application/json' -d '{
  "id":"gate_a","name":"Gate A","vendor":"hikvision",
  "address":"192.168.0.2","username":"admin","password":"YOUR_PASSWORD"}'

curl http://localhost:8000/api/v1/system                     # uptime, camera/segment counts
curl http://localhost:8000/api/v1/cameras/gate_a/timeline    # recorded ranges
curl http://localhost:8000/api/v1/system/retention           # recording size cap + free-disk floor
curl http://localhost:8000/api/v1/system/timezone            # the site clock schedules and search use
```

> Do not brute-force camera credentials. HikVision devices lock out after failed attempts.

> **Bounded recordings.** The retention sweeper keeps recordings from filling the disk: a size cap
> (`HELDAR_MAX_RECORDINGS_GB`, default 20) and a free-disk floor (`HELDAR_MIN_FREE_DISK_GB`, default 5),
> evicting oldest-first (evidence-locked clips are never evicted). Both are tunable at runtime via
> `GET`/`PUT /api/v1/system/retention` (PUT admin-only) and the dashboard's System page — no restart.
> The metadata DB is also bounded (`HELDAR_MAX_DB_GB`, default 4), and a pre-existing DB self-converts to
> `auto_vacuum=INCREMENTAL` in the background on first upgrade (`HELDAR_DB_AUTOVACUUM_CONVERT=true`).

> **Which clock is yours.** Timestamps are stored in UTC. *Interpretation* — a schedule's "18:00",
> a search's "after 6pm", "yesterday" — follows the site's timezone once one is set
> (`GET`/`PUT /api/v1/system/timezone`, admin-only). With none set, schedules follow the server's
> local zone and search follows UTC, exactly as before, so nothing moves until you choose. Set one
> and both move together. See [#125](https://github.com/Straits-AI/heldar/issues/125).

Run the reference AI worker against an AI-enabled camera:

```bash
cd apps/ai && python3 -m venv .venv && .venv/bin/pip install -r requirements.txt
HELDAR_API=http://localhost:8000 .venv/bin/python worker.py
```

See the [Quickstart](https://heldar.swmengappdev.workers.dev/docs/getting-started/quickstart) for
enabling detection tasks, drawing zones, and configuring alerting.

> **AI ingest provenance.** Detections posted over `/api/v1/ai/events` are always recorded as
> `source: "worker"` — the kernel writes that attribute itself and strips whatever the client sent, so
> no API caller can claim to be a camera's on-board ANPR engine (a read the barrier treats as
> authoritative). Workers additionally **lease** their tasks and post a server-issued **frame ticket**,
> which lets the kernel derive `camera_id`/`task_type`/`frame_id` instead of trusting the body.
> Requiring the ticket is staged: `HELDAR_INGEST_PROVENANCE` defaults to `warn` (ticketless ingest
> still works, with a once-per-hour notice per credential) and is **never promoted automatically** —
> not even by `HELDAR_DEPLOYMENT_MODE=production*`, because unlike `HELDAR_MACHINE_AUTH` it is a
> CLIENT protocol change: `enforce` 401s every worker that does not yet mint a ticket. Set it
> explicitly after the hourly `ingest_unleased` log goes quiet.
> See [ADR 0005](docs/adr/0005-task-lease-bound-ai-ingest.md)
> and [`docs/AI-WORKERS.md`](docs/AI-WORKERS.md) §5.0.

### Default ports

| Port | Service |
| --- | --- |
| 8000 | Heldar Core HTTP API + dashboard |
| 8080 | Dashboard (Docker deploy, nginx → core :8000) |
| 5173 | Web dashboard (Vite dev server) |
| 8554 / 8888 / 8889 | MediaMTX RTSP / HLS / WebRTC (WHEP signaling) |
| 8189/udp | MediaMTX WebRTC (ICE media) |
| 9997 | MediaMTX control API (loopback) |
