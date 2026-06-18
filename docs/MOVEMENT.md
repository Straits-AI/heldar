# Heldar Core — Movement Intelligence (Stage 6) Operator & Integrator Guide

This is the definitive guide to **Movement Intelligence** **as
actually built** in `crates/heldar-movement`: cross-camera ReID as **probabilistic
candidate matching with human review**, an operator-defined **camera-topology graph**,
**movement trails**, and a **red-zone breach** incident engine — all under strict
privacy gates.

Implementation: `reid.rs` (the vehicle candidate proposer + scoring + plate trail),
`breach.rs` (the red-zone rule engine + subject correlation), `routes.rs` (HTTP surface
+ audited searches), `schema.sql` (its three tables), `config.rs` (knobs), `models.rs`
(`CameraLink` / `MovementCandidate` / `BreachAlert`), `lib.rs` (the privacy stance). The
kernel architecture is in [`ARCHITECTURE.md`](../ARCHITECTURE.md) §19; the
detector/tracker + ANPR worker side is documented in [`docs/AI-WORKERS.md`](AI-WORKERS.md)
and [`docs/ACCESS-CONTROL.md`](ACCESS-CONTROL.md).

Stage 6 builds **entirely on stored kernel + Access Control data** (`entry_events`,
`detections`, `zone_events`, `zones`) and adds **no ingest path and no decode**. Like
BakerySense (Stage 5), Movement is **not** a `DetectionConsumer` on the hot path — it is
a **correlation layer** of two background loops (a ReID candidate **proposer** and a
red-zone breach **rule engine**) plus an on-demand search surface, all reading data the
kernel and Access Control have *already* written. The kernel is unaware it exists.

---

## 1. Overview

```
   gate / corridor cameras (RTSP)
        │
        ▼
   media kernel + Access Control  (Stages 0–4)
        ├─► entry_events     (one canonical ANPR event per vehicle: normalized plate, attrs, direction)
        ├─► detections       (person/vehicle boxes + ephemeral ByteTrack track_id)
        └─► zone_events      (enter/exit/dwell on polygon zones, incl. restricted/red zones)
        │
        │  ── Movement reads these tables; it never sees RTSP, frames, or the ingest batch ──
        ▼
   heldar-movement (two supervised loops, every HELDAR_MOVEMENT_INTERVAL_S)
        ├─ reid::run    — propose_vehicle_candidates(): same plate on two topology-linked
        │                 cameras within a plausible transit window → fused score → movement_candidates
        │                 (status=pending; never auto-confirmed)
        └─ breach::run  — sweep(): zone 'enter' events on red/restricted zones → breach_alerts
                          (status=open), correlating track_id → plate when available
        │
        ▼
   operator review (RBAC-gated, audited)
     GET /movement/candidates  → confirm / reject   (human makes the call — ReID ≠ identity)
     GET /movement/breaches    → ack / resolve       (worked incident lifecycle)
     GET /movement/search/plate/{plate}  → plate trail + candidates   (AUDITED)
     GET /movement/search/person?camera&track&at    → weak topology+time candidates (AUDITED)
```

The product stance is wired into the code: **multi-signal, never
pure visual embedding**; **candidate matching, not identity**; **every identity-like
query audited**. Vehicle ReID is anchored on the resolved **plate**; person ReID is
**deliberately weak** (topology + time only, on demand, low confidence).

---

## 2. The privacy stance (`lib.rs`)

Movement is the most identity-adjacent app in the stack, so its design is governed by
three hard rules:

1. **Multi-signal, never pure visual embedding.** There is **no appearance/visual ReID
   embedding anywhere in this crate.** Vehicle ReID is anchored on the **plate** (already
   resolved by Access Control into `entry_events`) and fused with **transit-time
   plausibility** + **vehicle attribute agreement** (colour/type) over the operator's
   **camera-topology graph**. The plate is the dominant signal; the rest only nudge a
   plate-exact match up or down.

2. **Candidate matching, not legal identity.** Every cross-camera link is a scored
   **candidate** with per-signal evidence and a confidence in `[0,1]`, written with
   `status = 'pending'`. **Nothing is ever auto-confirmed.** A human with gate-operator
   capability confirms or rejects each one, and that decision is recorded
   (`reviewed_by` / `reviewed_at`) and audited. The schema comment says it plainly:
   *"ReID = probabilistic correlation, NOT identity."*

3. **Audited.** Every identity-like **query** — the plate trail search and the person
   candidate search — writes a kernel `audit_log` entry (`auth::audit`) before returning,
   so there is an accountable record of *who looked up whom, when*. Search responses also
   carry an explicit `note` reminding the caller the result is probabilistic and requires
   human judgement, not legal identity.

Person ReID specifically has **no plate and no embedding** here, so it is **never
auto-proposed** — it exists only as an on-demand, low-confidence search (§5). For
security, person ReID is probabilistic movement correlation, never legal identity
verification.

---

## 3. Vehicle ReID — plate-anchored, multi-signal candidate proposer (`reid.rs`)

`reid::run(pool, cfg)` is launched in `main.rs` via `spawn_supervised("movement_reid",
…)` and ticks every `HELDAR_MOVEMENT_INTERVAL_S`. Each tick calls
`propose_vehicle_candidates()` then `prune()`; `run_once()` exposes the same proposer for
the manual trigger (§8) and tests.

### 3.1 Finding pairs

The proposer scans `entry_events` for the **same normalized plate appearing on two
topology-linked cameras**, `b` later than `a`, with `b` inside the current scan window:

```sql
SELECT … FROM entry_events a
  JOIN entry_events b
    ON b.plate = a.plate AND b.camera_id != a.camera_id AND b.timestamp > a.timestamp
  JOIN camera_links l
    ON (l.from_camera = a.camera_id AND l.to_camera = b.camera_id)
    OR (l.bidirectional = 1 AND l.from_camera = b.camera_id AND l.to_camera = a.camera_id)
 WHERE a.plate IS NOT NULL AND a.plate != '' AND b.timestamp >= :scan_start
 ORDER BY b.timestamp DESC LIMIT 1000
```

- The `plate` column is the **normalized** plate Access Control committed (uppercase,
  alphanumeric-only) — the join is plate-exact.
- The `camera_links` join is what **scopes** correlation: a pair is only considered if an
  operator has declared an adjacency between the two cameras (§4). Direction is honored —
  a non-bidirectional link only matches travel `from → to`.
- An implausible time gap is rejected before scoring: `gap < 1 s` (too fast to physically
  transit) or `gap > transit_seconds × 4` (far beyond the link window) is skipped.

### 3.2 The exact fused score (`score_pair`)

Each surviving pair is scored into `[0,1]`, with the **plate as the anchor** and the
other signals only adjusting it:

| Component | Contribution |
|---|---|
| **Plate-exact anchor** | base **`0.8`** (not 1.0 — OCR can still err / plates can be cloned) |
| **Transit-time plausibility** | `+0.10` if `gap ≤ transit_seconds`; `+0.05` if `transit < gap ≤ 2×transit`; `+0.00` otherwise |
| **Colour agreement** | `+0.05` if both known and equal (case-insensitive); **`−0.10` if both known and conflict**; `0` if either unknown |
| **Vehicle-type agreement** | `+0.05` if both known and equal; **`−0.10` if both known and conflict**; `0` if either unknown |

The result is `clamp(0, 1)`. Worked corollaries:

- A plate-exact pair with a plausible transit and no attribute data scores **0.9**;
  matching colour **and** type pushes it to **1.0**.
- An attribute **conflict lowers confidence** (signalling a possible OCR misread or a
  cloned plate) but, because the plate anchor dominates, a double conflict still floors at
  ~**0.6** — above the default `HELDAR_MOVEMENT_MIN_SCORE = 0.5`. The conflict is
  surfaced in the candidate's `signals` for the reviewer rather than silently suppressing
  the link.

The `signals` JSON stored on the candidate records exactly which signals fired:
`{ "plate_exact": true, "transit_seconds": <gap>, "expected_transit": <link transit>,
"color_match": true|false|null, "type_match": true|false|null }`.

### 3.3 Writing the candidate

A pair scoring `≥ HELDAR_MOVEMENT_MIN_SCORE` is written to `movement_candidates` as
`subject_type='vehicle'`, `anchor=<plate>`, `from_*`/`to_*` referencing the two
`entry_events` rows and their cameras/times, `transit_seconds=<gap>`, the fused `score`,
the `signals` evidence, and `status='pending'`. The insert is
`ON CONFLICT(subject_type, from_ref, to_ref) DO NOTHING` — so a pair already proposed (and
possibly already **human-reviewed**) is never clobbered or reset to pending on a later
tick.

---

## 4. Camera-topology graph (`camera_links`) — and how to build it

`camera_links` is an **operator-configured directed adjacency**: *a subject leaving
`from_camera` may appear at `to_camera` within ~`transit_seconds`.* It is the spatial
prior that scopes all cross-camera matching (vehicle proposer **and** person search) — no
link, no candidate.

| Field | Meaning |
|---|---|
| `from_camera` / `to_camera` | the directed edge (camera ids; cannot be equal) |
| `transit_seconds` | expected travel time; default **120**, clamped **1…86400** on write. Drives both the plausibility window (`gap ≤ transit`, hard reject `> transit×4`) and the transit score component |
| `bidirectional` | if `1`, the edge also matches travel `to → from` |
| `note` | free-text operator annotation |

`UNIQUE(from_camera, to_camera)` prevents duplicate edges.

**Building the graph (operator workflow):**

1. Walk the site and decide which camera views are physically adjacent for the subject of
   interest (a vehicle leaving Gate A's exit view reappears at the Car Park entrance camera
   in ~90 s).
2. For each adjacency, `POST /api/v1/movement/links` with `{from_camera, to_camera,
   transit_seconds, bidirectional, note}` (requires **manage** capability; the create is
   audited as `movement_link_create`).
3. Use `bidirectional: true` for two-way corridors; keep one-way gates directed so a
   candidate is only proposed in the physically possible direction.
4. Tune `transit_seconds` from observed reality — too tight suppresses real links, too
   loose proposes implausible ones (the `×4` hard cap is the backstop). `GET
   /api/v1/movement/links` lists the current graph; `DELETE …/links/{id}` removes an edge.

> **Note:** the proposer **always** requires an explicit `camera_links` row (its query `JOIN`s it):
> a vehicle candidate is only proposed between two cameras the operator has linked. There is no
> implicit/same-site fallback — link the cameras you want correlated.

---

## 5. Person ReID — deliberately weak, on-demand only (`routes.rs::search_person`)

Person ReID has **no plate and no appearance embedding**, so it is **never auto-proposed**
into `movement_candidates`. It exists **only** as an on-demand search, and is built to be
visibly low-confidence so an operator treats it as triage, not truth.

`GET /api/v1/movement/search/person?camera=<id>&track=<track_id>&at=<RFC3339>`:

1. Audits the query (`movement_search_person`, target `track`, detail `{at}`) **before**
   computing anything.
2. Finds linked **downstream** cameras + their transit windows from the topology graph
   (the camera's `from→to` edges, plus `to→from` edges where the link is bidirectional).
3. For each linked camera, lists **distinct downstream person tracks first seen** within
   the transit window `(at, at + transit×4]`:
   `SELECT track_id, MIN(timestamp) FROM detections WHERE camera_id=? AND label='person'
   AND track_id IS NOT NULL AND timestamp > at AND timestamp <= hi GROUP BY track_id
   ORDER BY MIN(timestamp) ASC LIMIT 50`.
4. Scores each candidate on **topology + time only**: `0.4` if it arrived within the
   expected transit (`gap ≤ transit`), else `0.25`. There is **no appearance comparison**.
5. Returns the candidates sorted by score, with the standing caveat:
   *"Person ReID here uses ONLY camera topology + transit time (no plate, no appearance
   embedding). These are weak, low-confidence candidates for human triage — never identity."*

The deliberately low ceiling (max 0.4) and the absence of any auto-proposal are the
privacy design, not a missing feature: without consent/legal basis, person movement is
human-triaged correlation only.

---

## 6. Red-zone breach rule engine (`breach.rs`)

`breach::run(pool, cfg)` is launched via `spawn_supervised("movement_breach", …)` and
ticks every `HELDAR_MOVEMENT_INTERVAL_S`, calling `sweep()` (also exposed as
`run_once()` for the trigger/tests). It turns restricted-zone entries into **worked
incidents** with **subject correlation** — complementing, not duplicating, the kernel's
existing zone alerting.

### 6.1 What counts as a red zone

The sweep resolves the set of red/breach zones by **kind**: for each kind in
`HELDAR_MOVEMENT_RED_ZONE_KINDS` (default `restricted,red`) it selects
`zones WHERE kind = ? AND enabled = 1`, collecting `(zone_id, severity)`. If no zone
matches, the sweep returns immediately. So designating a red zone is just creating an
ordinary kernel zone (Stage 3) with `kind = 'restricted'` (or `'red'`).

### 6.2 Incident creation + dedup

For each red zone, the sweep reads the zone's **`enter`** events in the scan window
(`zone_events WHERE zone_id=? AND event_type='enter' AND created_at >= scan_start`) and
records one `breach_alerts` row per event with `rule='red_zone_entry'`, `status='open'`,
the zone's `severity`, the source `track_id`, and the zone event's `evidence_path` (the
Stage 3 entry-frame snapshot). The insert is **`ON CONFLICT(zone_event_id) DO NOTHING`** —
`zone_event_id` is the dedup key, so re-sweeping the overlapping window never creates a
duplicate incident for the same zone entry.

### 6.3 Subject correlation (track → plate)

Each incident is enriched by `correlate()`: a **best-effort** join from the breach's
`(camera_id, track_id)` to a **vehicle plate** in `entry_events` within **±5 minutes** of
the zone event:

- If the zone event has a `track_id` **and** a matching plated entry event exists →
  `subject_type='vehicle'`, `subject=<plate>`, `detail.correlation='track_to_plate'`.
- Otherwise (no track id, or no plated entry on that track) → `subject_type='unknown'`,
  `subject=NULL`, `detail.correlation='none'`. **Person breaches stay unknown by design**
  (no plate, no embedding to resolve them).

### 6.4 How it complements the kernel's zone alerting

The **kernel zone engine already mirrors** `warning`/`critical` zone events into the
`events` log and the Stage 1 alert webhook (ARCHITECTURE §16.3). The breach engine
deliberately **does not re-notify** — it adds a **tracked, correlated incident** on top:
a worked record (open → acknowledged → resolved), a correlated subject (plate where
possible), and a dedup'd one-row-per-entry incident queue. Real-time push stays with the
kernel; accountability + triage live here.

### 6.5 The worked incident lifecycle

```
   zone 'enter' on a restricted/red zone  (kernel ZoneEngine, Stage 3)
        │  breach::sweep() (next tick)
        ▼
   breach_alerts row  status = 'open'   (subject correlated, evidence_path carried over)
        │  POST /movement/breaches/{id}/ack     (operate_gate; audited 'breach_acknowledged')
        ▼
   status = 'acknowledged'
        │  POST /movement/breaches/{id}/resolve (operate_gate; audited 'breach_resolved';
        │                                         stamps resolved_by / resolved_at)
        ▼
   status = 'resolved'   →  eligible for retention pruning once older than retention_days
```

`GET /api/v1/movement/breaches?status=open` is the live incident queue.

---

## 7. Movement trails (plate appearances) (`reid.rs::trail_for_plate`)

A subject's **movement trail** is every appearance of a plate across cameras,
time-ordered. `trail_for_plate(pool, plate_norm)` returns the `entry_events` for that
normalized plate ordered ascending by time — each appearance carrying `event_id`,
`camera_id`, `timestamp`, `event_type`, `auth_status`, `direction`. It is surfaced
through the audited plate search (§8) and is anchored on the resolved plate, so it is the
honest, evidence-backed trail (not an inferred journey). The caller **must** audit it —
and the route does.

---

## 8. HTTP surface (`routes.rs`) — roles + audit

Reads need **`view`**, candidate/breach reviews need **`operate_gate`**, topology edits
and the manual run need **`manage`** (the kernel capability matrix from
[`docs/ACCESS-CONTROL.md`](ACCESS-CONTROL.md) §4). With `HELDAR_AUTH_ENABLED=false` every
caller is the synthetic system admin. The router takes `MovementConfig` as an `Extension`
and is `merge`d into the server.

| Method | Path | Capability (roles) | Purpose |
|---|---|---|---|
| POST | `/api/v1/movement/run` | manage (admin/manager) | Run the ReID proposer **+** breach sweep once (ops/test); both also run on the timer. `{ok:true}` |
| GET | `/api/v1/movement/links` | view (all) | List the camera-topology graph |
| POST | `/api/v1/movement/links` | manage | Create a topology link → `201`. **Audited** (`movement_link_create`) |
| DELETE | `/api/v1/movement/links/{id}` | manage | Delete a link → `204` (`404` if unknown) |
| GET | `/api/v1/movement/candidates` | view | List candidates (`status`, `anchor`=plate, `limit≤5000`); score DESC, newest-first |
| POST | `/api/v1/movement/candidates/{id}/confirm` | operate_gate (admin/manager/guard) | **Human** confirm (sets `reviewed_by`/`reviewed_at`). **Audited** (`movement_candidate_confirmed`) |
| POST | `/api/v1/movement/candidates/{id}/reject` | operate_gate | **Human** reject. **Audited** (`movement_candidate_rejected`) |
| GET | `/api/v1/movement/breaches` | view | List breach incidents (`status`, `limit≤5000`), newest-first |
| POST | `/api/v1/movement/breaches/{id}/ack` | operate_gate | Acknowledge incident. **Audited** (`breach_acknowledged`) |
| POST | `/api/v1/movement/breaches/{id}/resolve` | operate_gate | Resolve incident (stamps `resolved_by`/`resolved_at`). **Audited** (`breach_resolved`) |
| GET | `/api/v1/movement/search/plate/{plate}` | view | **AUDITED** identity search: plate trail (appearances) + candidates anchored on the plate (`movement_search_plate`) |
| GET | `/api/v1/movement/search/person` | view | **AUDITED** weak person candidates by topology+time (`?camera&track&at`; `movement_search_person`) |

Both search responses include a `note` re-stating that the result is probabilistic and
requires human judgement, not legal identity. The plate input is normalized
(uppercase, alphanumeric-only) before lookup; an empty normalized plate is `400`.

> **Privacy gate:** the two `search/*` endpoints are the identity-adjacent surface, and
> both call `auth::audit(...)` **before** querying — there is no way to run an identity
> search without leaving an audit trail. The review mutations (`confirm`/`reject`/`ack`/
> `resolve`) and `links` create are also audited; link **delete** is capability-gated but
> not currently audit-logged.

---

## 9. Data model (`schema.sql`)

`schema::init(&pool)` applies three tables idempotently (`CREATE TABLE IF NOT EXISTS`)
against the **shared kernel pool** at boot — owned by the app crate, single-tenant-per-
deployment, **correlation/candidate data only, no legal-identity records**.

**`camera_links`** — operator-configured directed adjacency (§4): `id`, `from_camera`,
`to_camera`, `transit_seconds` (default 120), `bidirectional` (0/1), `note`,
`created_at`/`updated_at`; `UNIQUE(from_camera, to_camera)`.

**`movement_candidates`** — cross-camera candidate links (ReID = probabilistic
correlation, NOT identity):

| Column | Notes |
|---|---|
| `id` | PK, `cand_<uuid>` |
| `subject_type` | `vehicle` \| `person` (only `vehicle` is auto-proposed) |
| `anchor` | normalized plate (vehicle) or `''` (person) |
| `from_camera`/`from_ref`/`from_time` | the earlier appearance (camera + `entry_events` id + time) |
| `to_camera`/`to_ref`/`to_time` | the later appearance |
| `transit_seconds` | observed gap (REAL) |
| `score` | `0..1` fused confidence |
| `signals` | JSON: which signals agreed + values (`plate_exact`/`transit_seconds`/`expected_transit`/`color_match`/`type_match`) |
| `status` | `pending` \| `confirmed` \| `rejected` |
| `reviewed_by`/`reviewed_at` | the human reviewer + time (set on confirm/reject) |
| `created_at` | |

`UNIQUE(subject_type, from_ref, to_ref)` (idempotent proposal, never clobbers a review);
indexes on `(status, score)` and `anchor`.

**`breach_alerts`** — red-zone breach incidents (one per triggering zone event):

| Column | Notes |
|---|---|
| `id` | PK, `brc_<uuid>` |
| `camera_id`/`zone_id`/`zone_name` | where it fired (zone_name denormalized) |
| `zone_event_id` | **UNIQUE** — the source `zone_event` (dedup key) |
| `rule` | `red_zone_entry` (the engine emits this today) |
| `subject_type` | `vehicle` \| `unknown` (person breaches stay unknown) |
| `subject` | correlated plate, if any |
| `track_id` | the source track |
| `severity` | inherited from the zone (`info`/`warning`/`critical`) |
| `status` | `open` \| `acknowledged` \| `resolved` |
| `detail` | JSON: `{zone_event_at, correlation: track_to_plate\|none}` |
| `evidence_path` | the zone entry-frame snapshot URL |
| `created_at`, `resolved_by`, `resolved_at` | |

Index on `(status, created_at)`. As with the kernel's event tables, neither
`movement_candidates` nor `breach_alerts` has an FK to `cameras`/`zones`/`entry_events`,
so the correlation record survives the deletion of the underlying source row
(auditability).

---

## 10. Configuration (`config.rs`)

All via `HELDAR_MOVEMENT_*` env vars, loaded by the composing server
(`MovementConfig::from_env`); the kernel `Config` carries none of them.

| Var | Default | Meaning |
|---|---|---|
| `HELDAR_MOVEMENT_INTERVAL_S` | `60` (clamp 15…600) | How often **both** the ReID proposer and the breach engine run |
| `HELDAR_MOVEMENT_SCAN_WINDOW_S` | `900` (min 120) | Lookback each tick scans for new events; must exceed the interval so no event is missed between ticks |
| `HELDAR_MOVEMENT_MIN_SCORE` | `0.5` (clamp 0…1) | Minimum fused score at which a vehicle candidate is proposed for review |
| `HELDAR_MOVEMENT_RED_ZONE_KINDS` | `restricted,red` | Comma-separated zone `kind` values treated as red/breach zones |
| `HELDAR_MOVEMENT_RETENTION_DAYS` | `365` | How long candidates + resolved breaches are kept before pruning |

---

## 11. Retention

The ReID loop runs `prune()` every tick (alongside the proposer):

- Delete `movement_candidates` with `created_at < now − retention_days` (`max(1)` day
  floor) — **all** candidates age out by creation time, reviewed or not.
- Delete `breach_alerts` with `created_at < now − retention_days` **AND `status =
  'resolved'`** — only **closed** incidents are pruned; an `open`/`acknowledged` breach is
  retained until it is worked, no matter its age.

The app owns its own data lifecycle here — this is **not** the kernel retention sweeper,
and recording segments / evidence-lock are untouched.

---

## 12. How it composes (composed, not welded) + isolation

Movement is wired in `crates/heldar-server/src/main.rs` purely as a bundled app: its
schema is applied after the kernel migrations (`heldar_movement::schema::init`), its
config is loaded from the environment (`MovementConfig::from_env`), its two loops are
`spawn_supervised("movement_reid", …)` and `spawn_supervised("movement_breach", …)`, and
its router is `merge`d. Crucially it is **absent from the `consumers` vec** — it is **not**
a `DetectionConsumer`, so it never runs on the ingest request.

Because both engines read stored tables on their own timer (and the searches are
on-demand reads), a slow or crashed Movement loop **cannot back-pressure** ingest,
recording, the sampler, or live view — a panic just respawns the loop after 5 s (Stage 1
supervision). Adding Movement is a link + `merge` + two `spawn_supervised` calls with
**zero** change to the kernel ingest handler — the same "kernel-open, apps-bundled" seam
as BakerySense, now correlating *across* cameras instead of *within* one.

---

## 13. Honest scope — what is built, what is deliberately not

**Built (production-grade engineering):** plate-anchored multi-signal candidate proposal
with the exact fused scoring + transit gating, the human confirm/reject review workflow,
the operator camera-topology graph, plate-trail + low-confidence person search (both
audited), the red-zone breach rule engine with `zone_event_id` dedup and track→plate
subject correlation, the worked incident lifecycle, the schema, retention, and the full
RBAC-gated API.

**Deliberately not built (honest deferrals):**

- **No visual/appearance ReID embedding** anywhere. Vehicle ReID is **anchored on the
  plate** (+ transit/colour/type); person ReID is **weak, topology + time only**, on demand
  and human-triaged. This is the privacy stance, not a gap to "fix"
  with a face/appearance model.
- **No homography / ground-plane calibration.** Transit windows are operator-declared
  `transit_seconds` per link, not geometry-derived; there is no metric speed/distance model.
- **ReID accuracy is unbenchmarked on local footage** (false-link / missed-link / path
  accuracy). The *engineering* is complete; *accuracy* is an evaluation
  task gated on collecting local data — and by design the human review gate is the
  safeguard, never an auto-decision.
- **Cross-camera person journeys are low-confidence human-triage only** — never
  auto-proposed, capped at 0.4, and always audited.
- **No implicit/same-site link fallback** — the proposer only correlates cameras joined by an
  explicit `camera_links` row (§4).
