# Heldar Core — Roadmap

> **Thesis:** Camera streams become structured events → events become workflows → workflows become operational intelligence.
> We build the **media kernel first**, then AI as plugins on top, then vertical apps. The long arc is to turn continuous video into a **compressed, queryable, verifiable world memory** of a physical space — so analytical intent can be defined *after* collection, not before.

> **Status (2026-06):** Stages **0–7 are all shipped (✅ DONE)** — the media kernel, observability, the AI frame sampler, detection/tracking/zones, Access Control, BakerySense, Movement intelligence, and Semantic search. What remains is the research frontier below (Level 4–5) and the per-stage accuracy benchmarking gated on local footage.

---

## Two roadmaps, one product

There are two threads to one plan. The **systems/vertical** thread owns the VMS, then ships Entry / Retail / Security apps. The **representation/intelligence** thread runs event memory → scene graph → semantic search → world model. They are the same product viewed from two ends:

```
systems           Stage 0 ── 1 ── 2 ── 3 ──── 4/5 ──── 6 ──────── 7
                  kernel  obs  sampler det/track  apps    ReID    semantic search
                    │      │     │      │          │       │           │
maturity ladder    L1 ───────────────── L2 ────── L2/L3 ── L3 ──── L3→L4→L5
                  task     event memory   scene/event graph    world memory
```

- **Stage 0–2** = build the substrate (Level 1 plumbing).
- **Stage 3** = events + scene/event graph (**Level 2 → 3**).
- **Stage 7** = semantic/causal query (**Level 3 → 4**).
- **The research frontier** = predictive bounded **world model** (**Level 5**, not solved).

Maturity ladder: **L1** task-specific analytics (industry baseline) · **L2** event memory (buildable now, MVP target) · **L3** scene/event graph (buildable with engineering, the differentiator) · **L4** AI-native latent world memory (research frontier, the moat) · **L5** general physical world model (not solved).

---

## ✅ Stage 0 — Media kernel MVP  — **DONE**

Goal: *own the base VMS.* Record compressed packets without decode; index, play back, export, and keep cameras healthy. Built in `crates/heldar-kernel` (Rust / Axum / Tokio / SQLx-SQLite) with MediaMTX + FFmpeg as the media engine.

**Shipped checklist:**

- [x] **Camera registry** — `tenants → sites → cameras` schema; CRUD API (`/api/v1/cameras`), vendor RTSP-URL templating + explicit override, main/sub stream + record-stream selection, capabilities JSON, connection test endpoint. (`routes/cameras.rs`, `camera_url.rs`, `migrations/0001_init.sql`)
- [x] **RTSP ingest + recording** — per-camera recorder writing **compressed segments (no re-encode)**, configurable `segment_seconds`, reconnect/restart supervision. (`services/recorder.rs`)
- [x] **Timeline index** — one `segments` row per file (start/end/duration/codec/size, indexed by camera+time); segment list + timeline API. (`services/indexer.rs`, `routes/recordings.rs`)
- [x] **Playback** — segment listing + timeline for a camera/time range; live + recorded delivery via MediaMTX. (`routes/playback.rs`, `routes/recordings.rs`)
- [x] **Clip export** — MP4 export for a camera/time window. (`routes/playback.rs` → `services/clip.rs`)
- [x] **Snapshot** — frame extraction at a timestamp. (`routes/playback.rs` → `services/snapshot.rs`)
- [x] **Live view** — brokered through MediaMTX gateway (HLS / WebRTC / RTSP URLs; camera credentials never exposed to the browser). (`routes/liveview.rs`, `services/mediamtx.rs`)
- [x] **Camera health** — per-camera status (state, last segment, reconnect count, segments written, observed fps/bitrate, last error) + lifecycle event log; health + events API. (`services/health.rs`, `routes/health.rs`, `camera_status`/`events` tables)
- [x] **Retention** — per-camera age policy + global size cap sweeper; **evidence-lock** (`locked` segments never deleted); retention/disk events logged. (`services/retention.rs`)
- [x] **System surface** — `/healthz`, `/api/v1/system` info; web frontend scaffolded (React + Vite + TS in `apps/web`).

**Stage 0 success criteria:**

| Success criterion | Status | Backed by |
|---|---|---|
| 8–16 cameras | ✅ multi-camera registry + per-camera recorder supervision | `recorder.rs`, validation run |
| 7 days continuous operation | ✅ reconnect/watchdog + retention keep it running unattended | `recorder.rs`, `retention.rs` |
| Recording playable | ✅ timeline index + segment/playback API + MediaMTX | `indexer.rs`, `routes/recordings.rs` |
| Clip export works | ✅ MP4 export endpoint | `routes/playback.rs`, `clip.rs` |
| Camera reconnect works | ✅ reconnect tracked in `camera_status`, surfaced via health/events | `recorder.rs`, `health.rs` |

> Maps to **Level 1** (the raw substrate) — the prerequisite for everything above it. No AI yet, by design.

---

## ✅ Stage 1 — Observability & reliability  — **DONE**

**Goal:** the system is operable by a non-developer; faults are visible; recording gaps are explainable. Built on the Stage 0 kernel with no new tables — everything is computed over `segments` / `camera_status` / `events` or read live from the OS. Operator/SRE guide: [`docs/OBSERVABILITY.md`](docs/OBSERVABILITY.md); implementation: `ARCHITECTURE.md` §14.

**Shipped checklist:**

- [x] **Recording gap detector** — `recording_gap` (warning) events emitted by the indexer when consecutive segments are >3 s apart, **plus** an on-demand `GET /api/v1/cameras/{id}/gaps?from&to` that reports the holes between coalesced availability ranges. (`services/indexer.rs`, `routes/recordings.rs`)
- [x] **Stream metrics** — observed `fps_observed` + `bitrate_kbps` computed per indexed segment and stored on `camera_status`, surfaced via the health API and (bitrate) Prometheus. (`services/indexer.rs`, `repo.rs`, `routes/health.rs`)
- [x] **Disk / storage health monitor** — `statvfs` free-space, recordings footprint, recent write rate, and free-disk-fill projection in the `/api/v1/system` `storage` block; `disk_pressure` events on pressure. (`services/storage.rs`, `routes/system.rs`)
- [x] **Prometheus metrics + liveness/readiness** — `GET /metrics` (system + per-camera gauges/counters), `GET /healthz` (liveness), `GET /readyz` (readiness, 200/503 on DB reachability). (`services/metrics.rs`, `routes/metrics.rs`, `routes/health.rs`)
- [x] **Alerting** — `HELDAR_ALERT_WEBHOOK_URL` notifier POSTs warning/critical events as JSON; starts-from-now (no replay on boot), retries on transport failure. (`services/notifier.rs`)
- [x] **Disk-free retention floor** — `HELDAR_MIN_FREE_DISK_GB` hard floor prunes oldest *unlocked* segments when the filesystem gets tight, on top of the age policy + `HELDAR_MAX_RECORDINGS_GB` size cap; evidence-lock honored throughout. (`services/retention.rs`)
- [x] **Service watchdog / auto-restart** — `spawn_supervised` respawns the indexer / health / retention / notifier loops 5 s after any return or panic. (`main.rs`)

**Deferred (rolls into later edge/cloud work):**

- [ ] Per-camera health **dashboard** UI (the health/events/metrics APIs exist; the web frontend view is still pending — `apps/web`)
- [ ] Edge offline buffer + cloud sync retry (the webhook notifier is the first upstream alert path; full store-and-forward sync remains planned)
- [ ] Packet-loss / throughput **trends** (current fps/bitrate are last-value, not time-series; trend storage is future work)

**Stage 1 success criteria:**

| Success criterion | Status | Backed by |
|---|---|---|
| System operable by a non-developer | ✅ health/system/events/metrics APIs + webhook alerts surface state without log-diving | `routes/health.rs`, `routes/system.rs`, `routes/metrics.rs`, `services/notifier.rs` |
| Faults are visible | ✅ `/metrics` + `/api/v1/events` + alert webhook; staleness → `error`, reconnect/offline/disk events logged | `services/metrics.rs`, `services/health.rs`, `services/notifier.rs` |
| Recording gaps are explainable | ✅ live `recording_gap` events + `/gaps` endpoint, cross-referenced with `camera_offline`/`recorder_error` events | `services/indexer.rs`, `routes/recordings.rs`, `services/recorder.rs` |

> Still **Level 1** (operable substrate). Stage 1 hardens the kernel for unattended operation; AI begins at Stage 2.

---

## ✅ Stage 2 — AI frame sampler  — **DONE**

**Goal:** AI consumes normalized frames **without breaking recording or live view.** Built on the kernel: a budgeted sub-stream sampler (the only component that decodes in the 24/7 path), an `ai_tasks` / `detections` data model, and a pull-based **worker contract** — workers never touch RTSP. Integrator guide: [`docs/AI-WORKERS.md`](docs/AI-WORKERS.md); implementation: `ARCHITECTURE.md` §15. Reference Python worker: `apps/ai`.

**Shipped checklist:**

- [x] **Substream frame sampler** — one supervised FFmpeg per AI-enabled camera, `-vf fps=<budgeted>,scale=<width>:-2` → `frames/<cam>/latest.jpg` (`-update 1`, overwritten in place). Decode happens **only** here; the recorder's 24/7 `-c copy` path stays decode-free. Sub-stream preferred (falls back to record URL); crash → `offline` + `sampler_offline` event + exponential backoff. (`services/sampler.rs`)
- [x] **FPS budgeting + task model** — global `HELDAR_AI_MAX_TOTAL_FPS` (default 40) split across active cameras: per-camera `effective = min(MAX(task.fps), budget/active)`, floored at `MIN_FPS=0.5`. `ai_tasks` carries `task_type / enabled / stream_profile / fps / width / config`; any create/update/delete triggers `reconcile()` → rebalance. (`services/sampler.rs`, `routes/ai.rs`, `migrations/0003_ai.sql`, `models.rs`)
- [x] **Frame delivery to workers (not raw RTSP)** — `GET /api/v1/cameras/{id}/frame` serves the latest sampled JPEG with `x-frame-age-ms` + `x-frame-captured-at` freshness headers; `GET /api/v1/ai/tasks` is worker discovery (each task + its `frame_url`); `GET /api/v1/ai/samplers` reports per-camera state + effective fps. (`routes/ai.rs`)
- [x] **Detections / events ingestion** — `POST /api/v1/ai/events` writes detections (`bbox` normalized `[x,y,w,h]` 0…1, `track_id`, `attributes`) and an optional event through the **same** `events`/notifier path as the kernel, so `warning`/`critical` AI events reuse the Stage 1 alert webhook. `GET /api/v1/cameras/{id}/detections` queries them. (`routes/ai.rs`, `repo.rs`, `migrations/0003_ai.sql`)
- [x] **Backpressure** — implemented as a **static** proportional fps split (adding AI cameras degrades per-camera fps, not the host). (`services/sampler.rs`)
- [x] **Reference worker + `Analyzer` seam** — `apps/ai/worker.py`: supervisor + per-task threads, discover → pull → analyze → post, retry/backoff, graceful shutdown. Ships a model-free `MotionAnalyzer` (frame-differencing) so the full path validates with no GPU/model; Stage 3 registers a real model behind the same `Analyzer` interface. (`apps/ai/`)

**Deferred (rolls into Stage 3 / later):**

- [ ] **High-res snapshot on trigger** (main-stream crop for plate/face) — not in the sampler; a worker can use the Stage 0 `/snapshot` endpoint today. Per-task `stream_profile=main` is stored/validated but the sampler currently always samples the sub-stream.
- [ ] **Dynamic backpressure ladder** (720p·5fps → 480p·1fps critical-only → recovery) — current split is static proportional fps; load-driven resolution downgrade + auto-recovery is future work.
- [ ] **Frame queue / `frame_id` stream** — realized as a single last-value `latest.jpg` per camera (staleness via `x-frame-age-ms`), not a multi-frame queue.

**Stage 2 success criterion:**

| Success criterion | Status | Backed by |
|---|---|---|
| AI consumes frames **without breaking recording/live view** | ✅ sampler is a separate supervised ffmpeg set decoding only the sub-stream at a bounded total fps; recorder `-c copy` + MediaMTX live view share no process/file/channel with it; a crashed/absent worker only stops frame *reads* | `services/sampler.rs`, `routes/ai.rs`, `ARCHITECTURE.md` §15.8 |

> AI begins here. Detection/tracking **models** (YOLO/RT-DETR, ByteTrack/BoT-SORT) and the canonical event model are **Stage 3**, slotting into the worker's `Analyzer` interface with no change to the kernel or the HTTP contract. Still **Level 1** substrate until Stage 3 turns frames into events.

---

## ✅ Stage 3 — Detection / tracking / zone kernel  — **DONE**

**Goal:** *turn frames into **events** — the shared base
for Security **and** BakerySense.* The build list: *person/vehicle
detector · tracker · zone annotation · zone entry/exit events · dwell-time events ·
evidence snapshot/clip.* Built across both halves of the Stage 2 contract: a
worker-side **YOLO + ByteTrack** analyzer behind the `Analyzer` seam, and a
kernel-side **zone engine** that turns tracked detections into events — **with no
change to the `POST /api/v1/ai/events` contract.** Integrator guide:
[`docs/AI-WORKERS.md`](docs/AI-WORKERS.md) §11; implementation: `ARCHITECTURE.md` §16.
Reference worker: `apps/ai`.

**Shipped checklist:**

- [x] **Person / vehicle detector (YOLO / RT-DETR baseline)** — runs in the worker behind the `Analyzer` seam, emitting class-labelled boxes (`bbox` normalized `[x,y,w,h]` 0…1). No kernel/contract change. (`apps/ai/worker.py` `Analyzer`, `docs/AI-WORKERS.md` §11.1)
- [x] **Multi-object tracker (ByteTrack)** — associates boxes across frames into stable `track_id`s, one tracker instance per task thread (per-camera state on `self`); **anonymous session tracking by default** (`track_id` ≠ identity; ReID is Stage 6). (`apps/ai/worker.py`)
- [x] **Zone annotation** — per-camera **polygon** zones (normalized 0…1 vertices), with `kind`, per-zone `labels` filter, `dwell_seconds`, `severity`, `enabled`; full CRUD API. (`routes/zones.rs`, `migrations/0004_zones.sql`, `models.rs::Zone`)
- [x] **Zone entry/exit + dwell-time events** — `ZoneEngine` evaluates each tracked detection's **bbox ground point** (bottom-center) with point-in-polygon + a per-`(camera,zone,track)` state machine → `enter` / `exit` / `dwell` events (dwell fires once per visit; state TTL-pruned at 120 s). Fed synchronously from detection ingest. (`services/zones.rs`)
- [x] **Evidence builder (snapshot)** — on `enter`, the engine copies the camera's latest sampled sub-stream frame to `/media/snapshots/zoneevt_<id>.jpg` (cheap copy, no decode) and stores it as the event's `evidence_path`. (`services/zones.rs::copy_evidence`)
- [x] **Canonical event (first concrete instance) + alert reuse** — each zone event is written to both `zone_events` **and** the kernel `events` log as `zone_{enter,exit,dwell}` at the zone's severity, so `warning`/`critical` zone events flow through the **Stage 1 alert webhook** unchanged. The event carries subject (`track_id`+`label`), location (`zone_id`/`zone_name`), timestamp, and an evidence pointer. (`services/zones.rs`, `repo::log_event`, `migrations/0004_zones.sql`)
- [x] **Event/search API** — `GET /api/v1/cameras/{id}/zone-events` (filter by `from`/`to`/`zone_id`/`event_type`, newest-first), alongside Stage 2's `/detections` (by time/label) and the kernel `/events` log. (`routes/zones.rs`, `routes/ai.rs`)

**Deferred (rolls into Stage 4+ / the fuller event model):**

- [ ] **Full canonical event model fields** — `subject` enrichment (plate/color/make), `authorization`, `workflow`, `audit.model_versions`, and **clip + recording-segment refs** on the event are not yet attached (today's evidence is a snapshot frame; segment-linked clip evidence + model-version stamping arrive with Stages 4/6 and the evidence-lock API).
- [ ] **Directional entry/exit *lines* + spatial calibration** — realized today as region enter/exit (in/out of a polygon); a dedicated directional line-crossing primitive and homography/ground-plane calibration are future work.
- [ ] **Search by object/track + zone counts** — `zone-events` filters by zone/type/time but not yet by `track_id`; count/occupancy aggregates (`kind:"count"`) are stored as a zone kind but not yet aggregated server-side.
- [ ] **BoT-SORT option** — ByteTrack is the shipped baseline; BoT-SORT (appearance + camera-motion comp) is a drop-in alternative behind the same seam when ReID-grade association is needed.

**Stage 3 goal:**

| Stage 3 build item | Status | Backed by |
|---|---|---|
| person/vehicle detector | ✅ engineering | worker `Analyzer` (YOLO/RT-DETR), `docs/AI-WORKERS.md` §11.1 |
| tracker | ✅ engineering | ByteTrack in worker, anonymous `track_id` |
| zone annotation | ✅ | `routes/zones.rs`, `zones` table |
| zone entry/exit events | ✅ | `services/zones.rs` state machine |
| dwell-time events | ✅ | `services/zones.rs` (`dwell_seconds` threshold) |
| evidence snapshot/clip | ◑ | snapshot frame on entry shipped; clip/segment refs deferred |

> **Engineering is production-grade; model accuracy is not yet benchmarked.**
> The Stage 3 *systems engineering* — the tracked-detection contract, polygon/point-in-polygon
> zone evaluation, the enter/exit/dwell state machine with TTL pruning, evidence capture, the
> schema, and the CRUD/query API — is complete and unit-tested. What is **not** yet validated is
> the detector/tracker **accuracy on local footage**: public/pretrained models may
> not reflect Malaysian vehicle distribution, plate/camera angles, motorcycles, night-IR, or rain;
> ReID/association degrades on new sites and in crowds. The required path is explicit:
> start with type + color, treat make/model and any identity-like match as **top-5 assistive
> candidates with human review**, **benchmark on local gate/shop footage**, fine-tune only after
> local data collection, and **never** use model recognition as a hard access decision. Accuracy
> benchmarking is gated on collecting that local footage set — an evaluation, not an engineering, task.

> This is the inflection to **Level 2 → 3** (event memory → scene/event graph). The zone event is a "claim level 2" with an evidence pointer; the graph-relational event schema is seeded here (`zone_events` denormalizes `zone_name` and outlives its zone for auditability) and deepens in Stages 6–7.

---

## ✅ Stage 4 — Access Control app (client Phase 1)  — **DONE**

**Goal:** the client's "Premise Security / Entry intelligence" deliverable. Built as the first **vertical app** on the kernel: an
RBAC layer, an entry registry (vehicles / passes / watchlist), an **ANPR
temporal-voting engine** producing canonical entry/exit events, a guard
confirm/reject workflow, and reports — all on the **unchanged** Stage 2 ingest
contract (`anpr` tasks feed the engine via `POST /api/v1/ai/events`). Operator/
integrator guide: [`docs/ACCESS-CONTROL.md`](docs/ACCESS-CONTROL.md); implementation:
`ARCHITECTURE.md` §17. Worker side: `apps/ai` `AnprAnalyzer`.

**Shipped checklist:**

- [x] **Visitor pre-registration + guard-booth check-in (operator dashboard surface)** — `visitor_passes` (auto `V-XXXXXX` code, validity window, `active→checked_in→checked_out`/`revoked` lifecycle) + check-in/out endpoints that also write a manual `visitor_checkin`/`visitor_checkout` entry event. Full CRUD API for the booth UI. (`routes/entry.rs`, `migrations/0005_entry.sql`)
- [x] **ANPR / ALPR** — vehicle→plate→OCR (worker `AnprAnalyzer`) → **server-time temporal voting** per `(camera,track)` → format/plausibility validate → registry lookup, committing **one** canonical event per vehicle. Plate/pass = **primary** identity anchor; voting is on the plate (min `HELDAR_ANPR_MIN_VOTES`, default 3) with commit-on-prune for fast passers. (`services/anpr.rs`, `apps/ai/worker.py`)
- [x] **Vehicle attributes (type → color → make → model)** — **secondary** verification + search metadata only: the engine compares **color + vehicle_type** for mismatch (→ *exception for guard review*, never auto-reject); make/model is assistive and never a hard access decision. The reference worker emits type + color (no make/model classifier yet). (`services/anpr.rs::check_mismatch`, `apps/ai/worker.py`)
- [x] **Daily entry logs · exception reports · audit reports** — `GET /reports/entry-log` (window + `by_auth_status` counts), `GET /reports/exceptions` (blocked/exception/unmatched/rejected), `GET /audit` (immutable action log, manager+). (`routes/entry.rs`)
- [x] **Role matrix (RBAC) + API integration layer** — five roles (`admin`/`manager`/`guard`/`viewer`/`integration`) × five capabilities; opaque `vos_` sessions + `vok_` API keys (SHA-256 at rest, argon2id passwords); `auth_enabled` gating with a synthetic system admin when off; env bootstrap admin. API keys (`X-API-Key` / `Bearer`) are the integration seam for the worker + external callers. (`auth.rs`, `routes/auth.rs`)

**Done when (status):** ✅ **Met.** A guard runs entry end-to-end — ANPR auto-resolves
registered/pass/VIP plates, raises `pending` exceptions/blocks for review, and the
guard confirms/rejects from the entry-event queue; manual booth check-in/out lands in
the same feed. Daily-log / exception / audit reports generate over any window. The
design (in-memory voting keyed per track, SQLite registry, one synchronous engine call
per ingest batch, 365-day entry retention) is sized for the ~2–3k students × 2 entries
target with no extra moving parts. **Open:** OCR/make-model *accuracy* is an evaluation
task pending local footage (see deferrals).

**Deferred (honest scope):**

- [ ] **Directional entry/exit *lines* + spatial calibration** — the engine accepts a
  per-camera `direction` config **hint** (`inbound`/`outbound`) only; a calibrated
  line-crossing / homography primitive (true in/out from geometry) is future work.
  Gate cameras are usually single-direction, so the hint covers the Phase 1 need.
- [ ] **OCR + make/model *accuracy* benchmarking on local Malaysian gate footage** —
  the *engineering* (voting, resolution, workflow, schema, API) is production-grade and
  unit-tested; *accuracy* is an evaluation task (Malaysian plate
  shapes/angles, motorcycles, night-IR, rain; fine-grained make/model). Never a hard
  access decision until locally benchmarked.
- [x] **Auth on the legacy Stage 0–3 routes** — the `Principal` guard now spans the
  kernel: camera list/read, live view, recordings (segments/timeline/gaps), health,
  events, and the recording + snapshot schedule lists all assert at least `can_view`, so
  the whole API requires a session when `HELDAR_AUTH_ENABLED=true` (default off keeps the
  open LAN appliance).

**Phase 1 items:**

| Phase 1 (Access Control) item | Status | Backed by |
|---|---|---|
| Visitor registration + guard-booth check-in | ✅ | `visitor_passes` + checkin/checkout (`routes/entry.rs`), manual entry events |
| ANPR / ALPR (primary identity anchor) | ✅ engineering; ⚠️ accuracy unbenchmarked | `services/anpr.rs` temporal voting + resolution, worker `AnprAnalyzer` |
| Vehicle attributes (type/color/make/model, secondary) | ◑ type + color shipped; make/model classifier deferred | `services/anpr.rs::check_mismatch` (color+type → exception), worker color heuristic |
| Daily entry logs | ✅ | `GET /reports/entry-log` (+ `by_auth_status`) |
| Exception reports (plate/vehicle mismatch) | ✅ | `GET /reports/exceptions`; mismatches surface as `exception` events |
| Audit reports | ✅ | `audit_log` + `GET /audit` (manager+), written on every mutation |
| Role matrix (RBAC) | ✅ | `auth.rs` 5 roles × 5 capabilities; sessions + API keys; `auth_enabled` gating |
| API integration layer | ✅ | `vok_` API keys (`X-API-Key`/`Bearer`), `integration` role = least-privilege ingest |

> **Engineering is production-grade; OCR/make-model accuracy is not yet benchmarked** — same posture as Stage 3: the systems work (temporal voting,
> fail-closed block lookup, attribute-mismatch-as-exception, canonical event +
> evidence, guard workflow, RBAC, reports) is complete and tested; recognition
> *accuracy* on local Malaysian gate footage is an evaluation task gated on collecting
> that footage set. This is **Level 2 → 3** applied to premise security:
> the canonical entry event is a typed claim with subject + authorization +
> evidence + workflow + audit, and the registry resolution is the first identity-aware
> event (anonymous tracking still the default elsewhere; cross-camera ReID is Stage 6).

---

## ✅ Stage 5 — BakerySense Vision  — **DONE**

**Goal:** retail behaviour analytics on the **same kernel**, different ontology.
Diagnosis-oriented, **anonymous by construction (no identity, no faces, no plates).**
BakerySense (`heldar-bakery`) is a **proprietary retail-analytics vertical that lives in a
separate private repo**; it is not part of the open Apache-2.0 distribution. It is built
on the open kernel as the second **vertical app**: not a detection consumer on the hot
path, but a **rollup and report layer** that reads the kernel's stored `zone_events` and
`detections` off the ingest path. It composes onto the open server through the kernel's
vertical seam, with its own storage and routes, so a slow or crashed rollup cannot affect
recording, ingest, or live view, and the kernel does not depend on it. Its internal
schema, metrics, thresholds, and algorithms live in the private repo.

**Capabilities (boundary level):**

- [x] **Shop camera analysis on ordinary kernel zones.** Retail zones are ordinary kernel zones tagged via `kind` (entrance / exit / shelf / cashier / queue / display); BakerySense interprets the kinds and requires a `detection` AI task running to produce anonymous events.
- [x] **Footfall, queue and browse dwell, occupancy, display engagement.** Periodic anonymous rollups over the stored kernel facts, per camera.
- [x] **Abandonment proxy (browse without a checkout transition).** Derived per camera over anonymous, ephemeral `track_id`s, with explicit caveats (it cannot see external purchases, staff, or pass-through).
- [x] **Daily diagnosis report: observation, evidence, interpretation, suggested experiment (correlation, not causation).** Every insight carries an explicit **confidence** (sample-size tiered) and **uncertainty** (the anonymity caveat); the diagnosis is deterministic and heuristic.
- [x] **Evidence-clip retrieval per insight.** Insights point at a `camera_id` and a day window; the operator requests footage from the **kernel** clip API (`POST /api/v1/cameras/{id}/clip`). BakerySense stores no video.

**Done when (status):** ✅ **Met.** With a detection task and retail-tagged zones on a shop
camera, the rollup loop produces footfall / dwell / occupancy / display / abandonment
observations, and generates a daily diagnosis over any day or scope, each insight running
observation to evidence to interpretation to experiment with confidence, uncertainty, and
a clip pointer. Because it runs periodically over stored kernel tables off the ingest hot
path, a slow or crashed rollup cannot affect recording, ingest, or live view. **Open:**
detector/tracker *accuracy* on local shop footage is an evaluation task (see deferrals).

**Deferred (honest scope):**

- [ ] **Staff coverage and shelf/counter-empty state.** Need dedicated detectors
  (staff-vs-customer classification, product-zone empty-state detection) that the current
  person-detection worker does not provide.
- [ ] **LLM/VLM report interpretation.** This stage's diagnosis is deterministic and
  heuristic by design; natural-language synthesis and VLM interpretation are **Stage 7**.
- [ ] **SKU-level analysis.** Out of scope: BakerySense works at shelf / product-group
  level, not per-product.
- [ ] **Cross-camera linking.** Occupancy and abandonment are per-camera over ephemeral
  `track_id`s; cross-camera journeys are **Stage 6** (ReID), under the same privacy gates.
- [ ] **Detector/tracker accuracy on local footage.** The *engineering* is complete;
  *accuracy* is an evaluation task, surfaced verbatim in every insight's uncertainty note.

> This is the concrete **Level 2 MVP** ("Queryable Retail CCTV Memory v0"):
> anonymous by construction, shelf/product-group level (not SKU), every number shipping
> with its sample size and the anonymity caveat. **Engineering is production-grade; model
> accuracy is not yet benchmarked**, the same posture as Stages 3 and 4. The diagnosis
> report is a deterministic precursor to the Stage 7 LLM/VLM interpretation layer.

---

## ✅ Stage 6 — ReID & movement intelligence (client Phase 2)  — **DONE**

**Goal:** cross-camera movement = client's "Movement intelligence" / Heldar Security.
Built as a third **vertical app**
(`crates/heldar-movement`) — the **same kernel, cross-camera**. Like BakerySense it is
**not** a detection consumer on the hot path, but a **correlation layer**: two
`spawn_supervised` loops (a ReID candidate **proposer** + a red-zone breach **rule
engine**) plus an on-demand trigger/search surface, reading the kernel's + Access Control's
stored `entry_events` / `detections` / `zone_events` / `zones`, **composed (not welded)**
with its own schema/config/loops/retention/routes. Operator/integrator guide:
[`docs/MOVEMENT.md`](docs/MOVEMENT.md); implementation: `ARCHITECTURE.md` §19.

**Shipped checklist:**

- [x] **Person ReID + vehicle ReID — multi-signal, never pure visual embedding** — **no
  appearance/visual embedding anywhere.** Vehicle ReID is anchored on the **plate**
  (resolved by Access Control) and fused with transit-time plausibility + colour/type
  agreement over the topology graph: `score_pair` = `0.8` plate anchor `±` transit (`+0.10`
  in-window / `+0.05` ≤2× / else 0) `±` colour (`+0.05`/`−0.10`) `±` type (`+0.05`/`−0.10`),
  proposed at `≥ HELDAR_MOVEMENT_MIN_SCORE` (default 0.5). Person ReID has no plate/no
  embedding, so it is **never auto-proposed** — only the weak topology+time search.
  (`reid.rs::score_pair` / `propose_vehicle_candidates`, `routes.rs::search_person`)
- [x] **Multi-camera topology graph + movement trails** — `camera_links` operator-defined
  directed adjacency (`from`/`to`/`transit_seconds`/`bidirectional`) scopes all matching;
  movement trail = all `entry_events` for a normalized plate, time-ordered
  (`trail_for_plate`). Full CRUD API (manage-gated). (`schema.sql`, `routes.rs`,
  `reid.rs::trail_for_plate`)
- [x] **Red/green zone breach alerts (rule engine)** — `breach::sweep()` resolves red zones
  by `kind` (`HELDAR_MOVEMENT_RED_ZONE_KINDS`, default `restricted,red`), records one
  `breach_alerts` incident per zone `enter` event (`ON CONFLICT(zone_event_id) DO NOTHING`
  dedup), correlates `track_id → plate` within ±5 min, and is worked open → acknowledged →
  resolved. **Complements** the kernel's existing restricted-zone webhook (it adds the
  tracked correlated incident, does **not** re-notify). (`breach.rs`)
- [x] **Candidate search + human-review workflow (probabilistic, not legal identity)** —
  every cross-camera link is a `pending` candidate a human (`operate_gate`) confirms or
  rejects (`reviewed_by`/`reviewed_at`); nothing is auto-confirmed. Audited plate trail +
  candidate search and weak person search; every identity-like query writes an `audit_log`
  row **before** querying, and responses carry a "probabilistic, not identity" note.
  (`routes.rs` confirm/reject + `search/plate` + `search/person`, `auth::audit`)

**Done when (status):** ✅ **Met.** With a camera-topology graph configured, the ReID loop
proposes vehicle candidates (same plate on two linked cameras within a plausible transit
window, fused-scored) into a `pending` review queue, and the breach loop turns
restricted-zone entries into deduped, subject-correlated incidents — both on
`HELDAR_MOVEMENT_INTERVAL_S`, or via `POST /api/v1/movement/run`. An operator reviews
candidates (`confirm`/`reject`), works breaches (`ack`/`resolve`), and runs audited plate /
person searches; the design (two supervised loops over stored kernel/Entry tables, own
SQLite schema, own retention off the ingest hot path) means a slow or crashed loop cannot
affect recording/ingest/live view. **Open:** ReID *accuracy* (false-link/missed-link/path)
is unbenchmarked on local footage — the human review gate is the safeguard (see deferrals).

**Deferred (honest scope):**

- [ ] **No visual / appearance ReID embedding** — by design:
  vehicle ReID is **anchored on the plate** (+ transit/colour/type fusion); person ReID is
  **weak, topology + time only**. No visual-embedding vector search / FastReID-style model
  is wired in — and adding one for *person* identity needs explicit legal/consent/governance
  basis, not just engineering.
- [ ] **No homography / ground-plane calibration** — transit windows are operator-declared
  `transit_seconds` per `camera_links` edge, not geometry-derived; no metric speed/distance
  model; the proposer correlates only cameras joined by an explicit `camera_links` edge.
- [ ] **ReID accuracy unbenchmarked on local footage** — the *engineering* (scoring, topology
  scoping, candidate workflow, breach engine, schema, API) is complete; *accuracy*
  (Rank-1/mAP/false-link/missed-link/site-path) is an evaluation task gated
  on local data. Never an auto-decision — confirm/reject is always human.
- [ ] **Cross-camera person journeys are low-confidence, human-triage only** — never
  auto-proposed, capped at `0.4` (topology+time), and always audited; not a continuous
  person-tracklet graph.

**ReID and privacy rules:**

| ReID / privacy rule | Status | Backed by |
|---|---|---|
| Vehicle ReID multi-signal, **not** pure visual embedding | ✅ plate-anchored + transit + colour/type fusion; no embedding | `reid.rs::score_pair` |
| Camera topology + time-window filter | ✅ `camera_links` join scopes all matching | `reid.rs`, `routes.rs::search_person`, `camera_links` |
| Movement trail / path | ✅ plate appearances time-ordered | `reid.rs::trail_for_plate` |
| Person ReID = probabilistic correlation, never legal identity | ✅ weak topology+time only, on-demand, human-triage | `routes.rs::search_person` (cap 0.4 + note) |
| Candidate matching + human-review workflow | ✅ `pending` → human confirm/reject (`operate_gate`), nothing auto-confirmed | `routes.rs::resolve_candidate` |
| Audit trail on every identity-like query | ✅ `auth::audit` before each search returns | `routes.rs` `search_plate`/`search_person` |
| Red/green zone breach alerts (rule engine) | ✅ red-zone-entry incidents, dedup, subject correlation, worked lifecycle | `breach.rs` |
| Role-based access | ✅ view / operate_gate / manage gating (Stage 4 RBAC) | `routes.rs` `principal.require(...)` |

> **Engineering is production-grade; ReID accuracy is not yet benchmarked,
> and visual-embedding ReID is deliberately absent.** Same posture
> as Stages 3–5: the systems work (multi-signal plate-anchored scoring, topology graph,
> candidate proposer, human-review workflow, breach rule engine with subject correlation,
> audited searches, retention, RBAC) is complete; *accuracy* on local footage is an
> evaluation task, and identity is always a **human** call. Privacy gates are wired in: candidate-not-identity, human enforcement, confidence
> thresholds, and an audit trail on every identity-like query. This is **Level
> 3** scene/event graph applied to security — typed, evidence-backed, audited cross-camera
> correlation that stays explicitly probabilistic.

---

## ✅ Stage 7 — Semantic video search  — **DONE**

**Goal:** searchable visual event memory — *who/what/where/when/confidence/evidence/workflow.*
Built as a fourth **vertical app**
(`crates/heldar-search`) — and the most "composed, not welded" of all: **not** a
`DetectionConsumer` and **not** even a background loop, but a **read-only query layer over
kernel facts** (three HTTP routes + one query log) reading the tables Stages 3/4/6 already
wrote (`entry_events`, `zone_events`, `breach_alerts`). One governing principle: **the LLM
is a query PLANNER, never the source of truth** — answers are the executed query's rows.
Operator/integrator guide: [`docs/SEARCH.md`](docs/SEARCH.md); implementation:
`ARCHITECTURE.md` §20.

**Shipped checklist:**

- [x] **Search by object attributes** — a structured `QueryPlan` (plate · colour ·
  vehicle_type · subject_type · auth_status · source · event_type · zone_kind · free-text ·
  camera · time window · time-of-day) executed deterministically over the kernel fact
  tables; `POST /api/v1/search/events`. (`query.rs::execute`)
- [x] **Search by plate** — exact normalized-plate filter across `entry_events` (and
  `breach_alerts` where the plate was correlated by Movement); plate-targeted queries are
  **identity-bearing** and audited. (`query.rs`, `routes.rs::is_identity_query`)
- [x] **Natural-language event search (LLM as query planner, never source of truth)** — a
  question is *planned* into the same `QueryPlan` (offline rule parser by default, optional
  LLM seam when configured), executed deterministically, and the **rows are the answer**;
  `POST /api/v1/search/nl` + a `POST /api/v1/search/plan` dry-run. (`planner.rs`,
  `routes.rs`)
- [x] **Proof layer** — every answer decomposed into claim levels
  (observation → track → event → aggregate → inference) with evidence + confidence; the
  NL→plan reading is the **single** step marked `fallible: true`; no layer asserts identity
  or causation. (`proof.rs`)
- [ ] **Search by vehicle image · by person crop** — **deferred** (needs event/clip
  embeddings; see below).
- [ ] **VLM-based report interpretation** — **deferred** (by design; see below).
- [ ] **Open-vocabulary enrichment + event/clip embeddings (vector retrieval)** —
  **deferred** (a seam, not built; see below).

**Done when (status):** ✅ **Met.** An operator (or integration key) with `view` can ask a
question in natural language — *"unknown white cars entering Gate B after 6pm last week"*,
*"people who entered red zones yesterday without authorization"* — and get back the matching
**stored events**, each with its evidence frame + a clip pointer, wrapped in a proof ladder
that shows the executed plan and flags the question-reading as the only inference. The same
filters are available as a structured `QueryPlan`, and `/search/plan` dry-runs the
interpretation with no execution. The design (read-only query over already-stored kernel
tables, a default 7-day window + a fetch cap, one small query log, no ingest path / no loop /
no decode) means a slow or failing search can affect only that request. The rule parser runs
**fully offline**; the LLM is optional and only ever plans. **Open:** the embedding/VLM
retrieval seam (search-by-image) is documented, not built (see deferrals).

**Deferred (honest scope):**

- [ ] **Open-vocabulary VLM enrichment + event/clip EMBEDDINGS + vector retrieval** — a
  documented **seam, not built.** They need an **embedding/VLM worker** (the same
  `Analyzer`-style contract as the detection worker) to write embeddings the query layer
  could rank against. This stage ships the deterministic structured + NL-plan + proof core
  only.
- [ ] **Search by image / vehicle crop / person crop** — depends on those embeddings, so
  **not available**; today's search is by structured *attributes*, not visual similarity.
- [ ] **VLM-based report interpretation** — intentionally absent: the proof layer reports
  deterministic aggregates, **not** generated prose (the LLM plans, it never narrates the
  answer).
- [ ] **LLM planner is optional and untested without a live endpoint** — exercised only when
  `HELDAR_SEARCH_LLM_URL` is configured; the default and fallback path is the rule parser.
- [ ] **Rule parser is best-effort** — it recognizes its keyword patterns (colour/type/
  subject/auth/source/event, relative dates, time-of-day, camera names, plate token) and
  leaves the rest to the default window; it cannot express dwell thresholds or
  multi-condition joins (use `/search/plan` to confirm a parse, or send a structured
  `QueryPlan` for full control).

**Semantic search targets:**

| Target | Status | Backed by |
|---|---|---|
| Searchable visual event memory (who/what/where/when) | ✅ | `query.rs` structured + NL search over `entry_events`/`zone_events`/`breach_alerts` |
| Natural-language search, **LLM as query planner** | ✅ | `planner.rs` (offline rules default + optional LLM seam, falls back) |
| Search by plate / object attributes | ✅ | `QueryPlan` filters + deterministic executor |
| Search by vehicle/person image | ◻ deferred | needs event/clip embeddings (embedding/VLM worker) |
| VLM-based report interpretation | ◻ deferred | by design — proof reports deterministic aggregates, not prose |
| Open-vocab enrichment + embeddings / vector retrieval | ◻ deferred | a seam; needs an embedding/VLM worker |
| Proof layer (claim levels + evidence + confidence) | ✅ | `proof.rs` (obs→track→event→aggregate→inference; NL→plan = the only fallible step) |

> **Engineering is production-grade; the embedding/VLM retrieval layer is a deliberate
> seam.** Same posture as Stages 3–6: the systems work (the `QueryPlan`, the deterministic
> time-bounded executor, the offline rule parser + optional LLM planner-with-fallback, the
> proof/claim ladder, the search log + identity-query audit, the RBAC-gated routes) is
> complete; visual/embedding retrieval is documented future work needing an embedding/VLM
> worker. This is **Level 3 → 4** (event memory → latent world memory): a typed,
> evidence-backed, deterministic query layer whose **only** inference — reading the question
> — is surfaced, fallible, and decoupled from the answer.

---

## 🔭 Beyond the staged plan — research frontier (Level 4–5)

Not committed deliverables; the long-term moat that Stages 3–7 are deliberately architected toward:

- [ ] Event-causal memory (State-Event-State graph, baseline/before-after comparison, hypothesis generation with caveats)
- [ ] Salience-aware compression & memory policy (JEPA-style: store what is surprising/agentic/risky/business-relevant; summarize the predictable)
- [ ] Predictive **bounded world model**: queue-buildup / abandonment / incident-risk forecasting, layout & staffing simulation (**Level 5 — not solved**)
- [ ] Internal **CCTV World Memory Bench** to drive R&D before claiming intelligence

---

## Principles carried across every stage

1. **Kernel first, AI as plugins, apps last** — never build AI before the substrate.
2. **Record compressed, decode only when sampling** — recording avoids decode; AI consumes substream frames.
3. **Privacy by architecture** — anonymous by default, no face recognition by default, RBAC, audit logs, evidence-lock, short raw retention.
4. **LLM is the planner, not the source of truth** — every answer carries evidence, confidence, and uncertainty.
5. **Separate observation / correlation / hypothesis / causation** — CCTV proves sequences, not causes.
6. **Product benchmarks > leaderboard metrics** — reconnect time, recording-gap rate, guard correction rate, cost/camera/month.
