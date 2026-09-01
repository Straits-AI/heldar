---
name: heldar-ai-worker-diagnostics
version: 1.0.0
summary: Localise an AI fault to the layer that actually failed — capture, sampling, lease, ticket, inference, model dependency, ingest, database or downstream consumer — naming exact worker, task and camera ids.
compatible:
  core_api: ">=0.1.0 <1.0.0"
permitted_tools:
  - heldarctl doctor
  - get_system_health
  - list_cameras
  - get_camera_health
  - list_ai_workers
  - get_timeline
  - get_recording_gaps
  - get_retention_limits
prohibited_actions:
  - actuate a gate, relay or PTZ
  - delete recordings, evidence or weaken retention
  - create, modify or retrieve credentials
  - identify a person from appearance similarity alone
  - assert that nothing happened without first checking recording gaps
  - present a correlation or hypothesis as an observation
---

# AI worker diagnostics

## Purpose

Turn "the AI is broken" into a named layer and a set of ids: which camera, which `ai_` task, which
worker, and which of the nine links in the chain from photons to a zone event actually failed.

The failure this exists to prevent is blaming the model for something upstream of it. The chain is
**capture → sampling → lease → ticket → inference → model dependency → ingest → database →
downstream consumer**, and only one of those nine is the model. The commonest wrong answer is
"the detector has stopped working" when the sampler is `offline` and no frame has been produced for
hours. It is a *comfortable* wrong answer because the dashboard still shows the camera green: the
recorder and the sampler read **different streams through different processes**, so a camera can
record 24/7 and feed the AI nothing at all. A recorder that is healthy is not evidence that frames
exist.

The second wrong answer this prevents is reading "no detections" as "nothing was there". At the
0.5 fps floor a three-second event can fall entirely between two sampled frames.

## Inputs

- The complaint, in the words it was reported in.
- At least one of: a camera id, an AI task id (`ai_…`), a worker id (`<host>:<pid>`).
- The window the fault is claimed for, and whose clock it is stated in.
- The downstream symptom, which decides where to look: no detections at all, no zone events, a gate
  that did not open, or a semantic search returning nothing.

## Prerequisites

- A credential with `ai:tasks` (for `list_ai_workers`), `camera:read` and `system:read`. A
  camera-scoped credential is filtered, not refused, so a short sampler list may mean a narrow scope
  rather than a stopped sampler. Check `list_cameras` before reading absence as a fault.

**Tools this job needs and does not have.** Design around these rather than guessing past them:

- **No read tool for AI tasks.** `GET /api/v1/cameras/{id}/ai-tasks` is not exposed. You cannot see
  `task_type`, requested `fps`, requested `width`, `enabled`, or the `config` blob that holds the
  model, threshold and class filter. Every one of those must come from the human.
- **No read tool for results.** Detections, zone events, embeddings and the kernel events log are
  all unexposed. You cannot confirm that a detection exists, only reason about whether a frame could
  have reached a worker and whether a result could have survived the path.
- **No lease view.** `list_ai_workers` is named for workers but returns **sampler** rows —
  `camera_id`, `stream_profile`, `state`, `fps`, `width`. No worker id, no lease holder, no ingest
  count, no error string.
- **No history.** Sampler state is held in the kernel process and is current-only. A fault that
  ended before you looked is invisible, and a kernel restart wipes the state entirely.

So this skill localises a fault to a **layer** and emits the exact query a human with the right
scope must run next. It does not confirm any model's output.

## Workflow

Ordered so the cheapest disqualifying fact comes first. Stop at the first layer that faults; do not
keep collecting evidence for a tidier story.

1. **Check the contract before trusting any field.** `heldarctl doctor`. On
   `compat.major_mismatch`, stop: every shape you read below may be parsed wrong, and a confident
   wrong diagnosis is worse than none.

2. **Ask whether frames exist at all.** `list_ai_workers`, and record `state`, `fps`, `width` and
   `stream_profile` per row. Rows are one per **(camera, stream_profile)**, not one per camera: a
   camera with a `sub` task and a `main` task appears twice, and one row may be healthy while the
   other is not. Match the row's `stream_profile` to the task before drawing any conclusion.
   - **Camera absent from the list** → no sampler is running for it. Classify **sampling**, never
     inference. Three causes with three different fixes: AI is off box-wide
     (`HELDAR_AI_ENABLED=false` makes rebalance a no-op), the camera has no enabled AI task, or the
     camera itself is disabled. You cannot tell them apart here (see Stop conditions).
   - **`state` is `budget_exhausted`** → the row was shed: the global AI budget
     (`HELDAR_AI_MAX_TOTAL_FPS`, default 40, floored at 1.0) could not seat it even at the cheapest
     ladder rung, so it fell below the priority cut. `fps` reads `0.0`, `width` reads `0`, and **no
     frame is ever produced for it**. Classify **sampling**. This is the shape most easily misread as
     a dead model: the camera is enabled, the task is enabled, the recorder is green, and nothing is
     sampled. Shedding runs from the bottom of the camera `priority` order, so this row is one of the
     lowest-priority AI cameras on the box.
   - **`state` is `offline`, `error` or `connecting`** → classify **capture**. The stream named by
     `stream_profile` is not decoding. `connecting` that never advances is a stuck source, not a
     warm-up. `error` is narrower than it looks: it means no URL could be built for that profile (and
     no record URL to fall back to), or ffmpeg failed to spawn — the second is a box fault, not a
     camera one.
   - **`state` is `stopped`** → the supervisor was told to stop. Not a capture fault; expect a
     reconcile (any AI-task create/update/delete, or boot) to have moved it.
   - **`state` is `sampling` but `fps` reads `0.5`** → this row is on the `MIN_FPS` floor. Allocation
     is **priority-ordered**, not an even split: rows are sorted by camera `priority` descending, each
     takes its requested fps at its requested width while that fits after reserving a floor for those
     behind it, and the rest get the floor. Consecutive frames are 2 s apart. Coverage fact, not a
     fault, and not evidence about the model.
   - **`width` below the requested width** → the resolution ladder stepped this row down under budget
     pressure (100% → 75% → 50% of requested, never below 320 px), which is only ever granted
     together with the 0.5 fps floor. Small objects and plates degrade first, and a missed plate here
     is a resolution fact. You cannot see the requested width from this surface, so a stepped-down
     row is only identifiable as `fps == 0.5` plus a `width` the human recognises as under-spec.

3. **Separate capture from sampling.** `get_camera_health` for the same cameras. Its `state`,
   `last_error`, `reconnect_count` and `last_segment_at` describe the **recorder** on the record
   stream, which is a different process reading a different URL.
   - recorder `recording` + sampler `offline` → the **sampled profile specifically** is failing, e.g.
     the `sub` profile is disabled on the device. Classify **capture**, and scope the wording to that
     profile. Do not report the camera as down; it is recording.
   - recorder `offline`/`error` + sampler `offline` → the device or the network. Classify
     **capture**.
   - recorder `disabled` → the recorder is not running, for one of two reasons: the camera is
     administratively disabled, or it records on a schedule/trigger and is currently idle. These are
     not the same for AI. A **disabled camera** has no sampler either (rebalance requires
     `cameras.enabled = 1`), so the AI silence follows. A **schedule-idle camera is still sampled** —
     the sampler does not read the recording schedule — so `disabled` here is not an explanation for
     AI silence and you must keep going. Ask the human which of the two it is; `get_camera_health`
     does not distinguish them.

4. **Only once frames are established for the whole window**, read `get_system_health` for
   `enforcement.ingest_provenance`, `enforcement.frame_tickets_required` and `uptime_seconds`.
   - `frame_tickets_required: true` → every worker that does not lease and carry a frame ticket gets
     `401 frame_ticket_required` on every post. Recording, health and the dashboard all look
     perfect while nothing is ingested. Classify **ticket**. This is the likeliest cause of a
     fleet-wide AI silence that begins at a deploy.
   - `ingest_provenance: warn` (the default) → ticketless ingest is accepted, so the tier is not
     your explanation. Look further.
   - `uptime_seconds` shorter than the window → the box restarted. Tickets do not survive a restart
     and sampler state resets, so a burst of failures immediately after is expected and
     self-healing. Do not classify it as a fault.

5. **Split lease from ticket from ingest by the *shape* of the silence**, since you cannot read
   leases directly.
   - One worker's tasks silent while others are fine → **lease**. Either a second credential holds
     the task (a task is leased to one holder; the loser gets an empty `tasks` array, not an error),
     or — a separate mechanism with the same symptom — the worker stopped polling `/ai/tasks` with
     its `worker_id` for over 60 s, was pruned from the live set, and its shard was reassigned.
   - Every worker silent from one instant → **ticket** or the credential itself.
   - Some task types landing and others not → **inference** or **model dependency**, step 6.
   - **ingest** has three quiet rejections worth naming: an `event_type` beginning `gate_`,
     `entry_`, `zone_`, `camera_`, `disk_` or `raid_` is rejected `400`; worker severity is clamped
     to `info`/`warning`, so a worker cannot raise `critical`; and a redelivered frame is a silent
     no-op answering `{"detections_ingested": 0, "duplicate": true, "ticketed": …}`. A worker
     retrying a batch sees zeros and reports "not ingesting" when the first delivery succeeded.

6. **Check model dependency before inference.** These degrade to silence with no error anywhere:
   - `anpr` with neither PaddleOCR nor EasyOCR installed → vehicles ingest with `vehicle_type` and
     `color` but **never a plate**, by design; the analyzer refuses to fabricate one. The gate never
     matches. Nothing is broken and nothing was ever installed.
   - `embedding` without `open_clip` → the task falls back to the placeholder analyzer, no vectors
     are indexed, and semantic search returns a fast `503`.
   - Semantic search returning **zero hits with no error** is usually a checkpoint mismatch: the
     ranking is prefiltered by the embedding model id, so a task indexing under one
     `clip_model`/`clip_pretrained` and a query worker using another honestly return nothing. Zero
     hits never means the object was absent.
   - A `task_type` with no registered analyzer → the placeholder runs forever, consumes frames, and
     **never fabricates a detection**. A task named for a capability nobody wired in looks exactly
     like a broken model.

7. **Check downstream consumers last.** Detections can be stored and still drive nothing:
   - Zone events require **both** `track_id` and `bbox`. A `motion` analyzer posts neither track
     ids, so it fills the detections table and raises zero zone events, correctly.
   - A zone's `labels` filter, `enabled: false`, its polygon, and the use of the bbox
     **bottom-centre** ground point rather than its centroid each exclude silently.
   - ANPR needs `anpr_min_votes` distinct frames per `(track, plate)`, one vote per frame and one
     frame per batch. Below threshold the entry event is still written but marked
     `workflow_status: "review"` with a `gate_review_not_actuated` event, and the barrier does not
     open. **A gate that did not open, with an event in the log, is a working vote threshold, not a
     fault.**

8. **Consider database only as retention, and only with human confirmation.** Embeddings ride the
   same detections TTL (`HELDAR_DETECTION_RETENTION_HOURS`, default 168 h), and when the DB size-cap
   fires it sheds in a fixed order — transient search-query rows, then oldest embeddings, then
   detections — so "last month's detections are gone" is retention rather than loss, and semantic
   search losing old vectors before old detections is the cap working. `get_retention_limits`
   reports the **recordings** size cap and free-disk floor **only** — it does not report the detections TTL or
   the DB cap, so you cannot confirm this from the permitted surface. Classify **database** only
   once a human confirms the row age against the TTL. Sustained `5xx` on ingest under recorder load
   is the other database shape, and is likewise invisible here.

9. **Before writing any sentence containing "nothing was detected"**, run `get_timeline` and
   `get_recording_gaps` for every camera in scope. Sampler state is current-only, so gaps are the
   only historical coverage evidence available to you.

10. **Report ids, not roles.** Every finding carries the `camera_id`, the `ai_` task id and the
    worker id as supplied. Where you were not given an id, write `unknown` rather than describing
    the thing in prose.

## Stop conditions

Stop and hand to a human when:

- **No row in `list_ai_workers` has this `camera_id`, and no `ai_` task id was supplied in Inputs.**
  "No task exists", "the task is disabled" and "AI is off box-wide" are three different remediations
  and the permitted surface cannot separate them.
- **`window.to` is earlier than `observed_at`, and every row you read has `state == "sampling"`.**
  Sampler state is current-only; reporting it would be offering the present as evidence about the
  past.
- **`uptime_seconds` (seconds) is less than `observed_at − window.from` (seconds).** The kernel
  restarted inside the window. Every sampler state and every frame ticket you would reason about
  belongs to a different process.
- **The complaint is that something was missed, and any row for a camera in scope has `fps <= 0.5`
  or `state == "budget_exhausted"`.** At the 0.5 fps floor consecutive frames are 2 s apart, and at
  `budget_exhausted` there are none. You cannot show a model missed an object that may never have
  appeared in a sampled frame. Report the coverage fact and stop.
- **`get_recording_gaps` returns a gap whose interval intersects `[window.from, window.to]` for any
  camera in scope.** Report the gap; do not answer over it.
- **`classification.layer` would be `inference`.** That is the residual bucket, reached by
  elimination and never by evidence, because no permitted tool reads a detection back. Emit
  `"undetermined"` with `reached_by: "elimination"`, say that every layer upstream of the model
  checks out, and hand over. Never say the model is at fault.
- **The question asks who a person is, or whether a plate read should have opened a barrier.** Plate,
  colour, make and model are assistive and are not benchmarked on local footage; a gate decision
  belongs to a human.
- **The answer would be a change** to an AI task, a lease, a zone, an environment variable or a
  credential. This skill classifies; it does not fix, and every tool it holds is `GET`.
- **`heldarctl doctor` reports a finding with `code == "compat.major_mismatch"`.**

## Output

```json
{
  "complaint": "…",
  "window": {"from": "UTC", "to": "UTC", "site_local_note": "…"},
  "observed_at": "UTC",
  "subject": {"camera_ids": ["…"], "ai_task_ids": ["ai_…"], "worker_ids": ["…"]},
  "classification": {
    "layer": "capture|sampling|lease|ticket|inference|model_dependency|ingest|database|downstream_consumer|undetermined",
    "confidence": "low|medium|high",
    "reached_by": "evidence|elimination"
  },
  "layers_checked": [
    {"layer": "…", "verdict": "ok|faulted|not_checkable", "evidence": ["…"], "evidence_ids": ["…"]}
  ],
  "sampler_rows":  [{"camera_id": "…", "stream_profile": "sub|main",
                     "state": "connecting|sampling|offline|error|stopped|budget_exhausted",
                     "fps_effective": 0.0, "width_effective": 0, "fps_requested": "unknown"}],
  "recorder_rows": [{"camera_id": "…", "state": "recording|offline|error|disabled",
                     "last_error": "…", "reconnect_count": 0, "last_segment_at": "UTC"}],
  "enforcement": {"ingest_provenance": "off|warn|enforce", "frame_tickets_required": false,
                  "box_uptime_seconds": 0},
  "coverage": {"cameras": [{"id": "…", "gaps": [{"from": "UTC", "to": "UTC"}]}]},
  "not_checkable": ["…"],
  "hypotheses": [{"statement": "…", "rests_on": ["…"], "how_to_test": "the exact command or query"}],
  "next_human_action": "…"
}
```

Every timestamp in the structured output is **UTC**, including `observed_at`. Site-local times may
appear in prose and must carry their zone. If the site has no timezone configured, the box reads
relative times as UTC; say so rather than assuming the reporter meant local.

`fps_requested` is always `"unknown"`: the permitted surface returns the effective rate only.
`sampler_rows` and `recorder_rows` are **current at `observed_at`**, not measurements of the window;
label them that way in prose too.

`classification.layer: "undetermined"` is a valid and often correct answer. `"inference"` is in the
enum because it is one of the nine links, **not** because this skill may emit it: no permitted tool
reads a detection back, so the model can never be shown at fault from here. Elimination down to the
model means `"undetermined"` plus a `layers_checked` list showing every upstream layer `ok`.
`reached_by: "elimination"` may not be paired with `confidence: "high"`.

`not_checkable` is not optional padding: it is where the AI task config, the detections themselves,
the lease table, the events log and the detections TTL go, and it is what keeps the reader from
believing you checked them.

## Security notes

This skill reads. The sampler roster leads with `camera_id`, which makes it a camera roster in
disguise; a scoped credential sees only its own cameras, and you must not describe the list as the
fleet.

Never copy a frame URL or an `x-frame-ticket` into the output. A ticket names one captured frame and
is a capability; a capability in a transcript has left the building.

Include the request or correlation id from any failed tool call, so a support engineer can join your
classification to the box's own logs.
