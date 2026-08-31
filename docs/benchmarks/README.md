# Qualification benchmarks

`docs/sizing.md` gives formulas. Formulas are not limits. This directory holds what a real box
actually did under a declared workload, and the machinery that stops a capacity claim being
published without one.

The rule this exists to enforce: **"supports 8–16 cameras" is a measurement or it is a guess.** If it
is a guess, it says so.

---

## Running one

```bash
cargo build --release --workspace
python3 scripts/bench/harness.py list
python3 scripts/bench/harness.py run qual-8cam-h264 --hardware-class appliance-n100
```

The harness writes `docs/benchmarks/results/<run-id>.json` and exits `0` on PASS, `1` on FAIL. It
boots its own MediaMTX, its own core on its own ports, and its own synthetic cameras under
`/tmp/heldar-bench-<run-id>`, so it never touches a dev database or a dev stack.

A **release build is required**. A debug recorder is several times slower than the one an appliance
ships, so a number measured against it is not conservative — it is wrong, and wrong in a direction
that tells you nothing about the product. The harness refuses rather than substituting.

### Against a real fleet

```bash
HELDAR_URL=https://box.local HELDAR_TOKEN=… \
  python3 scripts/bench/harness.py run field-1h --hardware-class site-alpha
```

Field mode boots nothing, creates no camera, and **refuses to inject faults** — killing a process on
someone's production recorder is an outage, not a benchmark. Metrics that need filesystem or process
access (segment playability, core CPU/RSS) come back `unmeasured`, which is a fact about the mode,
not a pass.

A field run **cannot qualify a row in the sizing table**, and the gate refuses one that tries. That
is deliberate rather than a limitation: a real site's cameras are whatever the site has — mixed
codecs, mixed bitrates, changing between runs — so a field result describes *that site on that day*
and is not a controlled workload another buyer can plan against. Field mode answers "is this box
keeping up", which is a different and equally useful question.

### Re-checking a published result

```bash
python3 scripts/bench/harness.py verify docs/benchmarks/results/<run>.json
```

Recomputes the verdict from the raw measurements. The file's own `verdict` field is not trusted: a
result file is a text file, and a benchmark whose conclusion can be edited with a text editor is a
press release.

---

## How a result becomes a capacity claim

```
scripts/bench/thresholds.json      the bar, declared before the run
        │
        ├── hashed into every result
        │
docs/benchmarks/results/*.json     raw measurements + provenance + verdict
        │
docs/sizing.md  <!-- qualification-table -->
        │        each row: qualified by a named result, or labelled EXTRAPOLATED
        │
scripts/verify_capacity_claims.py  refuses anything else — runs in CI
```

`verify_capacity_claims.py` recomputes each cited run's verdict, checks the row's profile against
the run's scenario (16 cameras cannot be qualified by an 8-camera run), and **refuses a claim whose
run was judged against different thresholds from the ones in the tree today**. Loosening a bar to
turn a red run green therefore invalidates the claim it was loosened for, and the profile has to be
re-run. That is the mechanical form of the issue's instruction not to move a threshold after seeing
the result.

---

## What is measured, and what is not

Measured from outside the box, by doing the work an operator does:

| Metric | How |
| --- | --- |
| `unexplained_gap_seconds_per_camera_hour` | `/gaps` over the measurement window, minus intervals the harness itself took the camera off the air |
| `recording_gap_seconds_per_camera_hour` | the same, raw — what a dashboard would show |
| `unplayable_segment_count` | ffprobe over a sample of the segments **the index claims exist**, spread across the whole run, plus every segment overlapping a restart |
| `unindexed_segment_files` | files on disk the indexer rejected (diagnostic, no bar) |
| `recorder_reconnect_seconds_p50/p95` | `heldar_camera_up` transitions, at sampling resolution |
| `restart_recovery_seconds` | until **every** camera has written a new segment (per-camera counter); a timeout is `unmeasured`, never the deadline |
| `mediamtx_recovery_seconds` | the same, for a stream-server restart. Reported, **not gated** |
| `footage_lost_per_restart_seconds` | `/gaps` across the restart window |
| `time_to_first_segment_seconds` | warm-up, reported rather than charged as a gap |
| `liveview_*`, `snapshot_*`, `clip_*` | real requests, timed |
| `api_5xx_rate`, `api_seconds_p95` | control-plane calls made inside the measurement window — setup traffic and the three media paths are excluded and counted separately |
| `core_cpu_percent_mean`, `core_rss_bytes_max` | `ps` on the core process |
| `disk_used_percent_max` | the peak of the exposition's disk gauge over the run |
| `retention_bytes_reclaimed` | decreases in `heldar_recordings_bytes` |
| `ai_detections_per_second` | AI profiles only — a **floor** on throughput, not effective FPS (see below) |

**Not measured, and reported as `unmeasured` rather than assumed:**

- `event_ingest_latency_seconds_p95` — no event producer is driven, and the kernel exposes no
  ingest histogram.
- `sqlite_busy_rate` — the kernel has no busy counter. `api_5xx_rate` is the externally visible
  superset.
- `retention_sweep_seconds` — the sweeper emits no duration metric; only bytes reclaimed is visible
  from outside.
- `ai_fps_effective_ratio` — the kernel exposes *stored detections*, not sampler frame rate. A
  sampler that runs at the requested rate and sees nothing stores nothing, so detections cannot
  stand in for effective FPS. **This needs a sampler-side counter that does not exist yet.**
- `worker_lease_churn`, `gpu_utilisation_percent`, `disk_iops`, `network_throughput_bytes`.

A threshold over an `unmeasured` metric **fails**. That is the same rule the security posture uses —
`unknown` is not a pass — and it exists so that adding a bar for something nothing measures breaks
every run loudly instead of passing quietly.

---

## Reading a result honestly

**The gap window is not the run window.** `/gaps` derives coverage from the segment index at query
time, and the retention sweeper has already deleted rows older than the camera's retention. Asking
for gaps over a 24-hour run with 6-hour retention returns roughly 18 hours of "gap" — the retention
policy working exactly as configured, reported as lost coverage. The harness therefore clamps the
gap query to 80% of the retention horizon and records the window it actually used alongside the
number (`measurements.*.window`). A run whose in-retention window is under two minutes reports the
gap metrics as `unmeasured` rather than computing a ratio from almost nothing.

**A percentile over ten samples is not a percentile.** Every measurement carries its `n`; the short
scenarios produce single-digit `n` for the media probes and their P95 is simply the worst of a few.
Check `n` before quoting a number. The raw probe rows are in the result under `probes`.

**Faults the harness injected are subtracted, and the subtraction is auditable.** `injected_outages`
lists every interval the harness had a camera or the fleet down, and each probe row carries
`excluded_during_injected_outage`. Without this, a fault-injection scenario would fail its own
coverage threshold by construction — and the tempting fix would be to loosen the threshold, which
is the move the whole mechanism exists to prevent.

**Synthetic throughput is not model accuracy.** These scenarios publish `testsrc`. They measure
whether the box can ingest, record, index, export and survive faults at a given load. They say
**nothing** about whether ANPR reads a plate or whether a ReID embedding matches the right person.
No result here may be cited for detection accuracy.

**A hardware class is part of the claim.** A run on a developer laptop qualifies a developer laptop.
`--hardware-class` is recorded in the result and checked against the sizing row, so a laptop run
cannot silently qualify an appliance profile.

---

## The scenario matrix

`scripts/bench/scenarios.json`. Every scenario except `field-1h` injects the same four faults — a
camera disconnect and reconnect, a MediaMTX restart and a core restart — because a threshold over
reconnect or restart recovery is `unmeasured`, and therefore failing, in a scenario that never
breaks anything. The two long profiles add a retention squeeze.

| Scenario | Shape |
| --- | --- |
| `smoke-2cam` | 2 × 360p, 7 min, all four process faults. Shape check for CI. **Not a capacity claim** |
| `qual-4cam-h264` | 4 × 720p H.264 @ 2 Mbps, 30 min |
| `qual-8cam-h264` | 8 × 720p H.264 @ 2 Mbps, 30 min |
| `qual-8cam-motion-ai` | as above with the motion sampler at 2 FPS |
| `qual-16cam-h265` | 16 × 1080p H.265 @ 4 Mbps, 30 min |
| `qual-32cam-h265` | 32 × 1080p H.265 @ 4 Mbps, 30 min |
| `rc-24h-8cam` | the release-candidate run, with a retention squeeze |
| `soak-7d-8cam` | the seven-day soak for the recommended appliance profile |
| `field-1h` | a real fleet, no faults |

Per-scenario knobs beyond the obvious ones: `warmup_s` (default 120) is excluded from the
measurement window and reported as `time_to_first_segment_seconds` instead; `probe_cameras`
(default 8) bounds how many cameras each probe round touches, rotating through the fleet — a round
costs roughly ten seconds per camera, so probing all 32 would take longer than that scenario's
probe interval and the loop would spend the whole run probing.

> **The host has to encode as well as record.** A 32-camera synthetic run needs the machine to run 32
> ffmpeg encoders *and* the recorder. On a host that cannot, the publishers starve and the run
> measures the generator, not the product. Above roughly 8–16 streams, generate on a separate
> machine or use a hardware encoder, and treat a single-host high-count result as suspect.

---

## Runs

| Date | Host | Outcome |
| --- | --- | --- |
| [2026-08-31](2026-08-31-dev-laptop.md) | Apple M2, 8 cores | 4-cam and 8-cam FAIL, 16-cam INVALID. **Nothing qualified.** The generator shared the host with the box, which contaminates every latency metric. |

## Known findings

Recorded here because a benchmark that finds nothing has usually not looked.

- **A core restart used to cost footage — fixed.** The recorder killed FFmpeg with SIGKILL, which
  left the in-progress segment truncated; the indexer correctly rejected it, so the timeline never
  lied, but the captured seconds were gone. Measured here at 3.0–4.4 s per camera per restart
  ([#167](https://github.com/Straits-AI/heldar/issues/167)). FFmpeg is now asked to close the segment
  first. `footage_lost_per_restart_seconds` is still reported, and a future run on a fixed box should
  show it near one keyframe interval rather than half a segment — which is the number to check when
  re-qualifying.
- **`GET /api/v1/cameras/{id}/snapshot` returns 500 for a camera whose stream is down**, where a
  4xx/503 would be right — an upstream that is not there is not an internal error. See
  [#168](https://github.com/Straits-AI/heldar/issues/168).
- **First live view of a camera can take the full 8-second ready-wait**
  (`services/mediamtx.rs`): the call returns URLs on timeout by design, so it is a latency
  characteristic rather than a failure, but any live-view bar below 8 s is unreachable when the
  wait times out.
