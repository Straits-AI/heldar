# VisionOps Core — Roadmap

> **Thesis:** Camera streams become structured events → events become workflows → workflows become operational intelligence.
> We build the **media kernel first**, then AI as plugins on top, then vertical apps. The long arc (see `research.md`) is to turn continuous video into a **compressed, queryable, verifiable world memory** of a physical space — so analytical intent can be defined *after* collection, not before.

Source of truth: `memo.md` §14 (Build roadmap) and `research.md` §21 (Product Roadmap) + §5 (Level 1–5 maturity ladder). This file reconciles the two.

---

## Two roadmaps, one product

`memo.md` is the **systems/vertical** roadmap (own the VMS, then ship Entry / Retail / Security apps). `research.md` is the **representation/intelligence** roadmap (event memory → scene graph → semantic search → world model). They are the same product viewed from two ends:

```
memo.md           Stage 0 ── 1 ── 2 ── 3 ──── 4/5 ──── 6 ──────── 7
                  kernel  obs  sampler det/track  apps    ReID    semantic search
                    │      │     │      │          │       │           │
research ladder    L1 ───────────────── L2 ────── L2/L3 ── L3 ──── L3→L4→L5
                  task     event memory   scene/event graph    world memory
```

- **memo Stage 0–2** = build the substrate (Level 1 plumbing).
- **memo Stage 3 + research Stage 1–2** = events + scene/event graph (**Level 2 → 3**).
- **memo Stage 7 + research Stage 3–4** = semantic/causal query (**Level 3 → 4**).
- **research Stage 5** = predictive bounded **world model** (**Level 5**, research frontier — not solved).

Maturity ladder (research.md §5): **L1** task-specific analytics (industry baseline) · **L2** event memory (buildable now, MVP target) · **L3** scene/event graph (buildable with engineering, the differentiator) · **L4** AI-native latent world memory (research frontier, the moat) · **L5** general physical world model (not solved).

---

## ✅ Stage 0 — Media kernel MVP  — **DONE**

Goal (memo §14): *own the base VMS.* Record compressed packets without decode; index, play back, export, and keep cameras healthy. Built in `apps/core` (Rust / Axum / Tokio / SQLx-SQLite) with MediaMTX + FFmpeg as the media engine.

**Shipped checklist** (memo §14 build list + §16 immediate technical actions):

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

**Cross-ref to memo Stage 0 success criteria:**

| Memo §14 success criterion | Status | Backed by |
|---|---|---|
| 8–16 cameras | ✅ multi-camera registry + per-camera recorder supervision | `recorder.rs`, validation run |
| 7 days continuous operation | ✅ reconnect/watchdog + retention keep it running unattended | `recorder.rs`, `retention.rs` |
| Recording playable | ✅ timeline index + segment/playback API + MediaMTX | `indexer.rs`, `routes/recordings.rs` |
| Clip export works | ✅ MP4 export endpoint | `routes/playback.rs`, `clip.rs` |
| Camera reconnect works | ✅ reconnect tracked in `camera_status`, surfaced via health/events | `recorder.rs`, `health.rs` |

> Maps to research.md **Level 1** (the raw substrate) — the prerequisite for everything above it. No AI yet, by design.

---

## ✅ Stage 1 — Observability & reliability  — **DONE**

**Goal (memo §14):** the system is operable by a non-developer; faults are visible; recording gaps are explainable. Built on the Stage 0 kernel with no new tables — everything is computed over `segments` / `camera_status` / `events` or read live from the OS. Operator/SRE guide: [`docs/OBSERVABILITY.md`](docs/OBSERVABILITY.md); implementation: `ARCHITECTURE.md` §14.

**Shipped checklist:**

- [x] **Recording gap detector** — `recording_gap` (warning) events emitted by the indexer when consecutive segments are >3 s apart, **plus** an on-demand `GET /api/v1/cameras/{id}/gaps?from&to` that reports the holes between coalesced availability ranges. (`services/indexer.rs`, `routes/recordings.rs`)
- [x] **Stream metrics** — observed `fps_observed` + `bitrate_kbps` computed per indexed segment and stored on `camera_status`, surfaced via the health API and (bitrate) Prometheus. (`services/indexer.rs`, `repo.rs`, `routes/health.rs`)
- [x] **Disk / storage health monitor** — `statvfs` free-space, recordings footprint, recent write rate, and free-disk-fill projection in the `/api/v1/system` `storage` block; `disk_pressure` events on pressure. (`services/storage.rs`, `routes/system.rs`)
- [x] **Prometheus metrics + liveness/readiness** — `GET /metrics` (system + per-camera gauges/counters), `GET /healthz` (liveness), `GET /readyz` (readiness, 200/503 on DB reachability). (`services/metrics.rs`, `routes/metrics.rs`, `routes/health.rs`)
- [x] **Alerting** — `VISIONOPS_ALERT_WEBHOOK_URL` notifier POSTs warning/critical events as JSON; starts-from-now (no replay on boot), retries on transport failure. (`services/notifier.rs`)
- [x] **Disk-free retention floor** — `VISIONOPS_MIN_FREE_DISK_GB` hard floor prunes oldest *unlocked* segments when the filesystem gets tight, on top of the age policy + `VISIONOPS_MAX_RECORDINGS_GB` size cap; evidence-lock honored throughout. (`services/retention.rs`)
- [x] **Service watchdog / auto-restart** — `spawn_supervised` respawns the indexer / health / retention / notifier loops 5 s after any return or panic. (`main.rs`)

**Deferred (rolls into later edge/cloud work):**

- [ ] Per-camera health **dashboard** UI (the health/events/metrics APIs exist; the web frontend view is still pending — `apps/web`)
- [ ] Edge offline buffer + cloud sync retry (the webhook notifier is the first upstream alert path; full store-and-forward sync remains planned)
- [ ] Packet-loss / throughput **trends** (current fps/bitrate are last-value, not time-series; trend storage is future work)

**Cross-ref to memo §14 Stage 1 success criteria:**

| Memo §14 success criterion | Status | Backed by |
|---|---|---|
| System operable by a non-developer | ✅ health/system/events/metrics APIs + webhook alerts surface state without log-diving | `routes/health.rs`, `routes/system.rs`, `routes/metrics.rs`, `services/notifier.rs` |
| Faults are visible | ✅ `/metrics` + `/api/v1/events` + alert webhook; staleness → `error`, reconnect/offline/disk events logged | `services/metrics.rs`, `services/health.rs`, `services/notifier.rs` |
| Recording gaps are explainable | ✅ live `recording_gap` events + `/gaps` endpoint, cross-referenced with `camera_offline`/`recorder_error` events | `services/indexer.rs`, `routes/recordings.rs`, `services/recorder.rs` |

> Still research.md **Level 1** (operable substrate). Stage 1 hardens the kernel for unattended operation; AI begins at Stage 2.

---

## ⬜ Stage 2 — AI frame sampler

**Goal:** AI consumes normalized frames **without breaking recording or live view.** (memo §4 Layer 4, §14)

- [ ] Substream frame sampler (decode only sampled frames; record stays decode-free)
- [ ] FPS budgeting + per-task scheduler (`AITask`: camera, type, fps, resolution, zone, priority)
- [ ] Frame queue / frame-sample object passed to workers (not raw RTSP)
- [ ] High-res snapshot on trigger (main-stream crop for plate/face)
- [ ] Backpressure policy: normal → high-load → severe → recovery (720p·5fps → 480p·1fps critical-only → restore)

**Done when:** AI workers run at sustained load with recording/live view unaffected.

---

## ⬜ Stage 3 — Detection / tracking / zone kernel

**Goal:** turn frames into **events** — the shared base for Security *and* BakerySense. (memo §7.1–7.2, §8)

- [ ] Person / vehicle detector (YOLO / RT-DETR baseline)
- [ ] Multi-object tracker (ByteTrack / BoT-SORT) — **anonymous session tracking by default**
- [ ] Zone annotation + spatial calibration (entry/exit lines, red/green/queue/shelf zones)
- [ ] Zone entry/exit + dwell-time events
- [ ] Canonical event model + evidence builder (snapshot + clip + recording segment refs + confidence + model versions) — extends the existing `events` table
- [ ] Event/search API: by time, camera, zone, object, event type

> This is the inflection to research.md **Level 2 → 3** (event memory → scene/event graph). The canonical event = research.md's "claim level 2" with evidence pointers. Build the graph-relational event schema here, not later.

---

## ⬜ Stage 4 — Campus Entry app (client Phase 1)

**Goal:** the client's "Premise Security / Entry intelligence" deliverable. (memo §2 Phase 1, §7.3–7.4, §14)

- [ ] Visitor pre-registration + guard-booth check-in (operator dashboard)
- [ ] **ANPR / ALPR** — vehicle→plate detect→rectify→OCR→temporal voting→format validate→lookup. Plate/pass = **primary** identity anchor.
- [ ] Vehicle attributes (type → color → make → model) — **secondary** verification + search metadata only (top-5 assistive; no hard access decision on make/model in Malaysia without local benchmarking)
- [ ] Daily entry logs · exception reports (plate/vehicle mismatch) · audit reports
- [ ] Role matrix (RBAC) + API/webhook integration layer

**Done when:** guard runs entry end-to-end; exceptions/audit reports generate; sized for ~2–3k students × 2 entries.

---

## ⬜ Stage 5 — BakerySense Vision

**Goal:** retail behaviour analytics on the **same kernel**, different ontology. Diagnosis-oriented, **anonymous (no identity, no face recognition).** (memo §7.7, §14; research.md §24 MVP, Stage 1)

- [ ] Shop camera analysis + zone annotation (entrance/exit/shelf/cashier/queue)
- [ ] Footfall (entry/exit count) · queue length & dwell · browse dwell · display engagement
- [ ] Abandonment proxy (browse without checkout transition) · staff coverage · shelf/counter-empty state
- [ ] Daily **diagnosis** report: observation → evidence → interpretation → suggested experiment (correlation, **not** causation)
- [ ] Evidence-clip retrieval per insight

> This is research.md's concrete **Level 2 MVP** ("Queryable Retail CCTV Memory v0"). Start at shelf/product-group level, not SKU. Every number ships with evidence + uncertainty.

---

## ⬜ Stage 6 — ReID & movement intelligence (client Phase 2)

**Goal:** cross-camera movement = client's "Movement intelligence" / VisionOps Security. (memo §2 Phase 2, §7.5–7.6, §14)

- [ ] Person ReID + vehicle ReID — **multi-signal**, never pure visual embedding (fuse plate/color/type/topology/time/direction)
- [ ] Multi-camera topology graph + movement trails (tracklet/event graph)
- [ ] Red/green zone breach alerts (rule engine + notification router)
- [ ] Candidate search + **human-review workflow** (ReID = probabilistic correlation, not legal identity)

> Privacy gates (memo §15.5, research.md §14): anonymous by default, ReID treated as candidate matching with human enforcement, local calibration set, confidence thresholds, audit trail on every identity-like query. Maps to research.md **Level 3** scene/event graph applied to security.

---

## ⬜ Stage 7 — Semantic video search

**Goal:** searchable visual event memory — *who/what/where/when/confidence/evidence/workflow.* (memo §9, §14; research.md Stage 3–4)

- [ ] Search by plate · by vehicle image · by person crop · by object attributes
- [ ] Natural-language event search (LLM as **query planner**, never source of truth)
- [ ] VLM-based report interpretation
- [ ] Open-vocabulary enrichment + event/clip embeddings (vector retrieval)
- [ ] **Proof layer:** every answer decomposed into claim levels (obs → track → event → aggregate → inference → hypothesis) with evidence + confidence (research.md §12–13)

> Example targets: *"unknown white cars entering Gate B after 6pm last week"; "people who entered red zones yesterday without authorization"; "customers who waited >5 min and left without checkout."* This is research.md **Level 3 → 4** (latent world memory / event-causal memory).

---

## 🔭 Beyond the staged plan — research frontier (research.md §21 Stage 5, Level 4–5)

Not committed deliverables; the long-term moat that Stages 3–7 are deliberately architected toward:

- [ ] Event-causal memory (State-Event-State graph, baseline/before-after comparison, hypothesis generation with caveats)
- [ ] Salience-aware compression & memory policy (JEPA-style: store what is surprising/agentic/risky/business-relevant; summarize the predictable) — research.md §9, §17
- [ ] Predictive **bounded world model**: queue-buildup / abandonment / incident-risk forecasting, layout & staffing simulation (**Level 5 — not solved**)
- [ ] Internal **CCTV World Memory Bench** to drive R&D before claiming intelligence (research.md §20)

---

## Principles carried across every stage

1. **Kernel first, AI as plugins, apps last** — never build AI before the substrate (memo §17).
2. **Record compressed, decode only when sampling** — recording avoids decode; AI consumes substream frames (memo §6.1, §15.1).
3. **Privacy by architecture** — anonymous by default, no face recognition by default, RBAC, audit logs, evidence-lock, short raw retention (memo §15.5, research.md §14).
4. **LLM is the planner, not the source of truth** — every answer carries evidence, confidence, and uncertainty (research.md §27).
5. **Separate observation / correlation / hypothesis / causation** — CCTV proves sequences, not causes (research.md §13).
6. **Product benchmarks > leaderboard metrics** — reconnect time, recording-gap rate, guard correction rate, cost/camera/month (memo §10.2).
