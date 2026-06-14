# Heldar Core — Observability & Reliability (Stage 1)

Operator / SRE guide to running Heldar Core unattended: how to check that it is
alive, how to scrape its metrics, how to get paged when a camera or disk goes bad,
and how recordings are kept inside their storage budget without ever deleting
evidence.

This document is **grounded in the code as built** (`crates/heldar-kernel/src`). Endpoint
shapes, metric names, event types, and env vars below are the real ones — if a
field or metric is not listed here, it is not emitted. The authoritative sources
are: `routes/health.rs`, `routes/system.rs`, `routes/recordings.rs`,
`routes/metrics.rs`, `services/storage.rs`, `services/metrics.rs`,
`services/notifier.rs`, `services/retention.rs`, `services/health.rs`,
`services/indexer.rs`, `config.rs`, and `.env.example`.

Maps to **memo §14 "Stage 1 — Observability and reliability"** (faults visible,
recording gaps explainable, operable by a non-developer).

---

## 1. Probe & telemetry endpoints

All served by the same Axum process on `HELDAR_API_PORT` (default `8000`).

| Method | Path | Purpose | Status codes |
|---|---|---|---|
| GET | `/healthz` | **Liveness** — the process is up. No dependency checks. | `200` always (`{"status":"ok"}`) |
| GET | `/readyz` | **Readiness** — the SQLite store is reachable (runs `SELECT 1`). | `200 {"ready":true}` / `503 {"ready":false,"reason":"database"}` |
| GET | `/metrics` | **Prometheus** text exposition (system + per-camera gauges/counters). | `200`, `Content-Type: text/plain; version=0.0.4` |
| GET | `/api/v1/system` | System info incl. the **`storage`** block (disk + footprint + projection). | `200` |
| GET | `/api/v1/cameras/{id}/gaps?from&to` | Recording-coverage **gaps** for a camera/time window. | `200` (404 if camera unknown) |
| GET | `/api/v1/health/cameras` | Per-camera live status (`CameraStatus[]`). | `200` |
| GET | `/api/v1/cameras/{id}/health` | One camera's status. | `200` (404 if no status row) |
| GET | `/api/v1/events?camera_id&event_type&severity&limit` | Event log (newest first, `limit` ≤ 2000, default 200). | `200` |

### Liveness vs readiness

- **`/healthz`** answers "is the process running?" — use it for container/process
  liveness. It does **not** touch the database, so it stays `200` even if SQLite is
  wedged (by design — a wedged DB should not trigger a kill-and-restart loop).
- **`/readyz`** answers "can it serve work?" — it executes `SELECT 1` against the
  pool and returns `503` if that fails. Use it as a load-balancer / orchestrator
  readiness gate.

### The `/api/v1/system` storage block

`GET /api/v1/system` returns the Stage 0 fields plus a `storage` object computed by
`services::storage::storage_report` (`storage.rs`):

```json
{
  "name": "Heldar Core",
  "version": "…",
  "uptime_seconds": 1234,
  "recordings_bytes": 10737418240,
  "max_recordings_gb": 20.0,
  "storage": {
    "disk": {
      "total_bytes": 500107862016,
      "free_bytes":  123456789012,
      "used_bytes":  376651073004,
      "used_percent": 75.3
    },
    "recordings_bytes":      10737418240,
    "segment_count":         17280,
    "oldest_segment":        "2026-06-12T00:00:00Z",
    "newest_segment":        "2026-06-13T00:00:00Z",
    "write_rate_bytes_per_day": 10200547328,
    "projected_days_remaining":  12.1
  }
}
```

| Field | Source | Meaning |
|---|---|---|
| `disk` | `statvfs(HELDAR_RECORDINGS_DIR)` | Filesystem totals, or **`null`** if statvfs fails. `free_bytes` is `f_bavail` (space usable by a non-root user — what we can actually write). |
| `recordings_bytes` | `SUM(size_bytes)` over `segments` | Indexed recording footprint (not raw disk usage). |
| `segment_count` | `COUNT(*)` over `segments` | Number of indexed segments. |
| `oldest_segment` / `newest_segment` | `MIN(start_time)` / `MAX(end_time)` | Coverage window of the index. |
| `write_rate_bytes_per_day` | `SUM(size_bytes)` of segments indexed (`created_at`) in the **last 24 h** | Recent write rate, not a long-run average. `0` when idle. |
| `projected_days_remaining` | `free_bytes / write_rate_bytes_per_day` | Days of free space left at the recent rate. **`null`** when `disk` is null or the write rate is `0`. |

> `projected_days_remaining` is a *free-disk* horizon, not a retention horizon —
> it ignores the size cap and the fact that retention recycles old segments. It is
> a "how long until the disk fills if nothing is pruned" estimate.

---

## 2. Prometheus metrics

`GET /metrics` renders the exposition below from `services/metrics.rs`. These are
the **only** metrics exported — there is no histogram/summary, and (note) **no fps
metric** on `/metrics` (observed fps is available per-camera via the health API,
§5).

| Metric | Type | Labels | Description |
|---|---|---|---|
| `heldar_build_info` | gauge | `version` | Always `1`; carries the build version label. |
| `heldar_cameras_total` | gauge | — | Registered cameras. |
| `heldar_cameras_recording` | gauge | — | Cameras whose status row is `state = 'recording'`. |
| `heldar_segments_total` | gauge | — | Indexed recording segments. |
| `heldar_recordings_bytes` | gauge | — | Total bytes of recorded segments (`SUM(size_bytes)`). |
| `heldar_disk_total_bytes` | gauge | — | Total bytes on the recordings filesystem. *Omitted if statvfs fails.* |
| `heldar_disk_free_bytes` | gauge | — | Free bytes on the recordings filesystem (`f_bavail`). *Omitted if statvfs fails.* |
| `heldar_disk_used_percent` | gauge | — | Used percent of the recordings filesystem. *Omitted if statvfs fails.* |
| `heldar_camera_up` | gauge | `camera`, `state` | `1` when that camera's state is `recording`, else `0`. One series per camera. |
| `heldar_camera_reconnects_total` | counter | `camera` | Recorder reconnect count (from `camera_status.reconnect_count`). |
| `heldar_camera_segments_written` | counter | `camera` | Segments written by the recorder. |
| `heldar_camera_bitrate_kbps` | gauge | `camera` | Observed bitrate of the last indexed segment. *Only emitted when known.* |
| `heldar_camera_last_segment_age_seconds` | gauge | `camera` | Seconds since the last indexed segment. *Only emitted when a segment exists.* |

The disk gauges are conditional on `statvfs` succeeding for
`HELDAR_RECORDINGS_DIR`; the per-camera bitrate / last-segment-age gauges are
conditional on those values being present. Alerting rules must tolerate the series
being absent (use `absent()` or `unless`, or alert on the camera-up signal).

### Sample scrape config

```yaml
# prometheus.yml
scrape_configs:
  - job_name: heldar-core
    metrics_path: /metrics
    scrape_interval: 30s
    static_configs:
      - targets: ['127.0.0.1:8000']   # HELDAR_API_HOST:HELDAR_API_PORT
        labels:
          site: edge-1
```

### Suggested alerting rules

These reference only metrics that actually exist. Recording **gaps** are primarily
delivered as events over the webhook (§3) and queried via the gaps endpoint (§4);
the Prometheus proxy below is "segments stopped advancing while the camera claims
to be up".

```yaml
# alerts.yml
groups:
  - name: heldar
    rules:
      # 1) Camera down — recorder not in the 'recording' state for 5 minutes.
      - alert: HeldarCameraDown
        expr: heldar_camera_up == 0
        for: 5m
        labels: { severity: warning }
        annotations:
          summary: "Camera {{ $labels.camera }} is not recording (state={{ $labels.state }})"

      # 2) Disk low — recordings filesystem under 10% free.
      - alert: HeldarDiskLow
        expr: heldar_disk_used_percent > 90
        for: 10m
        labels: { severity: critical }
        annotations:
          summary: "Recordings disk over 90% used"

      # 3) Stale segments — a recording camera that hasn't produced a segment in 3 min.
      - alert: HeldarStaleSegments
        expr: heldar_camera_last_segment_age_seconds > 180 and on(camera) heldar_camera_up == 1
        for: 2m
        labels: { severity: warning }
        annotations:
          summary: "Camera {{ $labels.camera }} stalled: no new segment in >3m"

      # 4) Recording gap proxy — segment counter flat while the camera is up.
      - alert: HeldarNoSegmentProgress
        expr: increase(heldar_camera_segments_written[10m]) == 0 and on(camera) heldar_camera_up == 1
        for: 10m
        labels: { severity: warning }
        annotations:
          summary: "Camera {{ $labels.camera }} wrote no segments in 10m (recording gap)"
```

---

## 3. Alerting webhook (notifier)

`services/notifier.rs` runs as a supervised background loop that pushes
**warning/critical events** to an external webhook as they happen.

- **Enable it** by setting `HELDAR_ALERT_WEBHOOK_URL`. If unset/blank, the
  notifier logs `alerting disabled` and is a no-op.
- **Poll cadence**: `HELDAR_NOTIFIER_INTERVAL_S` (default `15`, floored at 5s).
- **HTTP**: `POST` JSON with a 10-second client timeout.

### Payload shape (one POST per event)

```json
{
  "source":     "heldar-core",
  "event_id":   "…",
  "event_type": "camera_offline",
  "severity":   "warning",
  "camera_id":  "front-gate",
  "timestamp":  "2026-06-13T12:34:56Z",
  "payload":    { "ran_seconds": 3, "detail": "…" }
}
```

`camera_id` is `null` for system-wide events (e.g. disk pressure); `payload` is the
raw event payload object as logged.

### What gets delivered

Only events with **`severity IN ('warning', 'critical')`** are delivered (query in
`deliver()`). In the current code that is:

| `event_type` | Severity | Delivered? | Emitted by |
|---|---|---|---|
| `camera_offline` | warning | ✅ | recorder reconnect (`recorder.rs`) |
| `recorder_error` | warning | ✅ | no-URL / staleness (`recorder.rs`, `health.rs`) |
| `recording_gap` | warning | ✅ | indexer detects a >3 s hole (`indexer.rs`) |
| `disk_pressure` | warning | ✅ | size-cap pruning / locked-exceeds-cap (`retention.rs`) |
| `disk_pressure` | critical | ✅ | disk-free-floor pruning (`retention.rs`) |
| `retention_delete` | info | ❌ | routine age-based cleanup (`retention.rs`) |

### "Starts from now" + retry behavior

- **Starts from now:** the delivery cursor is initialized to `Utc::now()` at
  startup, so the notifier **never replays history** when the process boots — you
  only get events that occur after start-up.
- **On transport failure** (no HTTP response — connection refused, timeout): the
  cursor is **not** advanced and the loop `return`s, so the failed event *and any
  after it* are retried on the next poll cycle. This is at-least-once for
  unreachable endpoints.
- **On a non-2xx response** (the endpoint answered but rejected): it is logged as a
  warning and the cursor **advances** — that event is *not* retried. Make sure your
  receiver returns 2xx on accept.
- Each cycle delivers up to 100 events, oldest-first.

---

## 4. Recording gap detection

A "gap" is a hole in recording coverage. There are two complementary surfaces:

**1. Event-driven (live), `services/indexer.rs`.** Each time the indexer adds a new
segment it compares its `start_time` to the previous segment's `end_time`; if the
hole is **> 3 s** it logs a `recording_gap` (warning) event with
`{ gap_seconds, prev_end, next_start }`. These flow to `/api/v1/events` and (being a
warning) to the webhook.

**2. On-demand (historical), `GET /api/v1/cameras/{id}/gaps?from&to`.** Coalesces
the camera's indexed segments into availability ranges (segments closer than the
2 s tolerance are treated as contiguous) and reports the spans between ranges:

```json
{
  "camera_id": "front-gate",
  "from": "2026-06-13T00:00:00Z",
  "to":   "2026-06-13T01:00:00Z",
  "gaps": [ { "start": "…T00:10:00Z", "end": "…T00:10:42Z", "seconds": 42.0 } ],
  "gap_count": 1,
  "total_gap_seconds": 42.0
}
```

`from` / `to` are optional (RFC 3339); each side is open-ended if omitted. Only
holes larger than the 2 s coalescing tolerance are reported. Pair this endpoint
with a `recording_gap` event or a `HeldarNoSegmentProgress` alert to answer
"*why* is there a gap" by cross-referencing `camera_offline` / `recorder_error`
events over the same window.

---

## 5. Per-camera observed fps & bitrate

The indexer derives stream metrics from each freshly indexed segment
(`indexer.rs` → `repo::record_segment_indexed`):

- `bitrate_kbps = size_bytes * 8 / duration_s / 1000`
- `fps_observed` = the frame rate reported by `ffprobe` on that segment

Both are stored on the camera's `camera_status` row and reflect the **most recent
indexed segment** (they are overwritten each time — this is a last-value, not a
rolling trend). Read them via `GET /api/v1/health/cameras` /
`GET /api/v1/cameras/{id}/health` (`CameraStatus`):

```
camera_id, state, last_segment_at, last_started_at, reconnect_count,
segments_written, fps_observed, bitrate_kbps, last_error, recorder_pid, updated_at
```

States: `recording`, `connecting`, `offline`, `error`, `disabled`. Only
`bitrate_kbps` is mirrored to Prometheus (`heldar_camera_bitrate_kbps`);
`fps_observed` is health-API-only.

---

## 6. Storage management & retention

Two independent ceilings protect storage, on top of per-camera age policy. The
retention sweeper (`services/retention.rs`) runs every
`HELDAR_RETENTION_INTERVAL_S` (default `300`, floored at 30 s) and applies three
phases **in order**:

1. **Age policy (per camera).** Deletes *unlocked* segments whose `end_time` is
   older than the camera's `retention_hours`. Logs `retention_delete` (info).
2. **Global size cap — `HELDAR_MAX_RECORDINGS_GB`** (default `20`). A *soft cap*
   on total recording footprint. The deletable budget is
   `max_recordings_bytes − locked_bytes`; the oldest *unlocked* segments (by
   `end_time`, in batches of 20) are pruned until the unlocked footprint fits the
   budget. Logs `disk_pressure` (warning).
3. **Disk-free floor — `HELDAR_MIN_FREE_DISK_GB`** (default `5`). A *hard floor*
   on free space on the recordings filesystem (measured with `statvfs`). While
   free space is below the floor, the oldest *unlocked* segments are pruned (batches
   of 20, capped at 200 iterations per sweep) until back above it. Logs
   `disk_pressure` (critical).

### Size cap vs disk-free floor

| | Size cap (`MAX_RECORDINGS_GB`) | Free-disk floor (`MIN_FREE_DISK_GB`) |
|---|---|---|
| Measures | This app's recording footprint (`SUM(size_bytes)`) | Free space on the whole filesystem (`statvfs f_bavail`) |
| Kind | Soft cap (footprint budget) | Hard floor (host protection) |
| Triggers when | Unlocked footprint exceeds `cap − locked_bytes` | Free disk drops below the floor |
| Severity | `disk_pressure` / warning | `disk_pressure` / critical |
| Protects against | Heldar hoarding disk | Anything (incl. other apps) filling the disk and breaking recording |

Run both: the size cap keeps Heldar inside its own budget; the floor is a
backstop that fires regardless of the cap if the underlying disk gets tight (e.g.
something else on the box consumed space).

### Locked / evidence guarantee

**Locked segments are never deleted by any phase.** Every delete query filters
`locked = 0`, so a segment with `locked = 1` (evidence) survives age expiry, the
size cap, and the disk-free floor.

To keep this guarantee from wiping everything, the size-cap budget *subtracts*
locked bytes (`budget = cap − locked_bytes`) so locked footage does not force the
deletion of all unlocked footage. If **locked footage alone meets or exceeds the
cap** (`budget ≤ 0`), the sweeper does **not** delete unlocked footage — it logs a
`disk_pressure` (warning) `locked_exceeds_cap` event instead. Likewise, if the disk
is below the free floor but **no unlocked segments remain to prune**, it logs a
warning and stops rather than touching evidence. In short: evidence always wins;
when evidence is the cause of pressure, the operator is told rather than silently
losing data.

---

## 7. Background-task supervision

The four observability/reliability loops — **indexer, health, retention,
notifier** — are launched through `spawn_supervised` in `main.rs`. If a supervised
task **returns or panics**, the supervisor logs the cause and **respawns it after a
5 s delay**; if it is cancelled (graceful shutdown) it stops cleanly. The `run()`
loops are infinite by design, so a return/panic is treated as a fault and the task
is brought back automatically — a single bad cycle (e.g. a transient DB error) does
not permanently take down metrics, alerting, or retention.

The per-camera recorders are supervised separately by `RecorderManager` (reconnect
with exponential backoff, up to 30 s; each reconnect bumps `reconnect_count` and
logs a `camera_offline` event).

---

## 8. Quick operator checklist

- **Is it alive?** `curl -fsS localhost:8000/healthz`
- **Can it serve?** `curl -fsS localhost:8000/readyz` (503 ⇒ DB problem)
- **Disk headroom?** `GET /api/v1/system` → `storage.disk.used_percent`,
  `storage.projected_days_remaining`
- **Any camera unhealthy?** `GET /api/v1/health/cameras` → `state`, `last_error`,
  `reconnect_count`, `last_segment_at`
- **Recent faults?** `GET /api/v1/events?severity=warning` (or `critical`)
- **Coverage holes?** `GET /api/v1/cameras/{id}/gaps?from=…&to=…`
- **Get paged automatically?** set `HELDAR_ALERT_WEBHOOK_URL` and/or scrape
  `/metrics` with the rules in §2.

---

## 9. Relevant configuration

All `HELDAR_*` env vars (see `.env.example` / `config.rs`):

| Var | Default | Used by |
|---|---|---|
| `HELDAR_MAX_RECORDINGS_GB` | `20` | size cap (retention §6) |
| `HELDAR_MIN_FREE_DISK_GB` | `5` | disk-free floor (retention §6) |
| `HELDAR_ALERT_WEBHOOK_URL` | *(unset)* | notifier — unset disables alerting (§3) |
| `HELDAR_NOTIFIER_INTERVAL_S` | `15` (min 5) | notifier poll cadence |
| `HELDAR_RETENTION_INTERVAL_S` | `300` (min 30) | retention sweep cadence |
| `HELDAR_HEALTH_INTERVAL_S` | `15` (min 5) | staleness monitor cadence |
| `HELDAR_INDEXER_INTERVAL_S` | `10` (min 2) | indexer / gap-detect cadence |
| `HELDAR_RECORDINGS_DIR` | `./data/recordings` | filesystem that `statvfs` / disk metrics target |
| `HELDAR_API_HOST` / `HELDAR_API_PORT` | `0.0.0.0` / `8000` | where `/healthz`, `/readyz`, `/metrics` are served |
</content>
</invoke>
