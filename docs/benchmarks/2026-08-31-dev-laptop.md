# Qualification attempt — 2026-08-31, developer laptop

**Nothing here qualifies a capacity claim.** Three profiles were run against the real recorder; two
failed thresholds and one was invalid. `docs/sizing.md` therefore still carries zero qualified rows,
and `scripts/verify_capacity_claims.py` is refusing to let any of these be cited.

That is the machinery working. It is also the most useful thing this run produced, so it is written
down rather than filed away: **this hardware is not a qualification host**, and the reasons are
specific.

---

## What was run

| Scenario | Cameras | Codec | Bitrate | AI | Duration | Result |
| --- | --- | --- | --- | --- | --- | --- |
| `qual-4cam-h264` | 4 | H.264 | 2 Mbps CBR | off | 30 min | **FAIL** — 1 threshold |
| `qual-8cam-h264` | 8 | H.264 | 2 Mbps CBR | off | 30 min | **FAIL** — 3 thresholds |
| `qual-16cam-h265` | 16 | H.265 | 4 Mbps CBR | off | 30 min | **INVALID** — fleet never came up |

Host: Apple M2, 8 cores, macOS. Release build of `heldar-core`, MediaMTX, ffmpeg 8.0.1.
Thresholds 1.1.0. Raw results in `results/`.

Every scenario injected the same four faults: a camera disconnect and reconnect, a MediaMTX restart,
and a core restart.

---

## Results

| Metric | Bar | 4 cam | 8 cam | 16 cam (invalid) |
| --- | --- | --- | --- | --- |
| `unexplained_gap_seconds_per_camera_hour` | ≤ 30 | **0.0** | 37.7 ✗ | 2749.6 |
| `unplayable_segment_count` | ≤ 0 | **0** | **0** | **0** |
| `recorder_reconnect_seconds_p95` | ≤ 60 | **30.1** | 230.5 ✗ | 1101.8 |
| `restart_recovery_seconds` | ≤ 120 | **9.0** | **9.3** | 88.2 |
| `liveview_failure_rate` | ≤ 0.01 | **0.0** | **0.0** | **0.0** |
| `liveview_seconds_p95` | ≤ 5.0 | 6.31 ✗ | 6.84 ✗ | 8.78 |
| `snapshot_failure_rate` | ≤ 0.02 | **0.0** | **0.0** | 0.41 |
| `snapshot_seconds_p95` | ≤ 10.0 | **7.48** | **7.81** | 20.63 |
| `clip_success_rate` | ≥ 0.99 | **1.0** | **1.0** | — |
| `clip_seconds_p95` | ≤ 30.0 | **3.58** | **3.04** | — |
| `api_5xx_rate` | ≤ 0.005 | **0.0** | **0.0** | **0.0** |
| `api_seconds_p95` | ≤ 2.0 | **0.275** | **0.252** | **0.515** |

Reported, not gated:

| | 4 cam | 8 cam | 16 cam |
| --- | --- | --- | --- |
| `time_to_first_segment_seconds` | 17.2 | 18.4 | never completed |
| `footage_lost_per_restart_seconds` | 3.0 | 4.4 | 87.5 |
| `mediamtx_recovery_seconds` | 2.2 | 29.1 | 65.9 |
| `core_cpu_percent_mean` | 1.1% | 1.7% | 0.6% |
| `core_rss_bytes_max` | 29 MB | 34 MB | 23 MB |
| `unindexed_segment_files` | 6 | 10 | 18 |

---

## Why the 16-camera run is INVALID rather than failed

The publishers delivered a median of 100% of the requested bitrate *when they delivered anything*,
but not every camera had written a segment by the end of the warm-up, so the run never reached its
declared fleet size. Load average sat at 263 on 8 cores while the recorder's own CPU stayed at 0.6%.

libx265 cannot encode sixteen 1080p CBR streams in real time on this host. The recorder was idle and
starved. Publishing "16 cameras FAIL" from that would have been a false product limit — the number
looks like bad news, which makes it easier to accept than it should be.

---

## The caveat that governs everything above

**The machine generating the streams is the machine being measured.** In production, cameras are
external devices; the box only receives and records. Here, one laptop runs 4–16 CBR encoders *and*
MediaMTX *and* the recorder *and* the live publishers. That generation load does not exist on a real
appliance.

Its effect is not uniform, which matters for reading the table:

- **Throughput metrics are trustworthy** where the workload was delivered. Coverage, playability,
  clip success, API error rate and the restart behaviour all reflect the recorder doing real work at
  the declared byte rate.
- **Latency metrics are contaminated.** `liveview_seconds_p95` is the binding failure at 4 cameras,
  and live-view cold start means spawning an ffmpeg to republish a stream — on a host already
  running four encoders, that contends. Typical cold start was 3.6–4.2 s at 4 cameras against a 5 s
  bar, with P95 at 6.31 s. Whether a dedicated appliance clears 5 s is **untested**, and this run
  cannot answer it.

The 8-camera failures sit on the line between the two: 37.7 s/camera-hour of unexplained gap and 25
reconnect events are real coverage loss, but on a host at load 101 they cannot be attributed to the
recorder with confidence.

Two further environmental facts, recorded rather than smoothed over:

- **The host's disk was 97% full** for every run. That is not a clean environment for a recorder, and
  it is the kind of condition that shows up in write latency before it shows up anywhere obvious.
- **Two of the three results record `git_dirty: true`.** The dirt is the sibling result files written
  by the preceding run in the same series; the tracked source was unchanged. Benign, and stated
  because provenance is only worth recording if it is also read.

---

## What would make a real qualification

1. **Generate the streams on a separate machine.** Everything above follows from this one constraint.
2. **A host with headroom**, so load average stays near the core count and latency measures the
   product.
3. **A disk with room**, so write latency is not a hidden variable.

Until then the sizing table stays as it is: every row `EXTRAPOLATED`, no row citing a run. The gate
enforces that, and it is currently refusing this document's own results — which is the behaviour it
was built for.

## What this run does establish

Not a capacity claim, but not nothing:

- The recorder wrote **zero unplayable segments** across all three runs, including the 16-camera run
  where it was starved and flapping — 81 segments ffprobed there, 73 and 76 in the others, sampled
  across the whole run and including every segment overlapping a restart.
- **Clip export succeeded 100%** of the time it had footage (149 graded exports across the two valid
  runs), with a P95 of 3.6 s against a 30 s bar.
- **No 5xx and no connection failure** in 243 control-plane calls, with an API P95 of 0.25–0.52 s.
- **Restart recovery was 9 s** at both 4 and 8 cameras — every camera writing new footage again,
  measured per camera rather than from a fleet-wide counter.
- A core restart costs **3.0–4.4 s of footage per camera** at those sizes
  ([#167](https://github.com/Straits-AI/heldar/issues/167)), rising to 87.5 s on the saturated host.
