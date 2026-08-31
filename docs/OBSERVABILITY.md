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
`services/webhooks.rs`, `services/retention.rs`, `services/health.rs`,
`services/indexer.rs`, `config.rs`, and `.env.example`.

Covers observability and reliability (faults visible,
recording gaps explainable, operable by a non-developer).

---

## 1. Probe & telemetry endpoints

All served by the same Axum process on `HELDAR_API_PORT` (default `8000`).

| Method | Path | Purpose | Status codes |
|---|---|---|---|
| GET | `/healthz` | **Liveness** — the process is up. No dependency checks. | `200` always (`{"status":"ok"}`) |
| GET | `/readyz` | **Readiness** — the SQLite store is reachable (runs `SELECT 1`). | `200 {"ready":true}` / `503 {"ready":false,"reason":"database"}` |
| GET | `/metrics` | **Prometheus** text exposition (system + per-camera gauges/counters). Needs `SystemRead` and a **fleet-wide** credential — see below. | `200`, `Content-Type: text/plain; version=0.0.4`; `403` for a camera-scoped credential |
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

> **Scrape with a fleet-wide credential.** When auth is enabled the scraper must present
> an API key with `SystemRead`, and that key must **not** be camera-scoped. The exposition
> carries `heldar_camera_up{camera=…}` (and the per-camera counters) for the whole fleet,
> so a camera-scoped key is refused with `403` rather than served a filtered body:
> Prometheus reads a series that stops appearing as a camera that ceased to exist and
> writes a staleness marker, so filtering would quietly corrupt fleet history with gaps
> indistinguishable from real outages. With auth disabled (the LAN-appliance default)
> `/metrics` is open, unchanged. See docs/ACCESS-CONTROL.md §4b.

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
| `heldar_ai_tasks_enabled` | gauge | — | AI tasks with `enabled = 1`. |
| `heldar_detections_stored` | gauge | — | Detections currently stored. A **gauge**, not a counter: the retention sweeper prunes old rows, so it can decrease. |
| `heldar_disk_total_bytes` | gauge | — | Total bytes on the recordings filesystem. *Omitted if statvfs fails.* |
| `heldar_disk_free_bytes` | gauge | — | Free bytes on the recordings filesystem (`f_bavail`). *Omitted if statvfs fails.* |
| `heldar_disk_used_percent` | gauge | — | Used percent of the recordings filesystem. *Omitted if statvfs fails.* |
| `heldar_camera_up` | gauge | `camera`, `state` | `1` when that camera's state is `recording`, else `0`. One series per camera. |
| `heldar_camera_reconnects_total` | counter | `camera` | Recorder reconnect count (from `camera_status.reconnect_count`). |
| `heldar_camera_segments_written_total` | counter | `camera` | Segments written by the recorder. |
| `heldar_camera_bitrate_kbps` | gauge | `camera` | Observed bitrate of the last indexed segment. *Only emitted when known.* |
| `heldar_camera_last_segment_age_seconds` | gauge | `camera` | Seconds since the last indexed segment. *Only emitted when a segment exists.* |

`scripts/check_documented_metrics.py` compares this table against `services/metrics.rs` in CI, in
both directions. A name that has drifted produces an alerting rule that matches nothing and
therefore never fires, which is the worst way for an alert to fail — it looks like health. This
table was wrong in four places before that check existed.

### What qualification needs and this does not export

The benchmark harness (`docs/benchmarks/README.md`) measures what it can from outside the process
and reports the rest as `unmeasured` rather than guessing. These are the gaps it runs into, listed
here because they are metric gaps, not harness gaps:

- **sampler effective FPS.** `heldar_detections_stored` is not a substitute: a sampler running at
  the requested rate that sees nothing stores nothing. Answering "is the AI keeping up" needs a
  sampler-side frames-processed counter.
- **retention sweep duration.** Bytes reclaimed is inferable from `heldar_recordings_bytes` falling;
  how long a sweep took, and whether it stalled recording, is not visible at all.
- **SQLite contention.** No busy/retry counter, so the externally visible proxy is the API 5xx rate,
  which is a superset and cannot separate contention from anything else.
- **request latency.** No histogram or summary, so P95 has to be measured by the client.

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
        expr: increase(heldar_camera_segments_written_total[10m]) == 0 and on(camera) heldar_camera_up == 1
        for: 10m
        labels: { severity: warning }
        annotations:
          summary: "Camera {{ $labels.camera }} wrote no segments in 10m (recording gap)"
```

---

## 3. Alerting webhooks (subscriptions)

`services/webhooks.rs` runs as a supervised background loop that delivers events to
external systems through **webhook subscriptions** — it supersedes the old
single-URL alert notifier.

- **Enable it** by creating a subscription: `POST /api/v1/webhooks` (or the
  dashboard's Webhooks panel) with a target `url`, an `event_types` filter (`["*"]`
  = all), a `min_severity` floor (e.g. `warning`), and an optional HMAC `secret`.
  No enabled subscriptions = the loop idles.
- **Poll cadence**: `HELDAR_NOTIFIER_INTERVAL_S` (default `15`, floored at 5s).
- **HTTP**: `POST` JSON with a 10-second client timeout; redirects are disabled so
  a target can't 302 the box to an internal URL.

### Payload shape (one POST per event)

```json
{
  "id":         "…",
  "camera_id":  "front-gate",
  "site_id":    "…",
  "event_type": "camera_offline",
  "severity":   "warning",
  "timestamp":  "2026-06-13T12:34:56Z",
  "payload":    { "ran_seconds": 3, "detail": "…" }
}
```

`camera_id` is `null` for system-wide events (e.g. disk pressure); `payload` is the
raw event payload object as logged. Each POST carries `X-Heldar-Event` /
`X-Heldar-Delivery` / `X-Heldar-Timestamp` headers and, when the subscription has a
secret, `X-Heldar-Signature: sha256=<hex HMAC-SHA256(secret, raw_body)>` over the
exact bytes sent.

### What gets delivered

Whatever the subscription's `event_types` + `min_severity` filters admit
(`GET /api/v1/events/types` lists the known types). With a `warning` floor, the
Stage 1 kernel events are:

| `event_type` | Severity | Emitted by |
|---|---|---|
| `camera_offline` | warning | recorder reconnect (`recorder.rs`) |
| `recorder_error` | warning | no-URL / staleness (`recorder.rs`, `health.rs`) |
| `recording_gap` | warning | indexer detects a >3 s hole (`indexer.rs`) |
| `disk_pressure` | warning | size-cap pruning / locked-exceeds-cap (`retention.rs`) |
| `disk_pressure` | critical | disk-free-floor pruning (`retention.rs`) |
| `retention_delete` | info | routine age-based cleanup (`retention.rs`) — below a `warning` floor |

### "Starts from now" + retry behavior

- **Starts from now:** a new subscription's cursor is initialized to `Utc::now()`,
  so history is **never replayed** — you only get events that occur after it is
  created. Each subscription keeps its own persisted cursor, so subscriptions
  advance independently.
- **On a failed delivery** (transport failure or non-2xx): the attempt is recorded
  in `webhook_deliveries` and the cursor stays on that event, so it is retried next
  cycle (at-least-once) — until the per-event attempts reach 5, after which the
  event is given up on and the cursor advances (a dead endpoint can't wedge the
  queue). `GET /api/v1/webhooks/{id}/deliveries` shows the ledger;
  `POST /api/v1/webhooks/{id}/test` sends a synthetic event.
- Each cycle drains in batches of 100, oldest-first.

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
webhooks** — are launched through `spawn_supervised` in `main.rs`. If a supervised
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
- **Get paged automatically?** create a webhook subscription (§3) and/or scrape
  `/metrics` with the rules in §2.

---

## 9. Relevant configuration

All `HELDAR_*` env vars (see `.env.example` / `config.rs`):

| Var | Default | Used by |
|---|---|---|
| `HELDAR_MAX_RECORDINGS_GB` | `20` | size cap (retention §6) |
| `HELDAR_MIN_FREE_DISK_GB` | `5` | disk-free floor (retention §6) |
| `HELDAR_NOTIFIER_INTERVAL_S` | `15` (min 5) | webhook-delivery poll cadence (subscriptions via `/api/v1/webhooks`, §3) |
| `HELDAR_RETENTION_INTERVAL_S` | `300` (min 30) | retention sweep cadence |
| `HELDAR_HEALTH_INTERVAL_S` | `15` (min 5) | staleness monitor cadence |
| `HELDAR_INDEXER_INTERVAL_S` | `10` (min 2) | indexer / gap-detect cadence |
| `HELDAR_RECORDINGS_DIR` | `./data/recordings` | filesystem that `statvfs` / disk metrics target |
| `HELDAR_API_HOST` / `HELDAR_API_PORT` | `0.0.0.0` / `8000` | where `/healthz`, `/readyz`, `/metrics` are served |
</content>
</invoke>
