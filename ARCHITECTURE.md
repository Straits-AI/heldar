# VisionOps Core — Stage 0 Media Kernel Architecture

This document describes the Stage 0 "media kernel" of VisionOps Core **as actually
built** in `apps/core` (Rust / Axum / Tokio / SQLx), not as aspirationally planned.
It is the base VMS/NVR control plane: camera registry, RTSP ingest, segment
recording, timeline index, playback / clip / snapshot, brokered live view, and
camera health. Python AI workers and the detection/tracking kernel are later stages
and are intentionally absent here.

Stage 0 maps to **memo §14 "Stage 0 — Media kernel MVP"** (camera registry, RTSP
ingest, recording segmenter, timeline index, playback API, clip export, basic live
view, camera health) and is built on the layer model of **memo §5**. The recording
philosophy is governed by **memo §6 "Stream and codec strategy"** (cited in detail
below).

---

## 1. Layered architecture

The crate is organized as a thin HTTP control plane (Axum routes) over a set of
long-running background services, all sharing one SQLite store and one `Config`.
The layers below map one-to-one onto memo §5 (Layer 0–3); Layer 4 (AI frame
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

| Memo §5 layer | Stage 0 implementation | Files |
|---|---|---|
| **Layer 0 — Device registry** | `cameras` table + CRUD + `test` probe; vendor-templated RTSP URL builder; credential masking | `routes/cameras.rs`, `models.rs`, `camera_url.rs` |
| **Layer 1 — Stream ingestion** | Per-camera FFmpeg supervisor: RTSP pull, reconnect w/ backoff, status & bitrate metrics, reconnect/offline events | `services/recorder.rs`, `repo.rs` |
| **Layer 2 — Recording engine** | `-c copy` fragmented-MP4 segment writer (on disk) + timeline indexer + retention manager + gap detector + evidence lock | `services/recorder.rs`, `services/indexer.rs`, `services/retention.rs` |
| **Layer 2 — Storage monitor** | Global byte cap + `/api/v1/system` footprint reporting | `services/retention.rs`, `routes/system.rs` |
| **Layer 3 — Playback** | Segment listing, coalesced timeline ranges, clip export, snapshot extraction | `routes/playback.rs`, `routes/recordings.rs`, `services/clip.rs`, `services/snapshot.rs` |
| **Layer 3 — Live view (brokered)** | MediaMTX path registration + HLS/WebRTC/RTSP URL minting (server-side creds) | `services/mediamtx.rs`, `routes/liveview.rs` |
| **Camera health** | Staleness downgrade monitor + status & event APIs | `services/health.rs`, `routes/health.rs` |
| **Layer 4 — AI frame sampler** | **Not in Stage 0** (Stage 2+). `events`/`capabilities` schema is forward-shaped for it. | — |

### Process boot order (`main.rs`)

1. Load `.env` (dotenvy), init tracing (`VISIONOPS_LOG`, default `info,visionops_core=debug`).
2. Build `Config::from_env`; `create_dir_all` for data/recordings/clips/snapshots dirs.
3. Open SQLite pool (`db::init_pool`), run embedded migrations (`db::run_migrations`).
4. Construct `RecorderManager` and shared `reqwest::Client` (10s timeout).
5. `recorder.start_all()` — spawn one supervisor task per recordable camera.
6. `tokio::spawn` the indexer, health monitor, and retention sweeper loops.
7. Build the Axum router: API routes + three `ServeDir` mounts (`/media/recordings`,
   `/media/clips`, `/media/snapshots`) + `TraceLayer` + CORS.
8. Bind `api_host:api_port` (default `0.0.0.0:8000`) and serve with graceful shutdown
   on SIGINT/SIGTERM, which calls `recorder.shutdown()` to stop every FFmpeg child.

CORS allows all origins when `VISIONOPS_CORS_ORIGINS` is empty or contains `*`,
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
  bitstream straight to disk; no decode, no re-encode. This is the memo §6.1 rule
  made concrete (see §8 below).
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
  a no-op (with a warning) when `VISIONOPS_RECORDER_ENABLED=false`.
- `reconcile(id)` is called by the camera CRUD handlers after create/update: it
  stops any existing task, reloads the row, and (re)spawns only if `should_record()`,
  else marks `disabled`. This keeps recorders consistent with registry edits.
- `stop(id)` is called on delete (which also `remove_dir_all`s the camera's
  recordings) and `shutdown()` on process exit.

---

## 4. Timeline indexer (`services/indexer.rs`)

A periodic loop (`VISIONOPS_INDEXER_INTERVAL_S`, default 10s, min 2s) that turns
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

A loop (`VISIONOPS_HEALTH_INTERVAL_S`, default 15s, min 5s) catching the
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

A loop (`VISIONOPS_RETENTION_INTERVAL_S`, default 300s, min 30s) enforcing two
policies, in order. **Locked (evidence) segments are never deleted by either.**

1. **Age policy, per camera** — for each camera, delete segments with
   `locked = 0 AND end_time < now - retention_hours`. Each delete removes the file
   (`remove_file`) then the row. A summary `retention_delete` info event is logged if
   anything was removed.
2. **Global size cap** — read `SUM(size_bytes)` across all segments; while it exceeds
   `VISIONOPS_MAX_RECORDINGS_GB` (default 20 GB, stored as `max_recordings_bytes`),
   delete the oldest unlocked segments in batches of 20 (`ORDER BY end_time ASC`),
   re-checking the total each iteration. If the only remaining segments are locked,
   the loop breaks (the cap can be exceeded by locked evidence — by design). A
   `disk_pressure` warning event is logged if anything was pruned.

The **evidence lock** (`segments.locked`) is the memo §5 "Evidence lock" module:
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
gateway** (memo §5 Layer 3: `Camera → media gateway → browser`, never
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

## 8. Recording-without-decode & main/sub stream strategy (memo §6)

Stage 0 implements the **memo §6.1** separation of workloads directly:

> *"Ingest = pull compressed stream; Record = store compressed packets/segments;
> Decode = convert compressed video into frames; Infer = run AI models. Recording
> should normally avoid decode. AI requires decode."*

Concretely:
- The recorder is **ingest + record only**: `-c copy` keeps the camera's H.264/H.265
  bitstream untouched from RTSP socket to MP4 file. There is no decode in the
  24/7 path, so CPU/GPU cost is independent of resolution and AI is not yet a factor.
- **Decode happens only on demand and at the edges**: ffprobe in the indexer (cheap
  metadata read), single-frame MJPEG extraction for snapshots, and the keyframe-copy
  trim for clips. None of these run continuously.
- This honors the **memo §6.2** table: 24/7 recording and evidence export use the
  **main stream** (`record_stream` defaults to `main`), while **live preview and the
  snapshot live path prefer the sub-stream** (`stream_url(cam,"sub")` first, record
  URL as fallback). The per-stream choice is data-driven: `record_stream` selects
  which stream the recorder pulls; live view / live snapshot independently bias
  toward the lighter sub-stream.
- It also realizes the **memo §4.3 core principle** ("Raw continuous video stays
  local by default"): segments are written to the local `recordings_dir` and served
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
| GET | `/healthz` | Liveness `{status:"ok"}` |
| GET | `/api/v1/system` | Version, uptime, camera/segment counts, footprint vs cap |
| GET / POST | `/api/v1/cameras` | List / create cameras |
| GET / PATCH / DELETE | `/api/v1/cameras/{id}` | Read / partial update / delete (+stop recorder, +purge files) |
| GET / POST | `/api/v1/cameras/{id}/test` | Probe the record stream for reachability/codec/dims |
| GET | `/api/v1/cameras/{id}/segments` | Timeline index rows (with media URLs) |
| GET | `/api/v1/cameras/{id}/timeline` | Coalesced availability ranges |
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

All via `VISIONOPS_*` env vars (see `.env.example`). Notable defaults:

| Var | Default | Meaning |
|---|---|---|
| `VISIONOPS_DATABASE_URL` | `sqlite://./data/visionops.db` | SQLite only in Stage 0 |
| `VISIONOPS_DATA_DIR` / `RECORDINGS_DIR` / `CLIPS_DIR` / `SNAPSHOTS_DIR` | `./data` + subdirs | media roots |
| `VISIONOPS_FFMPEG_BIN` / `FFPROBE_BIN` | `ffmpeg` / `ffprobe` | external binaries |
| `VISIONOPS_MEDIAMTX_API_URL` | `http://127.0.0.1:9997` | gateway control API |
| `VISIONOPS_MEDIAMTX_HLS_BASE` / `RTSP_BASE` / `WEBRTC_BASE` | `:8888` / `rtsp://...:8554` / `:8889` | viewer URLs |
| `VISIONOPS_RECORDER_ENABLED` | `true` | master recorder switch |
| `VISIONOPS_DEFAULT_SEGMENT_SECONDS` | `60` | segment length |
| `VISIONOPS_DEFAULT_RETENTION_HOURS` | `24` | age policy |
| `VISIONOPS_INDEXER_INTERVAL_S` / `HEALTH_INTERVAL_S` / `RETENTION_INTERVAL_S` | `10` / `15` / `300` | loop cadences |
| `VISIONOPS_MAX_RECORDINGS_GB` | `20` | global size cap |
| `VISIONOPS_API_HOST` / `API_PORT` | `0.0.0.0` / `8000` | bind address |
| `VISIONOPS_CORS_ORIGINS` | `http://localhost:5173` | `*`/empty = allow all |

---

## 12. Stage 0 limitations and where they map onward

| Limitation (Stage 0, as built) | Why it's acceptable now | Where it's addressed |
|---|---|---|
| **SQLite only** — `db.rs` hard-bails on non-`sqlite` URLs | Single-node edge box; WAL handles the 8–16 camera target | SQLx is DB-agnostic; Postgres path planned (multi-node/cloud coordination, memo §4.2) |
| **Plaintext credentials** in `cameras.password` | Trusted single-tenant deploy; never serialized, always masked | Secret store / encryption (schema comment; security hardening stage) |
| **Keyframe-aligned clip cuts** (`-c copy`, no re-encode) | Preserves quality and is cheap; precision bounded by GOP | Frame-accurate trimming via optional re-encode in a later playback stage |
| **No auth on the API** | Local/LAN dev; CORS is the only gate | AuthN/AuthZ + tenant scoping (the `tenants`/`sites` tables already exist) — memo §14 Stage 1+ |
| **Audio dropped** (`-an`) | Video-first VMS; halves edge cases | Audio capture can be re-enabled when needed |
| **Evidence lock has no mutating API** (`locked`/`incident_id` columns only) | Retention already honors the flag | `lockEvidence(...)` endpoint (memo §5 Layer 3 playback API) |
| **No AI / frame sampler / decode pipeline** | Stage 0 is the media kernel; clean ingest/record/decode/infer separation already in place (memo §6.1) | Stage 2 frame sampler + Stage 3 detection kernel; `events`/`capabilities` schema pre-shaped |
| **`fps_observed` not populated; storage = byte cap only (no disk-throughput/SMART monitor)** | Bitrate + footprint cover sizing for now | Stage 1 "Observability and reliability": disk health monitor, stream metrics, service watchdog |
| **Single-node, raw video stays local** | Matches memo §4.3 core principle | Stage 1 edge offline buffer + cloud sync retry |

---

## 13. Background-task topology summary

```
 main()
   ├─ RecorderManager.start_all()
   │     └─ per camera: tokio::spawn supervise(id)   ── owns 1 ffmpeg child
   │                         writes  recordings_dir/<id>/<UTC strftime>.mp4
   │
   ├─ tokio::spawn indexer::run   (every ~10s)  scans dirs → segments rows, gaps
   ├─ tokio::spawn health::run    (every ~15s)  recording→error on staleness
   ├─ tokio::spawn retention::run (every ~300s) age purge + size-cap purge (skip locked)
   │
   └─ axum::serve(...)                          HTTP API + /media static files
         on SIGINT/SIGTERM → recorder.shutdown() → kill every ffmpeg child
```

All five concerns (1 supervisor-set + 3 loops + HTTP) share the single
`SqlitePool` and `Arc<Config>`; coordination between the recorder (writes files,
sets `connecting`/`recording`/`offline`) and the indexer (reads files, confirms
`recording`, computes bitrate) is entirely through the filesystem and the
`camera_status` row — there is no in-process channel between them, which keeps the
write path non-blocking.
