# Heldar Core — Stage 0 Media Kernel Architecture

This document describes the Stage 0 "media kernel" of Heldar Core **as actually
built** in `crates/heldar-kernel` (Rust / Axum / Tokio / SQLx), not as aspirationally planned.
It is the base VMS/NVR control plane: camera registry, RTSP ingest, segment
recording, timeline index, playback / clip / snapshot, brokered live view, and
camera health. The detection/tracking **models** are a later stage (Stage 3) and
are intentionally absent here.

**Stage 1 (observability & reliability) has since shipped** on top of this kernel —
storage/disk monitoring, Prometheus metrics, an alert webhook, a disk-free
retention floor, recording-gap reporting, observed fps/bitrate, a `/readyz`
readiness probe, and supervised background tasks. It is documented in §14 below and,
for operators, in [`docs/OBSERVABILITY.md`](docs/OBSERVABILITY.md).

**Stage 2 (AI frame sampler) has also shipped** — a budgeted sub-stream frame
sampler (the only component that decodes in the 24/7 path), an `ai_tasks` /
`detections` data model, and a pull-based **worker contract** (discover tasks →
pull the latest sampled frame → post detections/events). AI workers never touch
RTSP. It is documented in §15 below and, for integrators, in
[`docs/AI-WORKERS.md`](docs/AI-WORKERS.md); the reference worker lives in `apps/ai`.

**Stage 3 (detection / tracking / zone kernel) has also shipped** — frames become
**events**. The AI worker now runs a real detector + tracker (YOLO + ByteTrack
behind the Stage 2 `Analyzer` seam) and posts **tracked** detections (`track_id`
per object); the kernel gains a **zone engine** that evaluates those tracked
detections against per-camera polygon zones and raises `enter` / `exit` / `dwell`
events with an evidence frame. New schema: `zones` + `zone_events`
(`migrations/0004_zones.sql`). It is documented in §16 below and, for integrators,
in [`docs/AI-WORKERS.md`](docs/AI-WORKERS.md). The detection/tracking *engineering*
is production-grade; model *accuracy* on local footage (Malaysian vehicles/plates,
crowded ReID) still needs a local benchmark set before any hard decision is made on
it.

**Stage 4 (Access Control app) has also shipped** — the first vertical on the kernel.
It adds an **RBAC layer** (users / sessions / API keys, five roles, a `Principal`
extractor gated by `HELDAR_AUTH_ENABLED`), an entry **registry** (registered
vehicles, visitor passes, watchlist), and an **ANPR temporal-voting engine**
(`services/anpr.rs`) that consolidates per-frame plate reads from an `anpr` worker
task into one canonical entry/exit event, resolves it against the
registry, and drives a guard confirm/reject **workflow** + daily/exception/audit
**reports**. New schema: `migrations/0005_entry.sql`. It is documented in §17 below
and, for operators/integrators, in [`docs/ACCESS-CONTROL.md`](docs/ACCESS-CONTROL.md).
The ANPR/attribute *engineering* is production-grade; OCR + make/model *accuracy*
needs the same local benchmark before any hard access decision.

**Stage 5 (BakerySense app) has also shipped** — a second vertical, **same kernel,
different ontology**. It is a **proprietary retail behaviour-analytics vertical**
(`heldar-bakery`) that lives in a separate private repo; only its open-core boundary is
described here. It is **anonymous by construction**: it reads only the kernel's
anonymous perception data (zone events plus ephemeral person tracks) and rolls them into
behaviour metrics and a periodic report, never touching identity. Unlike the ANPR engine
it is **not** a `DetectionConsumer` on the ingest hot path — it is a periodic **rollup
loop + report generator** reading already-stored kernel tables, composed (not welded)
into the server with its own schema/config/loop/retention/routes; the kernel is unaware
of it. It is documented at the boundary level in §18 below.

**Stage 6 (Movement intelligence app) has also shipped** — cross-camera correlation on
the **same kernel data**, under strict privacy gates. It is the client's "Movement
intelligence" (Phase 2) app (`crates/heldar-movement`): **multi-signal ReID, never a
pure visual embedding.** Vehicle ReID is anchored on the **plate** (already resolved by
Access Control into `entry_events`), fused with transit-time plausibility + colour/type
agreement over an operator-defined **camera-topology graph**; person ReID has no plate and
no appearance embedding, so it is offered only as a **low-confidence, on-demand**
topology+time search. Every cross-camera link is a scored **candidate** a human confirms
or rejects (**not** legal identity), and every identity-like **search is audited**. It
also runs a **red-zone breach** rule engine that turns restricted-zone entries into worked
incidents with track→plate subject correlation. Like BakerySense it is **not** a
`DetectionConsumer` — it is a **correlation layer** of two `spawn_supervised` loops (a ReID
candidate proposer + a breach engine) plus an on-demand search surface, reading
already-stored kernel/Entry tables, composed (not welded) with its own schema/config/loops/
retention/routes; the kernel is unaware of it. It is documented in §19 below and, for
operators/integrators, in [`docs/MOVEMENT.md`](docs/MOVEMENT.md).

**Stage 7 (Semantic search) has also shipped** — the platform's accumulated event facts
become a queryable **visual-event memory**, under one governing principle: **the LLM is a
query PLANNER, never the source of truth.** A natural-language question is translated into
a structured **query plan**, the plan is executed deterministically against the kernel's
stored facts (`entry_events`, `zone_events`, `breach_alerts`), and the **answer is those
rows** — never model output. When no LLM endpoint is configured (the default) a transparent
**rule parser** produces the same plan, so search works **fully offline**; when one is
configured it **only** plans (and falls back to the rules on any failure). Every answer is
wrapped in a **proof layer** (a claim ladder), with the NL→plan reading
surfaced as the *single* fallible inference. Like BakerySense/Movement it is **not** a
`DetectionConsumer` — and unlike them it is not even a background loop: it is a **read-only
query layer over kernel facts** (three HTTP routes + one small query log), composed (not
welded) with its own schema/config/routes. New code lives in `crates/heldar-search`. It
is documented in §20 below and, for operators/integrators, in
[`docs/SEARCH.md`](docs/SEARCH.md). Open-vocabulary VLM enrichment + event/clip embeddings +
vector retrieval (search-by-image) remain a documented future seam — they need an
embedding/VLM worker; this stage ships the deterministic structured + NL-plan + proof core.

With Stage 7, **all roadmap stages 0–7 are now shipped** (see [`ROADMAP.md`](ROADMAP.md)).

Stage 0 covers the media kernel MVP (camera registry, RTSP
ingest, recording segmenter, timeline index, playback API, clip export, basic live
view, camera health) and is built on a layered model. The recording
philosophy follows a stream and codec strategy (described in detail
below).

---

## 1. Layered architecture

The crate is organized as a thin HTTP control plane (Axum routes) over a set of
long-running background services, all sharing one SQLite store and one `Config`.
The layers below follow the layer model (Layer 0–3); Layer 4 (AI frame
sampler) is deliberately out of scope for Stage 0.

```
                          HTTP clients (React/Vite UI, curl, tools)
                                        │
                      ┌─────────────────┴──────────────────┐
                      │            Axum router              │   src/routes/*
                      │  /api/v1/...  +  /media/* (ServeDir)│
                      └─────────────────┬──────────────────┘
                                        │  AppState { pool, cfg, recorder, http }
   ┌────────────────────────────────────┼─────────────────────────────────────────┐
   │                                     │                                          │
   ▼                                     ▼                                          ▼
┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐
│ L0 Device    │  │ L1 Ingest +  │  │ L2 Recording │  │ L2 Timeline  │  │ L3 Playback /    │
│ registry     │  │ supervisor   │  │ (segments on │  │ index        │  │ live view        │
│              │  │              │  │  disk)       │  │              │  │                  │
│ cameras tbl  │  │ recorder.rs  │  │ FFmpeg -c    │  │ indexer.rs   │  │ playback.rs      │
│ routes/      │  │ 1 FFmpeg per │  │ copy →       │  │ scan→ffprobe │  │ clip.rs          │
│ cameras.rs   │  │ camera       │  │ frag-MP4     │  │ →segments tbl│  │ snapshot.rs      │
│              │  │              │  │ files        │  │ +gap detect  │  │ mediamtx.rs(live)│
└──────┬───────┘  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘  └────────┬─────────┘
       │                 │                 │                 │                   │
       │            ┌────┴───────┐         │            ┌────┴───────┐           │
       │            │ Health     │         │            │ Retention  │           │
       │            │ monitor    │         │            │ sweeper    │           │
       │            │ health.rs  │         │            │ retention.rs│          │
       │            │ staleness  │         │            │ age + size │           │
       │            │ downgrade  │         │            │ cap, locks │           │
       │            └────┬───────┘         │            └────┬───────┘           │
       └─────────────────┴─────────────────┴─────────────────┴───────────────────┘
                                        │
                              ┌─────────▼──────────┐
                              │  SQLite (WAL)      │   db.rs + migrations/0001_init.sql
                              │  cameras, segments,│
                              │  camera_status,    │
                              │  events, sites,    │
                              │  tenants           │
                              └────────────────────┘

   External processes (never linked in-proc): ffmpeg, ffprobe (spawned per task),
   MediaMTX (HTTP control API at :9997; HLS :8888 / WebRTC :8889 / RTSP :8554).
```

| Layer | Stage 0 implementation | Files |
|---|---|---|
| **Layer 0 — Device registry** | `cameras` table + CRUD + `test` probe; vendor-templated RTSP URL builder; credential masking | `routes/cameras.rs`, `models.rs`, `camera_url.rs` |
| **Layer 1 — Stream ingestion** | Per-camera FFmpeg supervisor: RTSP pull, reconnect w/ backoff, status & bitrate metrics, reconnect/offline events | `services/recorder.rs`, `repo.rs` |
| **Layer 2 — Recording engine** | `-c copy` fragmented-MP4 segment writer (on disk) + timeline indexer + retention manager + gap detector + evidence lock | `services/recorder.rs`, `services/indexer.rs`, `services/retention.rs` |
| **Layer 2 — Storage monitor** | Global byte cap + `/api/v1/system` footprint reporting | `services/retention.rs`, `routes/system.rs` |
| **Layer 3 — Playback** | Segment listing, coalesced timeline ranges, clip export, snapshot extraction | `routes/playback.rs`, `routes/recordings.rs`, `services/clip.rs`, `services/snapshot.rs` |
| **Layer 3 — Live view (brokered)** | MediaMTX path registration + HLS/WebRTC/RTSP URL minting (server-side creds) | `services/mediamtx.rs`, `routes/liveview.rs` |
| **Camera health** | Staleness downgrade monitor + status & event APIs | `services/health.rs`, `routes/health.rs` |
| **Layer 4 — AI frame sampler** | **Shipped in Stage 2** (§15): budgeted sub-stream sampler → `frames/<cam>/latest.jpg`; `ai_tasks`/`detections` tables; pull-based worker contract. Detection/tracking *models* are Stage 3. | `services/sampler.rs`, `routes/ai.rs`, `migrations/0003_ai.sql`, `apps/ai/worker.py` |

### Process boot order (`main.rs`)

1. Load `.env` (dotenvy), init tracing (`HELDAR_LOG`, default `info,heldar_core=debug`).
2. Build `Config::from_env`; `create_dir_all` for data/recordings/clips/snapshots dirs.
3. Open SQLite pool (`db::init_pool`), run embedded migrations (`db::run_migrations`).
4. Construct `RecorderManager` and shared `reqwest::Client` (10s timeout).
5. `recorder.start_all()` — spawn one supervisor task per recordable camera.
6. Launch the indexer, health monitor, retention sweeper, and alert notifier as
   **supervised** background loops (`spawn_supervised` — respawn on return/panic, §14).
7. Build the Axum router: API routes + three `ServeDir` mounts (`/media/recordings`,
   `/media/clips`, `/media/snapshots`) + `TraceLayer` + CORS.
8. Bind `api_host:api_port` (default `0.0.0.0:8000`) and serve with graceful shutdown
   on SIGINT/SIGTERM, which calls `recorder.shutdown()` to stop every FFmpeg child.

CORS allows all origins when `HELDAR_CORS_ORIGINS` is empty or contains `*`,
otherwise restricts to the configured list (default `http://localhost:5173`).

---

## 2. Data model (`migrations/0001_init.sql`)

SQLite. Timestamps are RFC3339 UTC `TEXT`, booleans are `INTEGER` 0/1, JSON is
`TEXT`. Six tables:

```
 tenants ─1:N─ sites ─1:N─ cameras ─1:N─ segments        (timeline index)
                              │  1:1 ─── camera_status    (live state, upserted)
                              │  1:N ─── events           (lifecycle log; camera_id nullable)
```

### `tenants`, `sites` — multi-tenant scaffolding
Present for forward-compatibility but unused by Stage 0 logic. `sites` carries a
`timezone` (default `'UTC'`). `cameras.site_id` → `sites(id) ON DELETE SET NULL`;
`sites.tenant_id` → `tenants(id) ON DELETE CASCADE`.

### `cameras` — device registry (Layer 0)
| Column | Notes |
|---|---|
| `id` | PK, slug (e.g. `gate_a_01`); auto-derived from `name` via `slugify` if not given |
| `site_id` | nullable FK to `sites` |
| `name` | required |
| `vendor` | `hikvision\|dahua\|onvif\|generic` (default `generic`); drives RTSP path template |
| `model`, `address` | host/ip in `address` |
| `rtsp_port` | default 554 |
| `username`, `password` | **plaintext in Stage 0** (schema comment: "move to secret store later") |
| `main_stream_url`, `sub_stream_url` | explicit RTSP overrides; else built from vendor template |
| `record_stream` | `main\|sub` (default `main`) — which stream the recorder pulls |
| `codec`, `resolution_main/sub`, `fps_main/sub` | descriptive metadata |
| `capabilities` | JSON (default `{}`) — ptz/onvif/anpr_native etc. (forward-shaped for AI) |
| `record_enabled`, `enabled` | both must be 1 for recording (`Camera::should_record`) |
| `segment_seconds` | default 60; clamped 2..3600 on write |
| `retention_hours` | default 24; min 1 on write |
| `created_at`, `updated_at` | |

### `segments` — timeline index (Layer 2)
One row per **closed** recorded file on disk.
| Column | Notes |
|---|---|
| `id` | PK, `seg_<uuid-simple>` |
| `camera_id` | FK → `cameras` `ON DELETE CASCADE` |
| `path` | **UNIQUE** absolute file path (idempotency key for the indexer) |
| `start_time` | parsed from the strftime filename |
| `end_time` | `start_time + ffprobe duration` |
| `duration_s` | REAL, from ffprobe |
| `codec`, `width`, `height` | from ffprobe video stream |
| `size_bytes` | file size |
| `container` | always `'mp4'` in Stage 0 |
| `locked` | 0/1 — **evidence lock**; locked segments are never retention-deleted |
| `incident_id` | optional evidence association (column exists; no API sets it yet) |
| `created_at` | |

Indexes: `idx_segments_cam_time (camera_id, start_time)`, `idx_segments_end (end_time)`.

### `camera_status` — live recorder state (Layer 1, single row/camera, upserted)
| Column | Notes |
|---|---|
| `camera_id` | PK, FK → `cameras` CASCADE |
| `state` | `disabled\|connecting\|recording\|offline\|error\|unknown` |
| `last_segment_at` | set by the indexer when a new segment lands |
| `last_started_at` | set when the FFmpeg process (re)starts |
| `reconnect_count` | incremented on each FFmpeg exit/reconnect |
| `segments_written` | incremented per indexed segment |
| `fps_observed` | column exists; not populated in Stage 0 |
| `bitrate_kbps` | computed by the indexer (`size*8 / duration / 1000`) |
| `last_error` | last error tail (truncated to ~800 chars) |
| `recorder_pid` | OS pid of the live FFmpeg child (cleared on exit) |
| `updated_at` | |

### `events` — lifecycle log (forward-shaped for AI events)
`id`, nullable `camera_id`/`site_id`, `event_type`, `severity` (`info|warning|critical`),
`timestamp`, `payload` (JSON), `created_at`. Emitted types in Stage 0:
`camera_offline`, `recorder_error` (incl. stale-stream downgrade), `recording_gap`,
`retention_delete`, `disk_pressure`. Indexes on `timestamp` and `(camera_id, timestamp)`.

### Storage engine settings (`db.rs`)
SQLite opened with `create_if_missing`, **WAL journal**, `synchronous=NORMAL`,
`busy_timeout=15s`, `foreign_keys=ON`; pool of `max_connections=8`,
`acquire_timeout=20s`. The pool **rejects any non-`sqlite` URL** with an explicit
"Stage 0 supports sqlite only; Postgres is planned via SQLx" error.

---

## 3. Recorder supervisor (`services/recorder.rs`)

The heart of Layer 1+2. `RecorderManager` owns a `Mutex<HashMap<camera_id,
CameraTask>>`; each `CameraTask` holds a `watch::Sender<bool>` stop channel and the
supervisor `JoinHandle`.

### One FFmpeg `-c copy` process per camera (no decode)
For each recordable camera, `supervise()` spawns FFmpeg with:

```
ffmpeg -nostdin -hide_banner -loglevel warning
       -rtsp_transport tcp
       -rw_timeout 15000000          # 15s I/O timeout → exit on stall
       -i <rtsp record_url>
       -c copy -an                   # copy video bitstream; DROP audio in Stage 0
       -f segment
       -segment_time <segment_seconds, min 2>
       -segment_format mp4
       -segment_format_options movflags=+frag_keyframe+empty_moov+default_base_moof
       -reset_timestamps 1
       -strftime 1
       <recordings_dir>/<camera_id>/%Y%m%d_%H%M%S.mp4
```

Key properties:
- **Recording without decode** — `-c copy` passes the compressed H.264/H.265
  bitstream straight to disk; no decode, no re-encode. This is the recording-without-decode
  rule made concrete (see §8 below).
- **Fragmented MP4** — `movflags=+frag_keyframe+empty_moov+default_base_moof` makes
  each segment a fragmented MP4 so a partially-written, mid-rotation file is still
  a valid, seekable, browser-playable container.
- **UTC strftime filenames** — the child is spawned with `TZ=UTC` in its environment
  and `-strftime 1`, so segment filenames (`%Y%m%d_%H%M%S.mp4`) are UTC wall-clock,
  giving the indexer a timezone-free key (`util::parse_segment_time`).
- **Audio dropped** (`-an`) in Stage 0.
- `stdin` is null, `stdout` null, **`stderr` is piped and drained concurrently** by a
  spawned task (`read_to_end`) so the pipe never back-pressures FFmpeg; the tail is
  stored as `last_error` on exit.
- `kill_on_drop(true)` guarantees the OS process dies if the supervising Tokio task
  is dropped/panics — no orphaned FFmpeg processes.

### Supervision loop & reconnect with exponential backoff
```
                ┌──────────────────── supervise(camera_id) ────────────────────┐
                │ backoff = 1s                                                  │
                ▼                                                               │
        stop set? ──yes──► return                                              │
                │no                                                            │
        load camera from DB                                                    │
          ├─ deleted (None) ─────────────► return                             │
          ├─ !should_record ─► set_state "disabled" ─► return                 │
          └─ no record_url ─► set_state "error" + recorder_error event        │
                              ─► sleep_or_stop(30s) ─► loop                    │
                │                                                              │
        set_state "connecting"                                                │
        spawn ffmpeg (kill_on_drop, TZ=UTC)                                    │
          ├─ spawn err ─► set_state "error" ─► sleep_or_stop(15s) ─► loop      │
          └─ ok ─► set_running "recording" + pid                              │
                                                                               │
        tokio::select! {                                                       │
          child.wait()  => bump_reconnect + camera_offline event;             │
                           backoff = if ran>30 {1} else {min(backoff*2,30)};   │
                           sleep_or_stop(backoff) ─► loop ───────────────────► │
          stop.changed() => child.kill(); set_state "offline"; return         │
        }                                                                      │
                └───────────────────────────────────────────────────────────┘
```

- **Backoff**: starts at 1s; on each FFmpeg exit it doubles up to a 30s cap, **but
  resets to 1s if the process ran healthily for >30s** (`backoff = if ran > 30 { 1 }
  else { (backoff*2).min(30) }`). This avoids hammering a dead camera while
  recovering instantly from transient blips.
- **Watch-channel stop**: every sleep and the main `select!` listen on the
  `watch::Receiver<bool>`; `stop()` sends `true`, then joins the task with an 8s
  timeout (logs a warning if it overruns). `sleep_or_stop` checks the flag both
  before and during each backoff sleep.
- **Status transitions** are persisted via `repo.rs` upserts: `connecting` →
  `recording` (with pid) → on exit `offline` (reconnect bumped, pid cleared) →
  back to `connecting`. The indexer later flips state to `recording` again when a
  fresh segment is observed.

### Lifecycle management
- `start_all()` queries `WHERE enabled=1 AND record_enabled=1` and spawns each; it is
  a no-op (with a warning) when `HELDAR_RECORDER_ENABLED=false`.
- `reconcile(id)` is called by the camera CRUD handlers after create/update: it
  stops any existing task, reloads the row, and (re)spawns only if `should_record()`,
  else marks `disabled`. This keeps recorders consistent with registry edits.
- `stop(id)` is called on delete (which also `remove_dir_all`s the camera's
  recordings) and `shutdown()` on process exit.

---

## 4. Timeline indexer (`services/indexer.rs`)

A periodic loop (`HELDAR_INDEXER_INTERVAL_S`, default 10s, min 2s) that turns
closed segment files into `segments` rows. For each camera's recordings dir:

1. **List** `*.mp4` files, sort by name (≈ chronological, thanks to UTC strftime).
2. **Settle-time gate** — skip any file modified within the last `SETTLE_SECS = 5`
   seconds; that file is assumed to be the one FFmpeg is currently writing.
3. **Idempotency** — skip if a row with that `path` already exists (the UNIQUE
   constraint backs this).
4. **Parse start time** from the filename (`%Y%m%d_%H%M%S`); unparseable names are
   warned-and-skipped.
5. **ffprobe** the file (`format=duration`, `stream=codec_type,codec_name,width,height`).
   Probe failures are debug-logged and retried on the next pass (file may still be
   flushing). Files with `duration_s <= 0.05` or `size == 0` are treated as
   empty/just-rotated stubs and skipped.
6. **Insert** the segment: `end_time = start + duration`, plus codec/width/height/
   size; `locked=0`, `container='mp4'`.
7. **Update status** via `record_segment_indexed` — bumps `segments_written`, sets
   `last_segment_at = end`, sets `bitrate_kbps = size*8 / duration / 1000`, and
   (re)asserts `state='recording'`.
8. **Gap detection** — compares the new segment's `start` to the previous segment's
   `end_time` (max over the camera); a gap **> 3s** emits a `recording_gap` warning
   event with `gap_seconds`, `prev_end`, `next_start`.

This decoupling (recorder writes files; indexer reads them after they settle) means
the recorder never blocks on probing and the DB only ever references complete files.

---

## 5. Health monitor (`services/health.rs`)

A loop (`HELDAR_HEALTH_INTERVAL_S`, default 15s, min 5s) catching the
**stalled-but-connected** failure mode that the recorder's process-exit logic cannot
see — FFmpeg is alive and `state='recording'` but no new segments are landing.

For every camera whose status is `recording`, it joins `camera_status` to `cameras`
and computes a staleness threshold:

```
threshold = max( segment_seconds.max(10) * 3 , 30 )   # seconds
```

If **neither** `last_segment_at` **nor** `last_started_at` is within `threshold`
seconds of now, the camera is downgraded: `state='error'` with message
`"no segments for >Ns while recording"`, plus a `recorder_error` warning event
(`reason: "stale"`). Including `last_started_at` in the grace window prevents a
false downgrade in the window after a (re)start but before the first segment closes
and is indexed.

---

## 6. Retention sweeper (`services/retention.rs`)

A loop (`HELDAR_RETENTION_INTERVAL_S`, default 300s, min 30s) enforcing two
policies, in order. **Locked (evidence) segments are never deleted by either.**

1. **Age policy, per camera** — for each camera, delete segments with
   `locked = 0 AND end_time < now - retention_hours`. Each delete removes the file
   (`remove_file`) then the row. A summary `retention_delete` info event is logged if
   anything was removed.
2. **Global size cap** — read `SUM(size_bytes)` across all segments; while it exceeds
   `HELDAR_MAX_RECORDINGS_GB` (default 20 GB, stored as `max_recordings_bytes`),
   delete the oldest unlocked segments in batches of 20 (`ORDER BY end_time ASC`),
   re-checking the total each iteration. If the only remaining segments are locked,
   the loop breaks (the cap can be exceeded by locked evidence — by design). A
   `disk_pressure` warning event is logged if anything was pruned.

The **evidence lock** (`segments.locked`) is the evidence-lock mechanism:
the column and the retention guards exist, though no Stage 0 API mutates `locked`
or `incident_id` yet (that arrives with the `lockEvidence` endpoint in a later
stage).

---

## 7. Playback, clip, snapshot, and live view (Layer 3)

### Segment listing & timeline (`routes/recordings.rs`)
- `GET /api/v1/cameras/{id}/segments?from&to&limit` — overlap query
  (`start_time < to AND end_time > from`), each row decorated with a browser-playable
  `url` (`/media/recordings/{camera_id}/{file}` served by `ServeDir`). Without a
  range it returns the most recent `limit` (default 500, ≤5000) ascending.
- `GET /api/v1/cameras/{id}/timeline?from&to` — coalesces contiguous segments into
  availability ranges, merging across gaps ≤ `GAP_TOLERANCE_S = 2`, and reports
  `recorded_seconds` and `segment_count`. This is the data a scrub-bar UI renders.

### Clip export (`services/clip.rs`)
`POST /api/v1/cameras/{id}/clip {from,to}`:
1. Validate `to > from` and `requested ≤ MAX_CLIP_SECONDS = 3600`.
2. Select segments overlapping `[from,to)` ordered by start.
3. Write an FFmpeg `concat` list file referencing each segment path (single-quotes
   escaped), then run **`-f concat -i list -ss <offset> -t <dur> -c copy
   -avoid_negative_ts make_zero -movflags +faststart`**. No re-encode.
4. Output to `clips_dir/clip_<uuid>.mp4`, return `{id,url:/media/clips/..,size_bytes,
   segment_count,...}`; the temp list file is removed, and a failed export cleans up
   its partial output.

Because the cut is `-c copy`, the trim is **keyframe-aligned**: the actual clip
boundaries snap to the nearest keyframe at/after the requested `from` offset, so
start precision is bounded by the GOP length (Stage 0 limitation; frame-accurate
cuts need re-encode and arrive later).

### Snapshot (`services/snapshot.rs`)
`GET /api/v1/cameras/{id}/snapshot[?at=RFC3339]`:
- **With `at`** — find the segment covering that instant, compute the in-segment
  offset, and `ffmpeg -ss <offset> -i seg -frames:v 1 -q:v 3 -c:v mjpeg pipe:1`.
- **Without `at`** — grab a frame live from the camera (**sub-stream preferred**,
  falling back to the record URL), wrapped in a 20s timeout.

Returns `image/jpeg` with `Cache-Control: no-store`.

### Brokered live view via MediaMTX (`services/mediamtx.rs`)
`GET|POST /api/v1/cameras/{id}/liveview`. Live view is **brokered through the media
gateway** (Layer 3: `Camera → media gateway → browser`, never
`Camera → every browser`):

1. Resolve the camera's source RTSP URL **with embedded credentials**
   (sub-stream preferred, else record URL).
2. Probe MediaMTX `GET {api}/v3/config/paths/get/cam_{id}`; if absent, `POST
   {api}/v3/config/paths/add/cam_{id}` with `{source, sourceOnDemand:true}`. A `400`
   (already exists / race) is tolerated; other failures surface as 500.
3. Return non-credentialed playback URLs the browser can consume directly:
   - HLS: `{hls_base}/cam_{id}/index.m3u8` (`:8888`)
   - WebRTC: `{webrtc_base}/cam_{id}` (`:8889`)
   - RTSP: `{rtsp_base}/cam_{id}` (`:8554`)

The **camera credentials never leave the server** — they live only inside the
MediaMTX `source` config; the browser only ever sees the gateway path name.
`sourceOnDemand:true` means MediaMTX only pulls from the camera while a viewer is
connected, avoiding a permanent extra session per camera.

---

## 8. Recording-without-decode & main/sub stream strategy

Stage 0 implements this separation of workloads directly: ingest pulls the compressed
stream, record stores compressed packets/segments, decode converts compressed video into
frames, and infer runs AI models. Recording should normally avoid decode; AI requires
decode.

Concretely:
- The recorder is **ingest + record only**: `-c copy` keeps the camera's H.264/H.265
  bitstream untouched from RTSP socket to MP4 file. There is no decode in the
  24/7 path, so CPU/GPU cost is independent of resolution and AI is not yet a factor.
- **Decode happens only on demand and at the edges**: ffprobe in the indexer (cheap
  metadata read), single-frame MJPEG extraction for snapshots, and the keyframe-copy
  trim for clips. None of these run continuously.
- This honors the stream-role split: 24/7 recording and evidence export use the
  **main stream** (`record_stream` defaults to `main`), while **live preview and the
  snapshot live path prefer the sub-stream** (`stream_url(cam,"sub")` first, record
  URL as fallback). The per-stream choice is data-driven: `record_stream` selects
  which stream the recorder pulls; live view / live snapshot independently bias
  toward the lighter sub-stream.
- It also realizes the core principle that **raw continuous video stays local by
  default**: segments are written to the local `recordings_dir` and served
  from there; nothing is pushed to cloud.

### RTSP URL construction (`camera_url.rs`)
`stream_url(cam, "main"|"sub")` returns an explicit `main_stream_url`/`sub_stream_url`
override if set, otherwise builds from the vendor template:
- `hikvision` → `/Streaming/Channels/101` (main) or `/102` (sub)
- `dahua` → `/cam/realmonitor?channel=1&subtype=0` (main) or `subtype=1` (sub)
- `generic`/`onvif` → returns `None` (cannot guess a path; an explicit URL is required)

Credentials are percent-encoded into the userinfo (`encode_userinfo`, RFC-3986
unreserved set) and assembled as `rtsp://user:pass@host:port/path`.

---

## 9. Credential handling & masking

- **Storage**: `username`/`password` are stored **plaintext** in the `cameras`
  table (schema comment explicitly flags this as Stage-0-only).
- **Never serialized to clients**: the `Camera` row struct is internal;
  `CameraView` (the only camera shape returned by the API) drops `password`
  entirely and exposes `has_password: bool` plus `record_url_masked`.
- **Masking** (`camera_url::mask_url`): replaces the `user:pass@` (or `user@`)
  userinfo of any RTSP/HTTP URL with `***@` before it appears in API responses,
  logs, or the `/test` probe result/error. The recorder logs the masked URL, never
  the credentialed one. The `/cameras/{id}/test` endpoint additionally masks the
  ffprobe **error string** (which can echo the URL).
- **In transit to the gateway**: credentials are sent only to MediaMTX's loopback
  control API (`127.0.0.1:9997` by default) inside the path `source`; they are
  never minted into the HLS/WebRTC/RTSP URLs handed to the browser.

---

## 10. HTTP API surface

| Method | Path | Purpose |
|---|---|---|
| GET | `/healthz` | Liveness `{status:"ok"}` (no dependency check) |
| GET | `/readyz` | Readiness — runs `SELECT 1`; `200 {ready:true}` / `503 {ready:false,reason:"database"}` |
| GET | `/metrics` | Prometheus exposition (system + per-camera gauges/counters) |
| GET | `/api/v1/system` | Version, uptime, camera/segment counts, footprint vs cap, + `storage` block (disk/footprint/projection) |
| GET / POST | `/api/v1/cameras` | List / create cameras |
| GET / PATCH / DELETE | `/api/v1/cameras/{id}` | Read / partial update / delete (+stop recorder, +purge files) |
| GET / POST | `/api/v1/cameras/{id}/test` | Probe the record stream for reachability/codec/dims |
| GET | `/api/v1/cameras/{id}/segments` | Timeline index rows (with media URLs) |
| GET | `/api/v1/cameras/{id}/timeline` | Coalesced availability ranges |
| GET | `/api/v1/cameras/{id}/gaps` | Recording-coverage gaps for `[from,to]` (holes between availability ranges) |
| POST | `/api/v1/cameras/{id}/clip` | Export `-c copy` MP4 for `[from,to]` |
| GET | `/api/v1/cameras/{id}/snapshot` | JPEG frame (recorded `?at` or live) |
| GET / POST | `/api/v1/cameras/{id}/liveview` | Register MediaMTX path, return HLS/WebRTC/RTSP URLs |
| GET | `/api/v1/health/cameras` | All camera status rows |
| GET | `/api/v1/cameras/{id}/health` | One camera's status |
| GET | `/api/v1/events` | Event log (filter by camera_id/event_type/severity, limit ≤2000) |
| — | `/media/recordings/*`, `/media/clips/*`, `/media/snapshots/*` | Static file serving (`ServeDir`) |

Errors are normalized by `error::AppError` → JSON `{ "error": msg }` with
NotFound→404, BadRequest→400, Conflict→409, DB/Other→500 (internal detail logged,
not leaked).

---

## 11. Configuration (`config.rs`)

All via `HELDAR_*` env vars (see `.env.example`). Notable defaults:

| Var | Default | Meaning |
|---|---|---|
| `HELDAR_DATABASE_URL` | `sqlite://./data/heldar.db` | SQLite only in Stage 0 |
| `HELDAR_DATA_DIR` / `RECORDINGS_DIR` / `CLIPS_DIR` / `SNAPSHOTS_DIR` | `./data` + subdirs | media roots |
| `HELDAR_FFMPEG_BIN` / `FFPROBE_BIN` | `ffmpeg` / `ffprobe` | external binaries |
| `HELDAR_MEDIAMTX_API_URL` | `http://127.0.0.1:9997` | gateway control API |
| `HELDAR_MEDIAMTX_HLS_BASE` / `RTSP_BASE` / `WEBRTC_BASE` | `:8888` / `rtsp://...:8554` / `:8889` | viewer URLs |
| `HELDAR_RECORDER_ENABLED` | `true` | master recorder switch |
| `HELDAR_DEFAULT_SEGMENT_SECONDS` | `60` | segment length |
| `HELDAR_DEFAULT_RETENTION_HOURS` | `24` | age policy |
| `HELDAR_INDEXER_INTERVAL_S` / `HEALTH_INTERVAL_S` / `RETENTION_INTERVAL_S` | `10` / `15` / `300` | loop cadences |
| `HELDAR_MAX_RECORDINGS_GB` | `20` | global size cap (soft footprint budget) |
| `HELDAR_MIN_FREE_DISK_GB` | `5` | disk-free floor (hard host-protection floor) |
| `HELDAR_ALERT_WEBHOOK_URL` | *(unset)* | alert webhook; unset disables the notifier |
| `HELDAR_NOTIFIER_INTERVAL_S` | `15` (min 5) | notifier poll cadence |
| `HELDAR_API_HOST` / `API_PORT` | `0.0.0.0` / `8000` | bind address |
| `HELDAR_CORS_ORIGINS` | `http://localhost:5173` | `*`/empty = allow all |

> Stage 1 observability/reliability config is documented end-to-end in
> [`docs/OBSERVABILITY.md`](docs/OBSERVABILITY.md).

---

## 12. Stage 0 limitations and where they map onward

| Limitation (Stage 0, as built) | Why it's acceptable now | Where it's addressed |
|---|---|---|
| **SQLite only** — `db.rs` hard-bails on non-`sqlite` URLs | Single-node edge box; WAL handles the 8–16 camera target | SQLx is DB-agnostic; Postgres path planned (multi-node/cloud coordination) |
| **Plaintext credentials** in `cameras.password` | Trusted single-tenant deploy; never serialized, always masked | Secret store / encryption (schema comment; security hardening stage) |
| **Keyframe-aligned clip cuts** (`-c copy`, no re-encode) | Preserves quality and is cheap; precision bounded by GOP | Frame-accurate trimming via optional re-encode in a later playback stage |
| **No auth on the API** | Local/LAN dev; CORS is the only gate | AuthN/AuthZ + tenant scoping (the `tenants`/`sites` tables already exist) |
| **Audio dropped** (`-an`) | Video-first VMS; halves edge cases | Audio capture can be re-enabled when needed |
| **Evidence lock has no mutating API** (`locked`/`incident_id` columns only) | Retention already honors the flag | `lockEvidence(...)` endpoint (Layer 3 playback API) |
| **No AI / frame sampler / decode pipeline** *(Stage 0 only)* | Stage 0 is the media kernel; clean ingest/record/decode/infer separation already in place | **Resolved in Stage 2** — frame sampler + worker contract shipped (§15); detection/tracking *models* are Stage 3. `events`/`capabilities` schema was pre-shaped for it |
| **No SMART / disk-throughput monitoring** | statvfs free-space + footprint + write-rate projection cover capacity planning (Stage 1, §14); per-byte throughput/SMART is lower-value on the edge box | Future hardware-health probe if a deployment needs it |
| **Single-node, raw video stays local** | Matches the local-first core principle | Stage 1 edge offline buffer + cloud sync retry (still planned; alerting webhook ships the metadata/alert upstream path) |

> Resolved in **Stage 1** (see §14): `fps_observed` is now populated by the indexer,
> storage gained a free-disk floor + free-space projection + Prometheus metrics, the
> service watchdog is the supervised-task respawner, and the gap detector +
> `/api/v1/cameras/{id}/gaps` make every recording hole explainable.

---

## 13. Background-task topology summary

```
 main()
   ├─ RecorderManager.start_all()
   │     └─ per camera: tokio::spawn supervise(id)   ── owns 1 ffmpeg child
   │                         writes  recordings_dir/<id>/<UTC strftime>.mp4
   │
   ├─ spawn_supervised indexer::run   (every ~10s)  scans dirs → segments rows, gaps, fps/bitrate
   ├─ spawn_supervised health::run    (every ~15s)  recording→error on staleness
   ├─ spawn_supervised retention::run (every ~300s) age + size-cap + free-floor purge (skip locked)
   ├─ spawn_supervised notifier::run  (every ~15s)  POST warning/critical events → webhook
   │     (each spawn_supervised wrapper respawns its task 5s after any return/panic)
   │
   └─ axum::serve(...)                          HTTP API + /metrics + /healthz + /readyz + /media
         on SIGINT/SIGTERM → recorder.shutdown() → kill every ffmpeg child
```

All concerns (1 supervisor-set + 4 supervised loops + HTTP) share the single
`SqlitePool` and `Arc<Config>`; coordination between the recorder (writes files,
sets `connecting`/`recording`/`offline`) and the indexer (reads files, confirms
`recording`, computes bitrate/fps) is entirely through the filesystem and the
`camera_status` row — there is no in-process channel between them, which keeps the
write path non-blocking. The notifier reads only the `events` table (a polling
cursor), so alerting is fully decoupled from the producers.

---

## 14. Stage 1 — Observability & Reliability

Stage 1 makes the kernel **operable by a non-developer**: faults are
visible without log-diving, recording gaps are explainable, and the host disk is
protected. It adds no new tables — everything is computed over the existing
`segments`, `camera_status`, and `events` tables, or read live from the OS. The
operator/SRE-facing guide is [`docs/OBSERVABILITY.md`](docs/OBSERVABILITY.md); this
section documents the implementation.

### 14.1 Storage monitoring (`services/storage.rs`)

`disk_stats(path)` calls **`statvfs(3)`** via `libc` on `HELDAR_RECORDINGS_DIR`
and reports `total/free/used_bytes` + `used_percent`. Free space is `f_bavail`
(blocks available to a non-privileged user — the space we can actually write), not
`f_bfree`. It returns `None` (serialized as `null`) if the syscall fails.

`storage_report(pool, cfg)` (surfaced as the `storage` block on
`GET /api/v1/system`, `routes/system.rs`) combines that disk view with the
recordings footprint:

| Field | Computed as |
|---|---|
| `disk` | `disk_stats(recordings_dir)` or `null` |
| `recordings_bytes` / `segment_count` | `SUM(size_bytes)` / `COUNT(*)` over `segments` |
| `oldest_segment` / `newest_segment` | `MIN(start_time)` / `MAX(end_time)` |
| `write_rate_bytes_per_day` | `SUM(size_bytes)` of segments indexed (`created_at`) in the last 24 h |
| `projected_days_remaining` | `disk.free_bytes / write_rate`; `null` if disk is null or rate is 0 |

`projected_days_remaining` is a *free-disk-fill* horizon (ignores that retention
recycles old segments), not a retention horizon.

### 14.2 Prometheus metrics (`services/metrics.rs`, `routes/metrics.rs`)

`GET /metrics` renders Prometheus text exposition
(`text/plain; version=0.0.4`) directly from SQL each scrape (no in-process
registry). System gauges: `heldar_build_info`, `heldar_cameras_total`,
`heldar_cameras_recording`, `heldar_segments_total`,
`heldar_recordings_bytes`, and (when statvfs succeeds)
`heldar_disk_total_bytes` / `heldar_disk_free_bytes` /
`heldar_disk_used_percent`. Per-camera series (labeled `camera`):
`heldar_camera_up` (1 if `state='recording'`), `..._reconnects_total`,
`..._segments_written`, `..._bitrate_kbps`, `..._last_segment_age_seconds`. The
full table (types/labels/conditions) is in `docs/OBSERVABILITY.md` §2. Note there is
**no fps metric** on `/metrics`; observed fps is health-API-only (§14.6).

### 14.3 Alert notifier (`services/notifier.rs`)

A supervised loop that **POSTs warning/critical events to a webhook** when
`HELDAR_ALERT_WEBHOOK_URL` is set (no-op otherwise). Key properties:

- **Starts from now:** the delivery cursor is `Utc::now()` at boot, so history is
  never replayed on restart.
- Polls every `HELDAR_NOTIFIER_INTERVAL_S` (default 15, min 5); each cycle pulls
  up to 100 events with `severity IN ('warning','critical') AND created_at > cursor`
  oldest-first and POSTs one JSON body per event
  (`{source, event_id, event_type, severity, camera_id, timestamp, payload}`).
- **Retry semantics:** a *transport* failure (no response) stops the cursor and
  retries that event next cycle (at-least-once); a *non-2xx response* is logged but
  the cursor advances (not retried). 10 s HTTP timeout.

Delivered event types today: `camera_offline`, `recorder_error`, `recording_gap`
(all warning), and `disk_pressure` (warning/critical). `retention_delete` (info) is
**not** delivered.

### 14.4 Disk-free retention floor (`services/retention.rs`)

The sweeper gained a third phase on top of Stage 0's age policy + size cap:

1. **Age** — delete unlocked segments older than each camera's `retention_hours`
   (`retention_delete`/info).
2. **Size cap** (`HELDAR_MAX_RECORDINGS_GB`, soft) — prune oldest *unlocked*
   segments until the unlocked footprint fits `budget = cap − locked_bytes`
   (`disk_pressure`/warning).
3. **Disk-free floor** (`HELDAR_MIN_FREE_DISK_GB`, hard) — **new in Stage 1**:
   while `statvfs` free space is below the floor, prune oldest unlocked segments
   (batches of 20, capped at 200 iterations/sweep) until back above it
   (`disk_pressure`/critical).

The floor is a host-protection backstop independent of the size cap: it fires on
the *whole filesystem's* free space, so it still protects recording if something
else on the box consumes disk.

**Locked/evidence guarantee (preserved & reinforced):** every delete query filters
`locked = 0`, so evidence is never deleted by any phase. The size-cap budget
subtracts locked bytes so evidence cannot force-delete all unlocked footage; if
locked footage alone meets/exceeds the cap (`budget ≤ 0`) the sweeper logs a
`disk_pressure` warning (`locked_exceeds_cap`) instead of deleting. If the disk is
below the floor but no unlocked segments remain, it warns and stops rather than
touching evidence.

### 14.5 Gap reporting (`services/indexer.rs`, `routes/recordings.rs`)

Two surfaces, both already backed by the timeline index:

- **Live event** — when the indexer adds a segment whose `start_time` is > 3 s
  after the previous segment's `end_time`, it logs `recording_gap` (warning) with
  `{gap_seconds, prev_end, next_start}`.
- **On-demand** — `GET /api/v1/cameras/{id}/gaps?from&to` coalesces segments into
  availability ranges (2 s tolerance) and returns the holes between them
  (`{camera_id, from, to, gaps:[{start,end,seconds}], gap_count, total_gap_seconds}`),
  reusing the same `coalesce()` helper as `/timeline`.

### 14.6 Observed fps & bitrate (`services/indexer.rs` → `repo.rs`)

On indexing each segment the indexer computes `bitrate_kbps = size·8 / duration /
1000` and reads `fps` from `ffprobe`, then upserts both onto the camera's
`camera_status` row via `record_segment_indexed`. These are **last-value** (latest
indexed segment), exposed through `GET /api/v1/health/cameras` /
`/api/v1/cameras/{id}/health` (`CameraStatus.fps_observed`, `.bitrate_kbps`).
Bitrate is also mirrored to Prometheus; fps is not.

### 14.7 Readiness (`routes/health.rs`)

`/healthz` (liveness, always 200, no dependency check) is joined by **`/readyz`**,
which runs `SELECT 1` against the pool and returns `503 {ready:false,
reason:"database"}` when the store is unreachable — a real readiness gate for
orchestrators/load balancers, distinct from liveness.

### 14.8 Supervised background tasks (`main.rs`)

The indexer, health monitor, retention sweeper, and notifier are launched through
`spawn_supervised(name, make)`: an outer task re-runs the inner `run()` loop, and
if it ever **returns or panics** it logs the cause and respawns after 5 s
(cancellation = clean stop). The `run()` loops are infinite by design, so this is a
resilience backstop — a single panic (e.g. a transient DB hiccup) cannot
permanently take metrics, alerting, or retention offline. This is Stage 1's
"service watchdog / auto-restart" for the in-process services; per-camera FFmpeg
recorders remain supervised by `RecorderManager` (reconnect with exponential
backoff).

---

## 15. Stage 2 — AI frame sampler

Stage 2 makes the kernel **feed AI without owning AI**: it
decodes a budgeted sample of each camera's sub-stream to a JPEG that workers pull,
stores a task model + detection results, and exposes a pull-based worker contract.
AI workers never touch RTSP, and a slow/absent worker cannot affect recording or
live view. The integrator-facing guide is [`docs/AI-WORKERS.md`](docs/AI-WORKERS.md);
this section documents the implementation. New code: `services/sampler.rs`,
`routes/ai.rs`, `migrations/0003_ai.sql`, the AI types in `models.rs`, and the
`ai_*` settings in `config.rs`. The reference Python worker lives in `apps/ai`.

```
   AI tasks (DB)            SamplerManager (services/sampler.rs)             AI worker (apps/ai)
   ai_tasks ──reconcile──►  rebalance(): one ffmpeg per AI-enabled camera   ┌──────────────┐
   (enabled)                  ▼                                             │ GET /ai/tasks│ discover
                       ffmpeg -vf fps=<budgeted>,scale=<w>:-2 ──► decode    │ GET /frame   │ pull JPEG
                              ▼                                             │ POST /ai/    │ post results
                    frames/<cam>/latest.jpg  (─update 1, overwritten)       │   events     │
                              ▼                                             └──────┬───────┘
              GET /api/v1/cameras/{id}/frame  (+ x-frame-age-ms / -captured-at)    │
                                                                                   ▼
                                          POST /api/v1/ai/events ──► detections table + events log
```

### 15.1 Sampler supervisor (`services/sampler.rs`)

`SamplerManager` is constructed in `main.rs`, stored in `AppState`, started via
`start_all()` and stopped on shutdown. It owns `Mutex<HashMap<camera_id,
SamplerTask>>` (a `watch::Sender<bool>` stop channel + `JoinHandle` per camera), a
parallel `info` map of `SamplerInfo {camera_id, state, fps}`, and a
`rebalance_lock`.

- **`rebalance()`** (also reached via `reconcile()`, and by `start_all()`) is the
  single mutating path, **serialized by `rebalance_lock`** so concurrent AI-task
  edits can't race into overlapping ffmpegs. It: stops every running sampler,
  clears `info`, returns early if `!ai_enabled`, then queries the active set —
  `SELECT c.id, MAX(t.fps), MAX(t.width) FROM cameras c JOIN ai_tasks t ON
  t.camera_id=c.id WHERE c.enabled=1 AND t.enabled=1 GROUP BY c.id` — and spawns
  one supervisor per camera at the budgeted fps. **One sampler per camera**, with
  fps/width taken as the **MAX across that camera's enabled tasks** (all tasks on a
  camera share one ffmpeg and one frame file).
- **`supervise()` loop** per camera: loads the camera, resolves the source as
  `stream_url(cam,"sub")` falling back to `record_url(cam)` (sub-stream preferred —
  the per-task `stream_profile` is *advisory* here today), `create_dir_all` on the
  frames dir, and spawns:

  ```
  ffmpeg -nostdin -hide_banner -loglevel warning -rtsp_transport tcp -timeout 15000000
         -i <url> -an -vf "fps=<fps>,scale=<width>:-2" -q:v 5
         -f image2 -update 1 -y  <frames_dir>/<cam>/latest.jpg
  ```

  stderr is drained concurrently (tail capped at 8 KB), `kill_on_drop(true)`
  prevents orphans. On ffmpeg exit: state → `offline`, a **`sampler_offline`**
  warning event is logged (masked tail), and it retries with exponential backoff
  (doubling, capped at 30 s). On stop it kills the child and returns.
- **States** (surfaced via `/api/v1/ai/samplers`): `connecting` → `sampling`, or
  `offline` / `error` / `stopped`. `MIN_FPS = 0.5` is the per-camera floor.

### 15.2 Frame storage

`HELDAR_FRAMES_DIR` (default `<DATA_DIR>/frames`, via
`Config::camera_frames_dir`) holds **one `latest.jpg` per camera** in
`frames/<camera_id>/`. `-update 1` overwrites that single file in place — there is
no growing frame directory and no per-frame id; it is the always-current frame
(last-value). `GET /api/v1/cameras/{id}/frame` serves it with `Content-Type:
image/jpeg`, `Cache-Control: no-store`, and two freshness headers computed from the
file mtime: **`x-frame-age-ms`** and **`x-frame-captured-at`** (RFC3339). The `{id}`
segment is rejected if it contains `/`, `\`, or `..` (path-traversal defense);
missing file → `404` ("no sampled frame yet…").

### 15.3 Budget & backpressure

A single global fps budget is shared across AI-enabled cameras so adding cameras
degrades per-camera fps instead of overloading the host:

```
active         = # enabled cameras with ≥1 enabled AI task
budget         = HELDAR_AI_MAX_TOTAL_FPS  (default 40, floored at 1.0)
per_camera_cap = budget / active
effective_fps  = max( min(MAX(task.fps), per_camera_cap), 0.5 )
```

So with the default budget: 4 AI cameras → ≤10 fps each, 8 → ≤5, 20 → ≤2; a camera
never exceeds its requested fps. The `MIN_FPS=0.5` floor wins over the strict budget
(many cameras can push the summed rate slightly above budget rather than starve any
camera to zero). Any AI-task create/update/delete triggers `reconcile()` →
`rebalance()`, recomputing the split and restarting samplers. This is a **static**
proportional fps split; the dynamic resolution-downgrade ladder + load-driven
recovery is deferred (per-task `width` is honored as MAX, not
auto-downgraded). High-res on-trigger capture is not in the sampler — a worker can
use the Stage 0 `/snapshot` endpoint for a main-stream grab.

### 15.4 Data model (`migrations/0003_ai.sql`)

Two tables, both `camera_id` FK → `cameras` `ON DELETE CASCADE`:

- **`ai_tasks`** — `id`, `camera_id`, `task_type` (free-form: detection/anpr/…),
  `enabled` (default 1), `stream_profile` (`sub`|`main`, default `sub`), `fps`
  (REAL, default 5, clamped 0.1…30 on write), `width` (INT, default 1280, clamped
  160…3840), `config` (JSON blob: model params/zones/thresholds, default `{}`),
  `created_at`/`updated_at`. Index `idx_ai_tasks_camera`.
- **`detections`** — `id`, `camera_id`, `task_type`, `timestamp`, `label`,
  `confidence`, `bbox` (JSON `[x,y,w,h]` normalized 0…1), `track_id`, `attributes`
  (JSON, default `{}`), `created_at`. Indexes `idx_detections_cam_time
  (camera_id,timestamp)`, `idx_detections_label`.

### 15.5 HTTP surface (`routes/ai.rs`)

| Method | Path | Purpose |
|---|---|---|
| GET / POST | `/api/v1/cameras/{id}/ai-tasks` | list a camera's tasks / create a task (`201`) |
| PATCH / DELETE | `/api/v1/ai-tasks/{task_id}` | partial update / delete (`204`) — both `reconcile()` |
| GET | `/api/v1/ai/tasks` | **worker discovery**: every enabled task on an enabled camera + its `frame_url` |
| GET | `/api/v1/ai/samplers` | per-camera sampler `{camera_id, state, fps}` (effective fps) |
| GET | `/api/v1/cameras/{id}/frame` | latest sampled JPEG + `x-frame-age-ms` / `x-frame-captured-at` |
| POST | `/api/v1/ai/events` | **ingest**: detections (+ optional event) for a camera |
| GET | `/api/v1/cameras/{id}/detections` | query detections (`from`/`to`/`label`/`limit≤5000`, newest-first) |

Create/update validate `stream_profile ∈ {sub,main}` and clamp fps/width; task ids
are `ai_<uuid>`. The router is `merge`d in `routes/mod.rs`.

### 15.6 Detections & events ingestion

`POST /api/v1/ai/events` takes `AiIngest {camera_id, task_type, timestamp?,
detections[], event?}`. The `camera_id` must exist (`404` otherwise). Each
`DetectionIngest {label?, confidence?, bbox?, track_id?, attributes?}` is inserted
as a `det_<uuid>` row stamped with the batch `timestamp` (RFC3339, or server `now()`
if omitted/unparseable); the response is `{detections_ingested: N}`. An optional
`event {event_type, severity?, payload?}` is written through the **same
`repo::log_event`** the kernel uses (default severity `info`), so AI alerts at
`warning`/`critical` flow straight into the Stage 1 notifier/webhook path (§14.3)
with no extra wiring.

### 15.7 Boot, wiring & config

`main.rs` builds `SamplerManager::new(pool, cfg)`, puts it in `AppState`, and calls
`sampler.start_all().await` after the recorders start; `shutdown_signal` calls
`sampler.shutdown()` alongside the recorders. The sampler's internal per-camera
tasks provide their own crash/backoff supervision (it is not wrapped in
`spawn_supervised`). Config (`config.rs`): `HELDAR_AI_ENABLED` (default `true`;
`false` runs no samplers), `HELDAR_AI_MAX_TOTAL_FPS` (40), `HELDAR_DEFAULT_AI_FPS`
(5), `HELDAR_DEFAULT_AI_WIDTH` (1280), `HELDAR_FRAMES_DIR`
(`<DATA_DIR>/frames`).

### 15.8 Isolation (the Stage 2 success criterion)

Sampling runs as a **separate set of supervised ffmpeg processes** that decode only
the sub-stream at a bounded total fps, writing to their own `frames/` tree. The
recorder's 24/7 `-c copy` path (no decode) and the MediaMTX live view are entirely
independent — there is no shared process, channel, or file between them. A crashing,
slow, or absent AI worker only stops *frames being read*; the sampler keeps writing,
and recording/live view are unaffected. This satisfies the Stage 2 goal: AI
consumes frames without breaking recording/live view. Detection/tracking models
themselves are Stage 3, plugging into the reference worker's `Analyzer` seam.

---

## 16. Stage 3 — Detection / tracking / zone kernel

Stage 3 is the
inflection where **frames become events** — the shared base for both the Security
and BakerySense apps. It has two halves that meet at the Stage 2 `POST
/api/v1/ai/events` contract:

1. **In the worker (`apps/ai`)** — a real **detector + tracker** runs behind the
   Stage 2 `Analyzer` seam: a YOLO/RT-DETR detector turns each sampled frame into
   person/vehicle boxes, and a **ByteTrack** associator stitches boxes across frames
   into stable **`track_id`s**. The worker posts these *tracked* detections (label,
   confidence, normalized `bbox`, `track_id`) through the unchanged ingest endpoint.
   This is documented for integrators in [`docs/AI-WORKERS.md`](docs/AI-WORKERS.md)
   §11; the kernel does not know or care which model produced the boxes.
2. **In the kernel (`crates/heldar-kernel`)** — a **zone engine** (`services/zones.rs`)
   evaluates each tracked detection against the camera's polygon **zones** and
   raises **`enter` / `exit` / `dwell`** zone events (with an evidence frame). Zone
   CRUD + a zone-events query live in `routes/zones.rs`; the schema is
   `migrations/0004_zones.sql`. This section documents the kernel half.

New code: `services/zones.rs`, `routes/zones.rs`, the `Zone` / `ZoneCreate` /
`ZoneUpdate` / `ZoneEvent` types in `models.rs`, and `migrations/0004_zones.sql`.
The `ZoneEngine` is built in `main.rs` and held in `AppState` (`state.zones`); it
has **no background loop** — it is driven synchronously from the detection-ingest
path.

```
   AI worker (apps/ai)                 media kernel (crates/heldar-kernel)
   ┌────────────────────┐
   │ YOLO detector      │  frame → person/vehicle boxes
   │ ByteTrack tracker  │  boxes → stable track_id per object
   └─────────┬──────────┘
             │ POST /api/v1/ai/events  { detections:[{label,confidence,bbox,track_id}], ... }
             ▼
   routes/ai.rs::ingest ── tx: insert detections ──► detections table
             │
             │ st.zones.process(camera_id, ts, &detections)        (synchronous, in-proc)
             ▼
   services/zones.rs::ZoneEngine
     load enabled zones for camera ─► for each tracked detection:
        ground point = bbox bottom-center ─► point-in-polygon per zone
        per-(camera,zone,track) state machine ─► enter / exit / dwell
             │                                     │
             ▼                                     ▼
        zone_events table                  repo::log_event "zone_{enter,exit,dwell}"
        (+ evidence frame copy)            (severity = zone.severity → Stage 1 notifier)
```

### 16.1 The detection → tracking pipeline (worker side, summarized)

The kernel's only requirement is the Stage 2 ingest shape. Stage 3 simply makes a
worker that fills in the optional `track_id`:

- **Detection** — a YOLO/RT-DETR baseline produces class-labelled boxes
  per frame (`person`, `car`, `truck`, `motorcycle`, …). Detection is not the
  product; detection is the input to events.
- **Tracking** — **ByteTrack** associates boxes across frames —
  including low-confidence ones — into continuous tracks, emitting a stable
  `track_id` per object. Because the Stage 2 reference worker creates **one
  `Analyzer` instance per task thread**, the tracker's per-camera Kalman/track state
  lives on `self` and persists across the camera's frame sequence.
- **Anonymous by default** — `track_id` is a per-session track handle, **not** an
  identity. Cross-camera ReID / identity resolution is Stage 6 (anonymous
  tracking by default).
- The worker posts `{label, confidence, bbox:[x,y,w,h] normalized 0..1, track_id}`
  via `POST /api/v1/ai/events` — the **same** endpoint and shape as Stage 2. No
  kernel or contract change was needed to light up tracking.

### 16.2 The zone engine (`services/zones.rs`)

`ZoneEngine::process(camera_id, ts, detections)` is invoked by
`routes/ai.rs::ingest` **after** the detections batch is committed. It turns
tracked detections into zone events:

1. **Gate** — return immediately if **no** detection in the batch has *both* a
   `track_id` and a `bbox`. Only tracked, boxed detections can drive zone
   membership; un-tracked detections (e.g. raw motion) are ignored here.
2. **Load zones** — `SELECT * FROM zones WHERE camera_id=? AND enabled=1`. If the
   camera has no enabled zones, return. Each zone's `polygon` and `labels` JSON are
   parsed once per call.
3. **Ground point** — a detection's position is the **bottom-center of its bbox**:
   for `bbox = [x, y, w, h]` (normalized, top-left origin) the point is
   `[x + w/2, y + h]`. Using the bbox's *ground contact* (feet of a person, tyres of
   a vehicle) rather than its centroid is what makes "is the object standing inside
   this floor region?" correct.
4. **Point-in-polygon** — a standard **ray-casting** test on the zone's normalized
   `[[x,y], …]` vertices (`point_in_polygon`); polygons with <3 vertices are never
   "inside".
5. **Label filter** — if a zone's `labels` array is non-empty, only detections whose
   `label` is in that list are evaluated against it (e.g. a "vehicle queue" zone can
   ignore `person`). An empty `labels` means *all* labels count.
6. **Per-track state machine** — keyed by `"{camera_id}|{zone_id}|{track_id}"`, the
   engine holds `TrackZoneState { inside, entered_at, dwell_emitted, last_seen }` in
   an in-memory `Mutex<HashMap>`. Transitions per evaluation:

   | Previous | Now inside? | Action |
   |---|---|---|
   | outside | inside | **`enter`** event; set `inside`, `entered_at=ts`, clear `dwell_emitted` |
   | inside | inside | if `dwell_seconds>0` and not yet emitted and `ts−entered_at ≥ dwell_seconds`: **`dwell`** event (once), set `dwell_emitted` |
   | inside | outside | **`exit`** event; clear `inside` |
   | outside | outside | (no event) |

   `dwell` carries the measured dwell in seconds and fires **at most once** per
   entry (re-arms on the next `enter`). `last_seen` is updated every evaluation.
7. **State pruning** — after processing the batch, any track state not seen within
   `STATE_TTL_SECS = 120` s is dropped, so the map can't grow unbounded as tracks
   churn. (A track that leaves the frame without an `exit` simply ages out; it is not
   force-exited.)

The engine evaluates against the **batch timestamp `ts`** passed from ingest (the
ingest envelope's `timestamp`, else server `now()`), so dwell math is anchored to
capture time, not wall-clock arrival.

### 16.3 Event emission + evidence (`ZoneEngine::emit`)

For each transition the engine writes **two** records and (on entry) captures
evidence:

- **`zone_events` row** — `id = zev_<uuid>`, with `camera_id`, `zone_id`,
  denormalized `zone_name`, `track_id`, `event_type` (`enter`|`exit`|`dwell`),
  `label`, `timestamp = ts`, `dwell_seconds` (only on `dwell`), `evidence_path`, and
  `created_at`.
- **Kernel event-log entry** — via the **same `repo::log_event`** the rest of the
  kernel uses: `event_type = "zone_enter" | "zone_exit" | "zone_dwell"`,
  `severity = zone.severity` (`info`/`warning`/`critical`), and a payload of
  `{zone_id, zone, kind, track_id, label, dwell_seconds, evidence}`. Because it goes
  through `events`, a `warning`/`critical` zone event flows straight into the
  **Stage 1 alert notifier/webhook** (§14.3) with no extra wiring.
- **Evidence frame (entry only)** — `copy_evidence` copies the camera's latest
  sampled sub-stream frame (`frames/<cam>/latest_sub.jpg`) to
  `snapshots/zoneevt_<id>.jpg` and stores the served URL
  (`/media/snapshots/zoneevt_<id>.jpg`) as `evidence_path`. This is a **cheap file
  copy — no decode, no extra ffmpeg** — reusing the Stage 2 sampler's always-current
  frame. If the copy fails (no frame yet), `evidence_path` is `null`. `exit`/`dwell`
  events do not re-capture (the `enter` evidence anchors the visit).

This is the **zone engine + evidence builder**, and a first
concrete **canonical event**: a typed event with a subject (`track_id` +
`label`), a location (`zone_id`/`zone_name`), a timestamp, confidence-carrying
detections behind it, and an evidence pointer. Identity/authorization/workflow
fields of the full canonical-event model arrive with Stages 4/6.

### 16.4 Data model (`migrations/0004_zones.sql`)

Two tables. `zones` is camera-scoped and editable; `zone_events` is an append-only
log.

**`zones`** — polygon regions per camera:

| Column | Notes |
|---|---|
| `id` | PK, `zone_<uuid-simple>` |
| `camera_id` | FK → `cameras(id)` `ON DELETE CASCADE` |
| `name` | required |
| `kind` | default `region`; free-form (`region`/`restricted`/`count`/…) — semantics live in the app, not the engine |
| `polygon` | JSON `[[x,y], …]` **normalized 0..1**; validated as ≥3 points on write |
| `dwell_seconds` | REAL, default 0; `>0` arms the `dwell` event past this threshold |
| `labels` | JSON array of detection labels that count (empty = all) |
| `severity` | `info`/`warning`/`critical` — severity stamped on emitted events |
| `config` | JSON blob (default `{}`) — reserved for per-zone tuning |
| `enabled` | only enabled zones are evaluated |
| `created_at`/`updated_at` | RFC3339 |

Index: `idx_zones_camera (camera_id)`.

**`zone_events`** — enter/exit/dwell log:

| Column | Notes |
|---|---|
| `id` | PK, `zev_<uuid-simple>` |
| `camera_id`, `zone_id` | the camera and zone; `zone_name` is **denormalized** so the event is self-describing even after the zone is renamed/deleted |
| `track_id` | the object whose path crossed the zone (nullable) |
| `event_type` | `enter` / `exit` / `dwell` |
| `label` | the detection label that triggered it |
| `timestamp` | event time (ingest batch ts) |
| `dwell_seconds` | set only on `dwell` |
| `evidence_path` | served URL of the copied entry frame (nullable) |
| `created_at` | server-assigned |

Indexes: `idx_zone_events_cam_time (camera_id, timestamp)`,
`idx_zone_events_zone (zone_id, timestamp)`. Note `zone_events` has **no FK** to
`zones` — events deliberately outlive the zone definition (auditability), which is
why `zone_name` is copied in.

### 16.5 HTTP surface (`routes/zones.rs`)

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/v1/cameras/{id}/zones` | list a camera's zones (incl. disabled), oldest-first |
| POST | `/api/v1/cameras/{id}/zones` | create a zone → `201` + the zone |
| PATCH | `/api/v1/zones/{zone_id}` | partial update (any subset of fields) |
| DELETE | `/api/v1/zones/{zone_id}` | delete → `204` (`404` if unknown) |
| GET | `/api/v1/cameras/{id}/zone-events` | query zone events (`from`/`to`/`zone_id`/`event_type`/`limit≤5000`, newest-first) |

Validation: `name` required; `polygon` must be an array of ≥3 `[x,y]` points;
`severity ∈ {info,warning,critical}`; `dwell_seconds` floored at 0; `kind` defaults
to `region`, `labels` to `[]`, `config` to `{}`, `enabled` to `true`. The router is
`merge`d in `routes/mod.rs`.

### 16.6 How it composes with Stages 0–2

- **Stage 0 (kernel)** — zones reference `cameras` (FK CASCADE: deleting a camera
  drops its zones); evidence frames are served by the existing
  `/media/snapshots` `ServeDir`; zone events are written through the same `events`
  table as recorder/health events.
- **Stage 1 (observability)** — zone events at `warning`/`critical` reuse the alert
  notifier/webhook path unchanged; they appear in `/api/v1/events` alongside
  `camera_offline`/`recording_gap`/etc.
- **Stage 2 (sampler + worker contract)** — the zone engine consumes the **exact**
  detections posted to `POST /api/v1/ai/events`, and its evidence frame is the
  sampler's `latest_sub.jpg`. The detector/tracker plugs into the reference worker's
  `Analyzer` seam with **no kernel or contract change** — Stage 3 added the
  `track_id`-aware *consumer* (the zone engine) and the worker-side *producer* (the
  tracker), meeting in the middle at the unchanged HTTP contract.

The zone engine adds **no new background task** and **no decode**: it runs inline on
the ingest request and only ever copies an already-sampled JPEG, so it cannot affect
recording, live view, or the sampler (the Stage 2 isolation guarantee is preserved).

### 16.7 Honest scope — engineering done, accuracy needs local data

The Stage 3 **engineering** is production-grade: the tracked-detection contract, the
polygon/point-in-polygon zone evaluation, the enter/exit/dwell state machine with
TTL pruning, evidence capture, the schema, and the query/CRUD API are complete and
tested (`services/zones.rs` unit tests cover point-in-polygon, ground-point, and
parsing). What is **not** yet validated is model **accuracy on local footage**:
public/pretrained detectors may not reflect Malaysian vehicle
distribution, plate/camera angles, motorcycles, night-IR, or rain, and
ReID/association degrades on new sites and in crowds. The mitigation is explicit:
start with type + color, treat make/model and any identity-like association as
assistive (top-5) candidates, **benchmark on local gate/shop footage**, fine-tune
only after local data collection, and never use model recognition as a hard access
decision. Accuracy benchmarking is gated on collecting that local footage set.

---

## 17. Stage 4 — Access Control app

Stage 4 is the first **vertical app** on the kernel: turn the Stage 3 event
substrate into a guard-operable gate. It adds three things — an **RBAC layer**, an
entry **registry**, and an **ANPR temporal-voting engine** — and reuses everything
below it (sampler, ingest contract, events log + alert webhook, retention loop,
evidence snapshots) **with no change to the Stage 0–3 paths**. The
operator/integrator guide is [`docs/ACCESS-CONTROL.md`](docs/ACCESS-CONTROL.md); this
section documents the implementation.

New code: `services/anpr.rs` (engine), `auth.rs` (RBAC + `Principal` extractor),
`routes/auth.rs` (login/users/keys), `routes/entry.rs` (registry + events +
reports), the Stage 4 types in `models.rs`, the `auth_*`/`anpr_*`/`entry_*` settings
in `config.rs`, and `migrations/0005_entry.sql`. The `AnprEngine` is built in
`main.rs` and held in `AppState` (`state.anpr`); like the zone engine it has **no
background loop** — it is driven synchronously from the detection-ingest path.

```
   AI worker (apps/ai)                    media kernel (crates/heldar-kernel)
   ┌────────────────────┐
   │ AnprAnalyzer       │  vehicle boxes → color → (optional) OCR plate, per frame
   │ YOLO+ByteTrack+OCR │  attributes:{plate, plate_confidence, vehicle_type, color, direction, …}
   └─────────┬──────────┘
             │ POST /api/v1/ai/events  { task_type:"anpr", detections:[{track_id, attributes}], ... }
             ▼
   routes/ai.rs::ingest ── tx: insert detections ──► detections table
             │
             │ if task_type == "anpr":  st.anpr.process(camera_id, site_id, &detections)   (sync, in-proc)
             ▼
   services/anpr.rs::AnprEngine
     per (camera|track) vote on normalized plate ─► winning plate ─► commit at min_votes / on TTL-prune
        resolve(): block-watchlist → registered-vehicle → visitor-pass → vip → unmatched
             │                                            │
             ▼                                            ▼
        entry_events row (+ evidence frame)       repo::log_event "entry_<auth_status>"
        (canonical event)                         (severity → Stage 1 notifier/webhook)

   RBAC: auth.rs::Principal (FromRequestParts)  ── auth_enabled? token→principal : system_admin
         routes/entry.rs / routes/auth.rs handlers ── principal.require(can_*(), action)
```

### 17.1 Entry domain tables (`migrations/0005_entry.sql`)

Eight tables across three groups. As with `zone_events`, **event/audit rows have no
camera FK** so they outlive a camera deletion (audit integrity); registry rows key on
a normalized plate.

**RBAC**

| Table | Notes |
|---|---|
| `users` | `id`, `username` UNIQUE, `password_hash` (argon2id PHC), `role`, `display_name`, `active`. |
| `sessions` | `id` = **SHA-256 of the issued token** (the token itself is never stored), `user_id` FK CASCADE, `expires_at`, `last_used_at`. |
| `api_keys` | `id`, `name`, `key_hash` UNIQUE (SHA-256), `key_prefix` (display only), `role`, `active`, `last_used_at`. |

**Registry**

| Table | Notes |
|---|---|
| `vehicles` | the allow anchor. `plate_norm` UNIQUE (uppercased, alphanumeric-only); `owner_type ∈ student\|staff\|resident\|contractor\|visitor`; optional `valid_from`/`valid_until` window; `vehicle_type`/`make`/`model`/`color` for secondary verification. |
| `visitor_passes` | `code` UNIQUE (`V-XXXXXX`), `visitor_name`, optional `plate`/`plate_norm`, `valid_from`/`valid_until` (NOT NULL), `status ∈ active\|checked_in\|checked_out\|expired\|revoked`, `checked_in_at`/`checked_out_at`, `created_by`. |
| `watchlist` | `plate_norm` UNIQUE, `kind ∈ block\|vip\|alert`, `reason`, `severity ∈ info\|warning\|critical`, `active`. |

**Events + audit**

| Table | Notes |
|---|---|
| `entry_events` | the canonical entry event. Denormalized columns `plate`/`auth_status`/`workflow_status`/`direction`/`timestamp`/`plate_confidence` for fast query+reports; `subject`/`authorization`/`evidence`/`workflow`/`audit` are JSON. `event_type ∈ vehicle_entry\|vehicle_exit\|visitor_checkin\|visitor_checkout`. Indexed on ts/plate/auth_status/workflow_status. |
| `audit_log` | append-only RBAC accountability: `actor`, `actor_name`, `role`, `action`, `target_type`, `target_id`, `detail` JSON. |

### 17.2 The ANPR engine (`services/anpr.rs`)

`AnprEngine::process(camera_id, site_id, detections)` is invoked by
`routes/ai.rs::ingest` **after** the detection batch is committed, **only** when the
task is `anpr`. It consolidates many noisy per-frame plate reads of one vehicle into
**one** authoritative event.

**Server-time, per-track voting.** State is an in-memory `Mutex<HashMap<key,
TrackVoteState>>` keyed `"{camera}|{track}"`; with no `track_id` the key falls back to
`"{camera}|plate:{norm}"` so repeated reads of the same plate still consolidate (and
dedupe) inside the window. **All timing is server time** (`Utc::now()`), never the
worker-supplied timestamp. For each detection the engine:

- normalizes the plate (`normalize_plate`: ASCII-alphanumeric, uppercased) and adds a
  **vote** for that key (count + summed confidence);
- latches the **highest-confidence** observation of each attribute
  (`vehicle_type`/`color`/`make`/`model`), the `direction` (`inbound`/`outbound`), and
  `model_versions`.

**Winning plate** (`winning_plate`) = most votes, tie-broken by summed confidence, but
**plausible plates are preferred** over implausible ones (a digits-only OCR misread
can't mask a real plate); the overall leader is used only when none is plausible.
`is_plausible_plate`: 3–10 chars **and** mixes a letter and a digit (Malaysian shape).

**Commit + prune.** In one locked pass the engine (a) **commits** any track whose
winning plate has reached `anpr_min_votes` (default 3, clamp 1…50) reads — voting on
the *plate*, not the raw detection count, so a single noisy read or a plateless track
can't trip the gate; and (b) **prunes** tracks not seen for `STATE_TTL_SECS = 30` s,
**committing on prune** any that produced ≥1 plate read but never reached threshold (a
vehicle that passed too fast to accumulate votes is still logged). Tracks that yielded
**no** plate (pure background vehicles) are dropped silently so the log isn't flooded
with `unmatched` events. Commit jobs are built under the lock and processed after
release; an insert failure clears `committed` so a still-live track retries next batch.

**Identity resolution (`resolve`), strict precedence — first match wins:**

1. **Unreadable** plate (empty / not plausible) → `unmatched` (`no_plate_read` /
   `plate_unreadable`).
2. **Block watchlist** (`active`, `kind='block'`) → `blocked` (severity = entry's
   `severity` or `critical`). This is the only security-critical lookup and **fails
   closed**: a DB error becomes an `exception` (`watchlist_lookup_failed`), never a
   silent fall-through to an allow branch.
3. **Registered vehicle** (`active`): outside its validity window →
   `exception (outside_validity_window)`; else an **attribute check** comparing
   **`color` + `vehicle_type` only** (make/model is assistive, never a mismatch
   trigger), mismatch (both sides known + differ, case-insensitive)
   → `exception` with the `mismatches` list; a clean match → `matched`/`auto`, **but a
   concurrent alert listing downgrades it to `exception`/`pending`**.
4. **Visitor pass** currently within its window (`status IN active,checked_in`,
   `valid_from ≤ now ≤ valid_until`, newest `valid_until` first so a future-dated pass
   can't mask a valid one) → `matched`; an `active` pass on an **inbound** read is
   auto-flipped to `checked_in`. A pass that exists but is outside its window →
   `exception (pass_outside_validity_window)`.
5. **VIP watchlist** (`kind='vip'`) → `matched` (informational allow).
6. Otherwise unknown: **alert-listed** (`kind='alert'`) → `exception`; else
   `unmatched`.

`auth_status ∈ matched | exception | unmatched | blocked`;
`workflow_status ∈ auto | pending | confirmed | rejected`. A clean automatic match is
`auto`; everything needing review (`blocked`/`exception`/`unmatched`) is `pending`.

### 17.3 Canonical event + evidence + alert mirror

On commit the engine writes one `entry_events` row (the canonical event model — see
[`docs/ACCESS-CONTROL.md`](docs/ACCESS-CONTROL.md) §6 for the full JSON and field
mapping). `event_type` is `vehicle_exit` when `direction == "outbound"`, else
`vehicle_entry`. The top-level `plate` column is the **normalized** key; `subject.plate`
is the raw read; `subject.plate_valid` carries the plausibility flag;
`subject.make_model` is composed from make+model when present. `audit.model_versions`
is whatever the worker stamped.

**Evidence** (`copy_evidence`) copies the camera's latest sampled frame — preferring
`latest_main.jpg`, falling back to `latest_sub.jpg` — to
`snapshots/entryevt_<id>.jpg` and stores `/media/snapshots/entryevt_<id>.jpg` as
`evidence.snapshot_path`. A cheap file copy, no decode, reusing the Stage 2 sampler's
always-current frame (the Stage 3 evidence pattern).

**Alert mirror** — every event is also written to the kernel `events` log via the same
`repo::log_event` as `entry_<auth_status>` (e.g. `entry_blocked`) at the resolution's
severity, so `warning`/`critical` entry events flow into the **Stage 1 alert
notifier/webhook** (§14.3) with zero extra wiring — exactly like a zone event.

A **manual** guard check-in/out (`routes/entry.rs::record_manual_entry`) writes the
same canonical shape (`visitor_checkin`/`visitor_checkout`, `auth_status: matched`,
`workflow_status: confirmed`, `source: visitor_pass`) so the daily log is complete for
both automatic (ANPR) and booth (manual) entries.

### 17.4 The RBAC layer + `Principal` extractor (`auth.rs`)

Two credentialed principal kinds — **users** (`vos_…` session tokens) and **API keys**
(`vok_…`) — plus a synthetic **system** principal. Tokens are random 256-bit values;
only their **SHA-256** is stored (a DB leak exposes no usable credential). Passwords
are **argon2id**; login verifies even unknown/disabled users against a dummy hash so
latency can't reveal account existence.

`Principal` implements `FromRequestParts<AppState>` — it is just a handler argument.
`token_from_headers` reads `Authorization: Bearer <t>` (or `bearer`) and falls back to
`X-API-Key`. `resolve_token` dispatches on prefix: `vok_` → `api_keys` (active +
parseable-role checks, else deny — never fail-open), else a `sessions JOIN users`
lookup (expiry → delete + deny; inactive → deny; `last_used_at` best-effort stamp).

**`auth_enabled` gating** is the crux:

```
token present?
 ├─ resolves to a principal      ─► use it
 ├─ no/invalid token, auth_enabled=false ─► Principal::system_admin()   (open LAN appliance, default)
 └─ no/invalid token, auth_enabled=true  ─► 401 (authentication required / invalid credentials)
```

So with the default `HELDAR_AUTH_ENABLED=false` every request is the synthetic
admin and the whole API behaves as the pre-Stage-4 open appliance; flip it on and the
entry/admin surface requires a valid token and enforces roles. Five roles map to five
capabilities — `can_view` (all), `can_operate_gate` (admin/manager/guard),
`can_manage_registry` (admin/manager), `can_ingest` (admin/integration), `can_admin`
(admin) — asserted by `principal.require(allowed, action)` → 403 on denial. The full
role×capability matrix and per-endpoint roles are in
[`docs/ACCESS-CONTROL.md`](docs/ACCESS-CONTROL.md) §4/§9.

**Bootstrap** — `ensure_bootstrap` (called from `main.rs` right after migrations)
seeds one admin from `HELDAR_BOOTSTRAP_ADMIN_USER`/`_PASSWORD` (password ≥8) when
auth is enabled and no users exist; a no-op otherwise. **Last-admin protection** in
`routes/auth.rs` refuses to demote/disable/delete the final active admin (and refuses
self-deletion). Every mutation across `routes/auth.rs` + `routes/entry.rs` appends an
`audit_log` row via `auth::audit` (best-effort; never fails the caller).

### 17.5 HTTP surface (`routes/auth.rs`, `routes/entry.rs`)

Both routers are `merge`d in `routes/mod.rs`. The full table (method × path × role ×
purpose) is in [`docs/ACCESS-CONTROL.md`](docs/ACCESS-CONTROL.md) §9. In brief: `/auth/*`
(login/logout/me) + admin-only `/users` + `/api-keys`; `/vehicles` + `/watchlist`
(read = view, write = manage_registry); `/passes` + check-in/out + entry-event
confirm/reject (gate ops = operate_gate); `/entry-events` + `/reports/{entry-log,
exceptions}` (view); `/audit` (manager+). Reads require any authenticated principal;
the ANPR worker reaches the engine through the **Stage 2** `POST /api/v1/ai/events`
(ingest capability).

Reports resolve a `[from,to)` window from either `date=YYYY-MM-DD` (a UTC day, default
today) or explicit `from`/`to`. The exception report is
`auth_status IN ('blocked','exception','unmatched') OR workflow_status='rejected'`.

### 17.6 How it plugs into ingest + retention

- **Ingest** — no new endpoint. `routes/ai.rs::ingest` already runs the zone engine
  on tracked detections; Stage 4 adds one branch: `if task_type == "anpr"` →
  `st.anpr.process(camera_id, cam.site_id, &detections)`. The engine and the zone
  engine are independent in-proc consumers of the **same** committed batch.
- **Retention** — the Stage 0/1 sweeper (`services/retention.rs`) gained an entry
  phase governed by `HELDAR_ENTRY_RETENTION_DAYS` (default 365): prune
  `entry_events` older than the cutoff **and their evidence JPEGs**, then prune
  `audit_log` and the mirrored `events` rows past the same cutoff, and prune **expired
  `sessions`** every sweep. Recording-segment policy + evidence lock are untouched.
- **Evidence + alerting** — entry evidence is served by the existing
  `/media/snapshots` `ServeDir`; entry events reuse the `events` log + Stage 1 webhook.
- **Isolation preserved** — the engine adds **no background task** and **no decode**:
  it runs inline on the ingest request and only ever copies an already-sampled JPEG,
  so the Stage 2 guarantee (AI cannot break recording/live view) still holds.

### 17.7 Honest scope — engineering done, accuracy needs local data

The Stage 4 **engineering** is production-grade and unit-tested (`services/anpr.rs`
covers normalization, plausibility, plausible-preferred voting, and the
both-known-and-differ mismatch rule; `auth.rs` covers password/token roundtrips, role
parsing, and the capability matrix): temporal voting, the strict resolution precedence
with a fail-closed block lookup, the guard workflow, the canonical event + evidence,
RBAC, and the full CRUD/report API. What is **not** validated is **accuracy**: plate
OCR and vehicle-attribute recognition on **local Malaysian gate footage** — the
reference worker emits **type + color only** (no make/model
classifier), and by design attributes raise **review exceptions, never
auto-rejections**. Two further deliberate deferrals: **directional entry/exit *lines* +
calibration** (only a per-camera `direction` config *hint* is accepted; no
line-crossing/homography), and **extending the `Principal` guard to the legacy Stage
0–3 routes** (today auth gates the Stage 4 + ingest surface).

---

## 18. Stage 5 — BakerySense app

Stage 5 is the second **vertical app** on the kernel: the **same media kernel,
different ontology**. BakerySense (`heldar-bakery`) is a **proprietary retail
behaviour-analytics vertical that lives in a separate private repo**; this section
documents only its **open-core boundary**, not its internal design. Where Access Control
is identity-aware at a gate, BakerySense is **anonymous by construction**: it reads only
the kernel's anonymous perception data (zone events plus ephemeral person tracks) and
turns it into behaviour metrics and periodic reports, never touching identity. It reuses
everything below it (sampler, detector/tracker, zone engine, `zone_events` +
`detections`, the `cameras` registry) **with no change to the Stage 0–3 paths and no new
ingest path or decode**.

The defining architectural fact: BakerySense is **not** a `DetectionConsumer`. The zone
engine (Stage 3) and the ANPR engine (Stage 4) run **synchronously on the ingest hot
path** as `DetectionConsumer`s. BakerySense deliberately sits **off** that path: it is a
periodic **rollup loop + report generator** that reads tables the kernel has *already*
written. This is the **analytics-layer-over-kernel-data** pattern: the app reasons over
stored events and metadata, never over the live frame stream. The vertical owns its own
modules, schema, config, rollup loop, retention, and HTTP routes inside its private
crate; the open kernel carries none of it.

### How it composes (composed, not welded) + isolation

BakerySense is wired in `crates/heldar-server/src/main.rs` purely as a bundled app: its
schema is applied after the kernel migrations, its config is loaded from the environment
(the kernel `Config` carries none of it), its rollup loop is `spawn_supervised`, and its
router is `merge`d. Crucially it is **absent from the `consumers` vec**: it is not a
`DetectionConsumer`, so it never runs on the ingest request.

- **Retention** is owned by the app: it prunes its own derived tables on its own timer;
  the kernel retention sweeper and evidence lock are untouched.
- **Evidence clips** are not duplicated: reports point at a `camera_id` + window, and the
  operator requests footage from the **kernel** clip API
  (`POST /api/v1/cameras/{id}/clip`, §7). The analytics layer stores no video.
- **Isolation preserved**: because BakerySense reads stored tables on its own timer
  rather than consuming the ingest batch, a slow or crashed rollup cannot back-pressure
  ingest, recording, the sampler, or live view (a panic just respawns the loop). Adding
  the app is a link + `merge` + `spawn_supervised` with **zero** change to the kernel
  ingest handler, the cleanest expression yet of the "kernel-open, apps-bundled" seam.

---

## 19. Stage 6 — Movement intelligence

Stage 6 is the client's
**Movement intelligence** deliverable — the **same media kernel, cross-camera**. Where
Stages 4/5 reason **within** a camera (an ANPR gate, a shop's anonymous footfall),
Movement correlates the kernel's per-camera observations **across** cameras into
candidate journeys, and flags **red-zone breaches** — under the strictest privacy gates
in the stack. New code (all in `crates/heldar-movement`): `reid.rs` (the vehicle
candidate proposer + scoring + plate trail), `breach.rs` (the red-zone rule engine +
subject correlation), `routes.rs` (HTTP surface + audited searches), `schema.sql` (its
three tables), `config.rs` (knobs), `models.rs` (`CameraLink` / `MovementCandidate` /
`BreachAlert`), `lib.rs` (the privacy stance). The operator/integrator guide is
[`docs/MOVEMENT.md`](docs/MOVEMENT.md).

The defining architectural fact, like BakerySense, is what Movement **is not**: it is
**not** a `DetectionConsumer`. The zone engine (Stage 3) and the ANPR engine (Stage 4)
run **synchronously on the ingest hot path**. Movement sits **off** that path — it is a
**correlation layer over stored kernel data**: two `spawn_supervised` background loops (a
ReID candidate **proposer** and a red-zone breach **rule engine**) on a shared timer,
plus an on-demand **trigger** and **search** surface. It reads tables the kernel and
Access Control have *already* written (`entry_events`, `detections`, `zone_events`,
`zones`) — never the live frame stream or the ingest batch. There is **no appearance/
visual embedding anywhere** and **nothing is auto-confirmed**: every cross-camera link is
a scored *candidate* a human reviews, and every identity-like *search* is audited.

```
   gate / corridor cameras ─► sampler ─► worker (YOLO+ByteTrack, ANPR) ─► detections + entry_events + zone_events
        │ POST /api/v1/ai/events
        ▼
   routes/ai.rs::ingest ── (sync consumers: ZoneEngine, AnprEngine) ──► detections / zone_events / entry_events
        │
        │  ╌╌╌ kernel boundary; Movement reads, never consumes ╌╌╌
        ▼
   heldar-movement (spawn_supervised, every HELDAR_MOVEMENT_INTERVAL_S)
     reid::run     propose_vehicle_candidates(): same plate on two topology-linked cameras
                   within a plausible transit window ─► fused score ─► movement_candidates (pending)
                   + prune()
     breach::run   sweep(): zone 'enter' on red/restricted zones ─► breach_alerts (open),
                   correlate track_id → plate (±5 min)
        │
        ▼
   RBAC + audit gate (routes.rs)
     GET /movement/candidates ─► confirm/reject (human; ReID ≠ identity)
     GET /movement/breaches    ─► ack/resolve     (worked incident lifecycle)
     GET /movement/search/{plate,person} ─► AUDITED identity-like queries
```

### 19.1 The vehicle candidate proposer + exact scoring (`reid.rs`)

`reid::run` is launched in `main.rs` via `spawn_supervised("movement_reid", …)` and ticks
every `HELDAR_MOVEMENT_INTERVAL_S`; each tick runs `propose_vehicle_candidates()` then
`prune()`. `run_once()` exposes the proposer for the manual trigger (§19.6) and tests.

**Pairing.** The proposer joins `entry_events` to itself on the **same normalized plate**
appearing on **two different, topology-linked cameras**, `b` later than `a`, with `b`
inside the scan window — gated by a `camera_links` join that honors link direction
(bidirectional links also match the reverse edge). It rejects implausible gaps before
scoring: `gap < 1 s`, or `gap > transit_seconds × 4`.

**Scoring (`score_pair`)** fuses signals into `[0,1]` with the **plate as the anchor**:

| Component | Contribution |
|---|---|
| Plate-exact anchor | base **`0.8`** (not 1.0 — OCR can err / plates can be cloned) |
| Transit-time plausibility | `+0.10` if `gap ≤ transit`; `+0.05` if `transit < gap ≤ 2×transit`; else `0` |
| Colour agreement | `+0.05` match · **`−0.10` conflict** · `0` if either unknown |
| Vehicle-type agreement | `+0.05` match · **`−0.10` conflict** · `0` if either unknown |

clamped to `[0,1]`. A plate-exact pair with plausible transit and no attribute data scores
**0.9** (matching colour+type → **1.0**); an attribute **conflict lowers confidence** (a
possible misread/clone) but, because the plate dominates, a double conflict still floors
near **0.6**, above the default `MIN_SCORE = 0.5` — the conflict is surfaced in the
candidate's `signals` for the reviewer, not silently suppressed. A pair scoring `≥
MIN_SCORE` is written to `movement_candidates` as `subject_type='vehicle'`,
`anchor=<plate>`, `from_*`/`to_*` referencing the two entry events, `status='pending'`,
`ON CONFLICT(subject_type, from_ref, to_ref) DO NOTHING` so a human-reviewed pair is never
clobbered.

### 19.2 Person ReID — deliberately weak, on-demand only (`routes.rs::search_person`)

Person ReID has **no plate and no appearance embedding**, so it is **never auto-proposed**.
It exists only as an audited search: `GET /movement/search/person?camera&track&at` finds
linked downstream cameras + their transit windows, lists **distinct downstream person
tracks first seen** in `(at, at + transit×4]` from `detections` (`label='person'`), and
scores each on **topology + time only** — `0.4` if it arrived within the expected transit,
else `0.25`, with no appearance comparison. The deliberately low ceiling and the absence
of any auto-proposal **are** the privacy design: person movement is
human-triaged correlation, never asserted identity.

### 19.3 Camera-topology graph (`camera_links`)

`camera_links` is an **operator-configured directed adjacency** (*a subject leaving
`from_camera` may appear at `to_camera` within ~`transit_seconds`*). It is the spatial
prior that **scopes** all cross-camera matching — vehicle proposer and person search alike
— so no link means no candidate. `transit_seconds` (default 120, clamp 1…86400) drives
both the plausibility window and the transit score component; `bidirectional` makes the
edge match both ways; `UNIQUE(from_camera, to_camera)` dedups edges. CRUD is `manage`-gated
(create is audited as `movement_link_create`). The proposer only correlates cameras joined by an explicit `camera_links` row (no implicit fallback).

### 19.4 The red-zone breach rule engine (`breach.rs`)

`breach::run` (`spawn_supervised("movement_breach", …)`) ticks on the same interval and
runs `sweep()`. It resolves red zones by **kind** (`HELDAR_MOVEMENT_RED_ZONE_KINDS`,
default `restricted,red`; `zones WHERE kind=? AND enabled=1`) — so a red zone is just an
ordinary Stage 3 kernel zone tagged `restricted`/`red`. For each red zone it reads recent
**`enter`** events and records one `breach_alerts` incident per event
(`rule='red_zone_entry'`, `status='open'`, inheriting the zone's `severity` and the Stage 3
entry-frame `evidence_path`), **`ON CONFLICT(zone_event_id) DO NOTHING`** so a re-sweep of
the overlapping window never duplicates an incident. `correlate()` enriches each incident
best-effort: it joins the breach's `(camera_id, track_id)` to a **vehicle plate** in
`entry_events` within **±5 min** → `subject_type='vehicle'` + the plate; otherwise
`unknown` (person breaches stay unknown — no plate, no embedding).

**Complements, does not duplicate, the kernel.** The kernel zone engine already mirrors
`warning`/`critical` zone events into the `events` log + Stage 1 alert webhook (§16.3);
the breach engine **does not re-notify** — it adds the **worked, deduped, subject-correlated
incident** (open → acknowledged → resolved, with `resolved_by`/`resolved_at`). Real-time
push stays with the kernel; accountability + triage live here.

### 19.5 Data model (`schema.sql`)

`schema::init(&pool)` applies three tables idempotently against the **shared kernel pool**
at boot — owned by the app crate, **correlation/candidate data only, no legal-identity
records**. **`camera_links`** (directed adjacency, §19.3); **`movement_candidates`**
(`subject_type`, `anchor`=plate/`''`, `from_*`/`to_*` appearance refs, `score`, `signals`
JSON, `status` pending/confirmed/rejected, `reviewed_by`/`reviewed_at`;
`UNIQUE(subject_type, from_ref, to_ref)`; indexes on `(status,score)` + `anchor`); and
**`breach_alerts`** (`zone_event_id` UNIQUE dedup key, `rule`, `subject_type`/`subject`,
`severity`, `status` open/acknowledged/resolved, `detail` JSON
`{zone_event_at, correlation}`, `evidence_path`; index on `(status, created_at)`). As with
`zone_events`/`entry_events`, none of the three has an FK to the kernel source rows — the
correlation record outlives the camera/zone/entry it was derived from (auditability).

### 19.6 HTTP surface + the privacy gates (`routes.rs`)

Reads need **`view`**, candidate/breach **reviews** need **`operate_gate`**, topology edits
+ the manual `run` need **`manage`** (the Access Control capability matrix, §17.4). The
router takes `MovementConfig` as an `Extension` and is `merge`d in `main.rs`.

| Method | Path | Cap | Purpose |
|---|---|---|---|
| POST | `/api/v1/movement/run` | manage | Run proposer + breach sweep once (both also run on the timer) |
| GET / POST | `/api/v1/movement/links` | view / manage | List / create topology links (create audited) |
| DELETE | `/api/v1/movement/links/{id}` | manage | Delete a link |
| GET | `/api/v1/movement/candidates` | view | List candidates (`status`/`anchor`/`limit`), score DESC |
| POST | `/api/v1/movement/candidates/{id}/confirm`·`/reject` | operate_gate | **Human** review (sets `reviewed_by/at`); **audited** |
| GET | `/api/v1/movement/breaches` | view | List breach incidents (`status`/`limit`) |
| POST | `/api/v1/movement/breaches/{id}/ack`·`/resolve` | operate_gate | Work an incident; **audited** |
| GET | `/api/v1/movement/search/plate/{plate}` | view | **AUDITED** plate trail (appearances) + candidates |
| GET | `/api/v1/movement/search/person` | view | **AUDITED** weak person candidates (`?camera&track&at`) |

The three **privacy gates** are enforced in code: (1) candidate `confirm`/`reject` require
`operate_gate` and stamp the human reviewer — **ReID is never an automatic identity
assertion**; (2) both `search/*` endpoints call `auth::audit(...)` **before** querying, so
no identity-like lookup can run without an audit-log record; (3) both search responses
carry a `note` re-stating the result is probabilistic and requires human judgement, not
legal identity. The review mutations and `links` create are audited too (link **delete** is
capability-gated but not audit-logged).

### 19.7 How it composes (composed, not welded) + retention + isolation

Movement is wired in `crates/heldar-server/src/main.rs` purely as a bundled app: schema
applied after the kernel migrations (`heldar_movement::schema::init`), config from the
environment (`MovementConfig::from_env`), its two loops `spawn_supervised`, its router
`merge`d. It is **absent from the `consumers` vec** — not a `DetectionConsumer`, never on
the ingest request.

- **Retention** — `reid::prune()` (each tick) deletes `movement_candidates` older than
  `HELDAR_MOVEMENT_RETENTION_DAYS` (default 365), and `breach_alerts` older than the
  cutoff **only when `status='resolved'`** (open/acknowledged incidents are kept until
  worked). The app owns this lifecycle; the kernel retention sweeper + evidence-lock are
  untouched.
- **Evidence** — breach incidents reuse the Stage 3 zone entry-frame snapshot via
  `evidence_path`; Movement stores no video and captures no new frame.
- **Isolation preserved** — both engines read stored tables on their own timer (searches
  are on-demand reads), so a slow or crashed Movement loop cannot back-pressure ingest,
  recording, the sampler, or live view (a panic just respawns the loop after 5 s). Adding
  Movement is a link + `merge` + two `spawn_supervised` calls with **zero** change to the
  kernel ingest handler — the "kernel-open, apps-bundled" seam now correlating *across*
  cameras instead of *within* one.

### 19.8 Honest scope — engineering done, no embedding by design, accuracy deferred

The Stage 6 **engineering** is production-grade: the plate-anchored multi-signal proposer
with the exact fused scoring + transit gating, the human confirm/reject workflow, the
operator topology graph, the audited plate-trail + low-confidence person search, the
red-zone breach engine with `zone_event_id` dedup + track→plate correlation, the worked
incident lifecycle, the schema, retention, and the RBAC-gated API. **Deliberate
deferrals**: **no visual/appearance ReID embedding** anywhere —
vehicle ReID is anchored on the plate, person ReID is weak/topology-only; **no homography
/ ground-plane calibration** (transit windows are operator-declared per link, not
geometry-derived); **ReID accuracy is unbenchmarked on local footage** (false-link /
missed-link / path accuracy) — the human review gate is the safeguard, never
an auto-decision; and **cross-camera person journeys are low-confidence, human-triage
only** (never auto-proposed, capped at 0.4, always audited). This is a **scene/event
graph** applied to security: a typed, evidence-backed, audited cross-camera
correlation that stays explicitly probabilistic.

---

## 20. Stage 7 — Semantic search

Stage 7 turns
the platform's accumulated event facts into a queryable **visual-event memory** —
*who / what / where / when / confidence / evidence*. New code (all in
`crates/heldar-search`): `query.rs` (the `QueryPlan` + its deterministic executor),
`planner.rs` (the offline rule parser + the optional LLM seam), `proof.rs` (the claim
ladder), `routes.rs` (the HTTP surface + audit + log), `config.rs` (knobs), `schema.sql`
(its one query-log table), `lib.rs` (the governing principle). The operator/integrator
guide is [`docs/SEARCH.md`](docs/SEARCH.md).

The architecture is a **planner → deterministic executor → proof** pipeline, governed by one
rule: **the LLM is a query PLANNER, never the source of
truth.** A question is translated into a structured **plan** (a deterministic filter), the
plan is **executed** against stored kernel facts, and the **answer is the executed query's
rows** — not anything a model "said". No model ever sees, summarizes, or generates an answer
about the data; the inference surface is reduced to one explicit, inspectable, fallible step
(how the question was read), decoupled from the rows it selects.

Search is the most "composed, not welded" app in the stack: like BakerySense (§18) and
Movement (§19) it is **not** a `DetectionConsumer` — and unlike them it is **not even a
background loop**. It is a pure **read-only query layer over kernel facts**: three HTTP
routes plus one small query log, reading tables the kernel and the Stage 4/6 apps have
*already* written (`entry_events`, `zone_events`, `breach_alerts`). It owns no ingest path,
no decode, no `spawn_supervised` task, and no new fact table.

```
   kernel + app fact tables (Stages 3/4/6)        heldar-search (3 routes, no loop, no consumer)
     entry_events / zone_events / breach_alerts   ┌─────────────────────────────────────────────┐
        │                                         │ POST /search/events  ─ QueryPlan ───────┐     │
        │  ╌╌ search READS; never consumes ╌╌      │ POST /search/nl      ─ question ─ plan ─┤     │
        ▼                                         │       plan_llm() if LLM_URL set,        │     │
   query::execute(plan)  ◄───────────────────────┤       else parse_rules() (offline)      ▼     │
     time-bounded SQL per source (default 7d)     │                              execute → proof │
     + Rust field filters + sort + limit          │ POST /search/plan    ─ question ─ {plan}      │
        │                                         │       (dry-run: NO execution, NO data)        │
        ▼                                         └─────────────────────────────────────────────┘
   proof::build  → claim ladder (inference? · aggregate · event)
        │
        ▼   every search → search_log;  plate-targeted query → kernel audit_log
```

### 20.1 The `QueryPlan` + the deterministic executor (`query.rs`)

The `QueryPlan` is an all-optional flat struct (empty ⇒ "everything in the default window")
— time window (`from`/`to`), time-of-day (`hour_min`/`hour_max`, UTC hour), `cameras`,
`sources` (subset of `entry`/`zone`/`breach`, empty ⇒ all), and the attribute filters
`plate` / `color` / `vehicle_type` / `subject_type` / `auth_status` / `event_type` /
`zone_kind` / free-text `text` / `limit`. It is the **only** thing the NL layer produces, is
accepted directly by `/search/events`, is echoed in every response, and is stored in
`search_log`.

`execute(pool, plan, max)` is **pure SQL + Rust**, fully reproducible. It (1) resolves the
window — unset `from` defaults to **now − 7 days**, `to` to now + 1 min, so an unbounded
query never scans the whole history; (2) issues **one time-bounded, newest-first SQL query
per requested source**, capped at `fetch_cap = (max×5).clamp(100, 20_000)` — `entry_events`,
`zone_events LEFT JOIN zones` (for `kind`), and `breach_alerts` — normalizing every row into
a unified `SearchHit` (`claim_level = "event"`); (3) applies the remaining plan fields as
**deterministic Rust filters** (`hits.retain`: cameras, UTC hour bounds, plate, colour/type
on `subject`, lenient `subject_type`, `auth_status`, `event_type`, `zone_kind`, lowercased
`text` substring); (4) **merges, sorts newest-first, and truncates** to
`limit.unwrap_or(max).clamp(1, max)`. Only the time window and the fetch cap touch SQL (so
the query is always indexed and bounded); everything else is in-process. `breakdown(hits)`
computes counts by source / by day for the proof's aggregate.

### 20.2 The planner — rules by default, LLM as an optional seam (`planner.rs`)

Two implementations produce the **same** plan type. **`parse_rules(query, cameras)`** is the
always-available default: a transparent, dependency-free keyword parser over the lowercased
question (colour/type/subject/auth/source/event keywords, relative dates, time-of-day,
camera-name resolution by longest-name-first match, and a plate-like token), with `cameras`
as `(id, name)` pairs so phrases resolve to camera ids. It is **best-effort** — it recognizes
its patterns and leaves the rest to the default window — and it means **search works fully
offline with no external dependency.**

**`plan_llm(http, cfg, query, cameras)`** is engaged **only when
`HELDAR_SEARCH_LLM_URL` is set**. It asks an OpenAI-compatible endpoint
(`temperature: 0`, `response_format: json_object`, a system prompt spelling out the schema +
known camera ids, and the hard rule *"you never answer the question or invent data"*) to
emit a plan JSON, parsed back into a `QueryPlan` and `sanitize()`d (out-of-range hours
dropped). **It returns `None` — and the caller falls back to the rule parser — on any
failure** (no endpoint, non-2xx, unparseable). The model **never** sees or returns data;
only a plan flows out of it, executed deterministically and shown back exactly like a
rule-parsed one.

### 20.3 The proof layer (`proof.rs`)

`build(query, planner, plan, hits)` decomposes every answer into the
claim ladder — **observation → track → event → aggregate → inference**. Facts live at the
**event** level and below (kernel-produced, backed by `detections` provenance + an evidence
frame); this layer adds the **aggregate** (the executed count + by-source/by-day breakdown +
window, confidence `high`, *"the answer is these rows, not model output"*) and the
**inference** (the NL→plan reading, **`fallible: true`**, the *only* non-deterministic step,
with a caveat to verify the plan reflects intent). The **event** level spells out per-hit
provenance — pull the clip via the kernel clip API (`POST /api/v1/cameras/{id}/clip`) and the
evidence frame via `evidence_path`. A structured `/search/events` call has no question, so
the inference level is omitted — it has *no* fallible step. The closing note states plainly:
**no layer asserts identity or causation.**

### 20.4 HTTP surface, audit, and the search log (`routes.rs`, `schema.sql`)

All three routes require the Stage 4 RBAC **`view`** capability. `POST /search/events`
executes a `QueryPlan` directly (logged `mode=structured`, `planner=structured`);
`POST /search/nl` plans (LLM if configured, else rules) → executes → proves (logged
`mode=nl`, empty query ⇒ 400); `POST /search/plan` is a **dry-run** returning
`{ query, planner, plan }` only — **no execution, no data, no log, no audit**. Every
executed search writes a `search_log` row (actor, mode, verbatim question, executed plan,
planner, result count — `schema.sql`'s sole table), and a query that **targets a specific
plate** (`plan.plate.is_some()`, the re-identifying handle here) additionally writes a
`search_identity_query` row to the kernel `audit_log` via `auth::audit(...)` — the same
immutable trail as the Stage 6 plate searches. Every response echoes the `planner` and the
exact `plan` that ran, so there is nothing hidden between the question and the rows.

### 20.5 How it composes + honest scope

Search is wired in `crates/heldar-server/src/main.rs` purely as a bundled app: schema
applied after the kernel migrations (`heldar_search::schema::init`), config from the
environment (`SearchConfig::from_env`), router `merge`d. It is **absent from the `consumers`
vec** and has **no `spawn_supervised` loop** — it touches the ingest/recording/live-view path
nowhere, so a slow or failing request can only affect that request. Adding it was a
schema-init + a `merge` with **zero** change to the kernel ingest handler.

The Stage 7 **engineering** is production-grade: the `QueryPlan` + the deterministic
time-bounded executor over the three fact tables (default 7-day window + Rust filters +
sort/limit), the transparent offline rule parser, the optional LLM planner seam (sanitize +
fallback), the proof/claim-ladder layer, the search log + identity-query audit, and the
RBAC-gated structured / NL / dry-run routes. **Deliberate deferrals**: **open-vocabulary
VLM enrichment + event/clip embeddings + vector retrieval are
a documented seam, not built** — they need an embedding/VLM worker to write the vectors a
query layer could rank against; consequently **search-by-image / vehicle-crop / person-crop
is unavailable** (today's search is by structured *attributes*, not visual similarity);
**VLM-based report interpretation** is intentionally absent (the proof reports deterministic
aggregates, not generated prose); **the LLM planner is optional and untested without a live
endpoint** (the default path is the rule parser); and **the rule parser is best-effort** (it
cannot express dwell thresholds or multi-condition joins — use `/search/plan` to confirm a
parse, or send a structured `QueryPlan` for full control). This is an **event memory →
latent world memory** progression applied to search: a typed, evidence-backed,
deterministic query layer whose **only** inference — reading the question — is surfaced,
fallible, and decoupled from the answer.

## 21. Remote access — the WireGuard overlay model

A deployment is normally behind **CGNAT** (shared public IPv4, no inbound port-forward, DDNS
useless), so the only thing that reaches it is the node **dialing out**. Heldar standardizes
on a **WireGuard overlay** (Tailscale for personal/dev, NetBird self-hosted for shipped products)
running as an **external daemon** on the host. This is an **open kernel** capability — every
Apache-2.0 deployment gets private, P2P-first remote viewing with no proprietary component. Full
rationale, the transport comparison (vs Cloudflare-native and self-hosted reverse tunnels), and the
deploy recipes live in `docs/REMOTE-ACCESS.md`; this section records the architecture.

**Two layers, kept separate.** *Reachability* is the overlay: WireGuard builds a direct encrypted
tunnel between viewer and camera-site host whenever NAT traversal succeeds (hole-punch **+ UDP port
prediction**, or end-to-end IPv6), and falls back to an encrypted relay (DERP / NetBird relay) only
in the symmetric-on-both-ends case — a relay that forwards **ciphertext it cannot decrypt**. *Media*
is unchanged: MediaMTX serves its normal WebRTC (WHEP) / HLS on its normal ports, now reachable at
the host's overlay address; WHEP media is itself DTLS-SRTP encrypted (a second, independent layer).
So **content is private on every path**; only connection *metadata* is exposed, and only to a managed
coordinator — self-hosting the coordinator (NetBird/Headscale) removes even that.

**The overlay is orthogonal to the kernel.** Critically, remote access required **no** media-stack
changes: the overlay is a deployment concern (install a client, set an ACL), not kernel code. The
kernel does **not** embed or manage WireGuard — duplicating mature daemons would be wrong. Its entire
contribution is *awareness*: `config` reads `HELDAR_OVERLAY_{ENABLED,KIND,IFACE}`, and
`services::remote_access::status` probes the configured interface via `/sys/class/net/<iface>`
(dependency-free; TUN devices report `operstate=unknown` when healthy, so `unknown` is treated as
up), surfaced at `GET /api/v1/system → remote_access`. Transport-agnostic by construction: any
overlay that presents a network interface is reported. A managed, hosted, multi-site control plane —
if ever built — would be a **proprietary** crate; the default open path needs none of it. This is the
open-core boundary applied to connectivity: the platform-level, non-domain capability stays in the
Apache-2.0 kernel; only client-specific gateway customization would be proprietary.
