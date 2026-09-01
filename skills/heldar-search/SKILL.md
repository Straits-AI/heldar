---
name: heldar-search
version: 1.0.0
summary: Turn a question into a query plan, show the plan before it runs, and answer with rows and evidence ids rather than prose.
compatible:
  core_api: ">=0.1.0 <1.0.0"
permitted_tools:
  - get_system_health
  - list_cameras
  - get_camera_health
  - get_timeline
  - get_recording_gaps
  - list_ai_workers
prohibited_actions:
  - actuate a gate, relay or PTZ
  - delete recordings, evidence or weaken retention
  - create, modify or retrieve credentials
  - identify a person from appearance similarity alone
  - assert that nothing happened without first checking recording gaps
  - present a correlation or hypothesis as an observation
---

# Search

## Purpose

Answer "which white vans came through the back gate after 6pm last week" with rows an operator can
open, and with an explicit statement of which clock "6pm" was read on.

The specific wrong answer this exists to prevent is this sentence:

> "Three white vans came through the back gate after 6pm last week; the 21:40 one was unauthorised."

A model can produce it having run no query at all. It can also produce it from a query that *did*
run and answered a different question — "6pm" read on UTC at a site eight hours ahead selects 02:00
the next morning, and the rows look entirely plausible — or from a window in which the recorder was
down, where zero detections and zero events are the same empty list. Search's governing rule is that
a model is a query **planner** and the executed rows are the answer. Zero rows is a result. Zero rows
from a camera that was not recording is not a result at all, and the two look identical unless you
check.

## Inputs

- A question in prose, or a `QueryPlan` if the user already has one.
- Camera names as the user says them. You resolve them to ids; never put a name where an id goes.
- Any window in the user's own wall clock, together with which clock that is.

## Prerequisites

**`heldar-mcp` exposes no search tool.** The four search routes are POSTs (`/api/v1/search/plan`,
`/nl`, `/events`, `/semantic`) and the sidecar is GET-only by construction, so there is no phrasing
that makes it execute a search. Under this skill you *plan* and you *check coverage*; executing needs
a credential holding the `events:read` capability against the kernel HTTP API, held by the user or by
another client. When nobody in the loop has one, the deliverable is the plan plus the coverage
report, said plainly — not a description of results you never fetched.

- For the search routes: `events:read`. A camera-scoped credential is additionally confined to its
  own cameras on every one of the four routes.
- For the tools this skill permits: `system:read` (`get_system_health`), `camera:read`
  (`list_cameras`, `get_camera_health`), `video:playback` (`get_timeline`, `get_recording_gaps`),
  `ai:tasks` (`list_ai_workers`). These are separate grants from `events:read`; holding one is not
  holding another.
- No permitted tool reports the effective timezone. `GET /api/v1/system/timezone` gives the
  **box-wide default only** — not a site's — and is not exposed here either. The zone an answer is
  computed on is reachable one way without executing: `POST /api/v1/search/plan` returns
  `interpretation.timezone` and `interpretation.timezone_source` for the question, resolved exactly
  as `/nl` would. That dry run is how you settle the clock before any rows are read.

## Workflow

1. **Resolve camera names to ids** with `list_cameras`. The rule planner matches camera names out of
   the question text, longest first; a name that is not some camera's `name` or `id` matches nothing
   and the plan silently reads the whole fleet. Put ids in the plan yourself. Note each camera's
   `site_id` while you are here — step 2 needs it.
2. **Settle the clock before writing an hour filter.** `hour_min`/`hour_max` and the relative dates
   (`today`, `yesterday`, `last week`) are wall-clock notions, resolved in this order: an explicit
   `tz` on the plan, then the single zone shared by the plan's cameras (their site), then the
   box-wide default, then **UTC**. UTC is the historical fallback for a box nobody has configured,
   and "UTC" and "nobody has said" look the same in a timestamp. Two consequences worth stating to
   the user before running anything:
   - An hour filter across cameras whose effective zones **differ** is a `400`, deliberately. Do not
     route around it by dropping the filter or by choosing a zone; pass an explicit `tz`, or search
     one site at a time. You cannot read each site's zone from here, but you can see the risk:
     more than one distinct `site_id` among the plan's cameras means the refusal is possible (two
     sites may still share a zone, in which case it is not refused). A camera with no `site_id`
     contributes the box default rather than a disagreement.
   - That refusal fires **only** when `hour_min` or `hour_max` is set. "Yesterday" across mixed-zone
     cameras is *not* refused: it is answered on UTC day boundaries and labelled
     `timezone_source: not_time_of_day`. If where the day starts matters to the question, pass `tz`.
3. **Show the plan before it runs.** `/search/plan` is a dry run — no rows, no log, no audit — and is
   the right tool for this step. It echoes `planner` (`rules` or `llm`), the plan, and the
   `interpretation` block. Present the plan JSON and what each field will do. The parser is
   best-effort: whatever it could not read becomes an unconstrained default window (the last 7 days,
   every source, every camera in scope) that returns plausible rows about a different question, and
   the plan is the only place that is visible.
4. **Establish coverage before you interpret an empty result.** Do this before executing, not after:
   a hole changes what an empty result means, and finding it afterwards invites fitting it to an
   answer already written. Three tools, three different facts:
   - `get_timeline` is the coverage answer. It returns `ranges` (`start`, `end`, `seconds`),
     `recorded_seconds` and `segment_count`. The sidecar sends no `from`/`to`, so you get the
     camera's whole retained timeline and must intersect the plan's window yourself. A minute of the
     window inside no range is a minute with no footage. (Breaks shorter than 2 seconds are coalesced
     away, so the ranges are contiguous coverage, not a segment list.)
   - `get_recording_gaps` is **not** the complement of that timeline. It returns the persisted ANR
     `recording_gaps` rows — the newest 500, each with `gap_start`, `gap_end`, `gap_seconds` and a
     `fill_state` of `pending` | `filled` | `failed` — which is what the automatic replenishment loop
     knows about and has acted on. A `filled` gap is no longer a hole; a `pending` or `failed` one
     is. The computed coverage holes live at `/api/v1/cameras/{id}/gaps`, which this sidecar does not
     expose. Use the gap rows to explain and date a hole, the timeline to prove one.
   - `get_camera_health` is the recorder's state **now** — `recording` | `offline` | `error` | `idle`
     | `disabled`, plus `last_segment_at`, `reconnect_count` and `last_error`. There is no historical
     health tool here. Do not report a state during the window that you did not observe; report the
     state you read, the time you read it, and let the timeline speak for the window.
5. **Execute** (or hand the plan to whoever holds `events:read`) and read the whole envelope, not the
   hits: `planner`, `plan` as it actually ran, `interpretation.timezone` and `timezone_source`,
   `count`, `truncated`, and `proof`. On a natural-language answer the proof layer marks exactly one
   step fallible — how the question was read — and that step is yours to check, not the system's. A
   structured `/search/events` call has no such step and no inference level at all.
6. **Diff the echoed plan against the plan you showed.** On the natural-language route a camera the
   planner named but the credential does not hold is dropped *silently*, and if nothing survives, a
   scoped credential falls back to its **own** cameras — so the answer can be narrower than the
   question without saying so. On the structured route naming an unheld camera is a `403`, refused
   whole. An empty `cameras` means every camera **this credential can see**, so a scoped
   credential's "nothing at Gate B" may only mean Gate B is out of scope.
7. **Keep the evidence links.** Per hit: `source` (`entry` | `zone` | `breach`), `id`, `timestamp`
   (UTC), `camera_id`, `evidence_path`. Footage is pulled by taking that timestamp to the kernel clip
   API, `POST /api/v1/cameras/{camera_id}/clip`; the snapshot is `evidence_path`. A hit rewritten as
   prose without its ids cannot be checked by anyone.
8. **Semantic hits are a ranking, not facts.** `/search/semantic` returns detection crops
   cosine-ranked in CLIP space; `score` is a similarity, never a probability, and here the proof
   layer marks the *ranking* fallible rather than the reading of the question. Two things this skill
   cannot check for you and must therefore say out loud: the embedding analyzer's default classes are
   **vehicles only**, so no semantic hit for a person means person crops were never embedded, not
   that no person was there; and `list_ai_workers` reports the frame **sampler** per camera
   (`camera_id`, `stream_profile`, `state`, `fps`, `width`) — it does not name task kinds, so it
   cannot confirm an `embedding` task is running. A camera absent from it is being sampled for
   nothing and is indexing nothing new; a camera present may still have no embedding task. A `503`
   means the embedding worker is offline or still warming up (CLIP loads lazily on the first query
   after a restart), which is again not zero hits.

## Stop conditions

Stop and hand to a human when:

- `get_timeline` shows any part of the plan's window outside every recorded range on a camera in the
  plan, or a `recording_gaps` row whose `[gap_start, gap_end)` intersects that window has
  `fill_state` of `pending` or `failed`, and the question depends on that interval. Report the
  interval; do not report the empty result set as an answer.
- The response says `truncated: true`. A source returned as many rows as the fetch cap, so the field
  filters ran over a newest-first slice and older in-window matches may be missing. `count` is then
  not the number of matching events. Narrow the window or the camera list and run it again. (On a
  semantic answer the same flag means the candidate scan hit its 100k-row cap: the ranking covers the
  newest 100k crops, not all of them.)
- The plan sets `hour_min` or `hour_max` and its cameras carry **more than one distinct `site_id`**.
  Either the API refuses it with a `400`, or two sites happen to share a zone and it does not — and
  you cannot tell which from here. Get an explicit `tz`, or search one site at a time. Do not drop
  the hour filter to make the call succeed.
- `interpretation.timezone_source` comes back `utc_fallback` and the question contains a wall-clock
  notion (an hour filter, or `today` / `yesterday` / `last week`). Nothing is configured, the answer
  is on UTC, and it will be shifted by the site's offset while looking entirely convincing. Get a
  zone configured, or an explicit `tz`, before answering.
- The echoed `plan` differs field-for-field from the one you showed — a camera missing from
  `cameras`, a changed `from`/`to`, an `hour_min` that moved, a `tz` you did not pass. The question
  that was answered is not the question you presented.
- The question needs something no `QueryPlan` field can express. The fields are `from`, `to`,
  `hour_min`, `hour_max`, `tz`, `cameras`, `sources`, `plate`, `color`, `vehicle_type`,
  `subject_type`, `auth_status`, `event_type`, `zone_kind`, `text`, `limit` — every one a filter over
  single stored events. A dwell threshold, a join across sources, "left without buying", or counting
  distinct people is none of them. Name what the plan cannot express instead of answering the nearest
  question it can. (The plan also carries `zone`, but it scopes `/search/semantic` only: the
  structured executor ignores it and the planner clears it, so a zone-id on an `/events` or `/nl`
  plan filters nothing.)
- The question is **who**. A plate is a plate; appearance similarity and CLIP proximity are not
  identity and this skill does not turn them into one.
- Nobody in the loop holds an `events:read` credential. Deliver the plan and the coverage, and say
  the search was not run.
- The user asks you to confirm that nothing happened. You can report that no matching row was
  recorded, over stated coverage — a different sentence, and the only one the data supports.

## Output

```json
{
  "question": "…",
  "planner": "rules|llm|structured|clip|not_run",
  "plan": {"from": "2026-08-24T00:00:00Z", "to": "2026-08-31T00:00:00Z",
           "hour_min": null, "hour_max": null, "tz": "IANA or null",
           "cameras": ["…"], "sources": ["entry|zone|breach"], "plate": null, "limit": 200},
  "plan_shown_before_execution": true,
  "clock": {"timezone": "…",
            "timezone_source": "explicit|site|default|utc_fallback|not_time_of_day",
            "hour_filter_read_in": "…",
            "known_from": "plan_dry_run|executed_response|not_established"},
  "executed": true,
  "result": {"count": 0, "truncated": false,
             "hits": [{"source": "entry|zone|breach", "id": "…",
                       "timestamp": "2026-08-27T13:40:02Z", "camera_id": "…",
                       "evidence_path": "…"}]},
  "coverage": {"cameras": [{"id": "…",
                            "uncovered": [{"from": "2026-08-27T02:00:00Z",
                                           "to": "2026-08-27T02:14:00Z"}],
                            "unfilled_gaps": [{"id": "…", "gap_start": "…", "gap_end": "…",
                                               "fill_state": "pending|failed"}],
                            "health_when_checked": {"state": "recording|offline|error|idle|disabled",
                                                    "at": "2026-08-31T09:12:00Z"}}],
               "window_fully_covered": false},
  "not_expressible": ["…"],
  "unknowns": ["…"],
  "next_human_action": "…"
}
```

**Every timestamp in this object is UTC**, in `plan`, in `coverage` and in every hit — write them
with a trailing `Z` and never substitute a local rendering. `clock.timezone` says only how wall-clock
*inputs* were read; it never changes what a stored timestamp means. Site-local times may appear in
prose for the reader, labelled with the zone.

`uncovered` is what you derived by intersecting the window with `get_timeline`'s ranges;
`unfilled_gaps` is the ANR rows that explain it. `health_when_checked` is a reading with a time on
it, not a claim about the window.

`count: 0` with `window_fully_covered: false` is a non-answer and must be presented as one.
`count: 0` with full coverage is a real negative: no matching row was recorded. Those are different
answers, and telling them apart is what the `coverage` block is for.

For a semantic run, hits carry `score`, `label`, `track_id` and `bbox` instead of the event fields,
and `evidence_path` may be null; report them as a ranking, never merged into the event rows.

If `executed` is false, omit `result` entirely. Do not fill it with the rows you expect.

## Security notes

Every executed search writes a `search_log` row — actor, mode, the verbatim question, the executed
plan, the planner and the result count. An identity-bearing query is additionally written to the
kernel `audit_log` as `search_identity_query`: that means a plan carrying a `plate`, a plan whose
free-`text` field normalises to a plate-like token, and a semantic text query that does the same.
There is no quiet plate lookup; tell the user that before running one. The `/search/plan` dry run
executes nothing and writes neither record, so use it freely.

The permitted tools redact stream URLs, tokens and usernames before anything reaches your context;
camera addresses deliberately survive. Include the request or correlation id of any failed call in
the output, so a support engineer can join your plan to the box's own logs.
