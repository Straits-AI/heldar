---
name: heldar-incident-triage
version: 1.0.0
summary: Build a factual timeline for an incident, camera or time window, separating what was observed from what is inferred.
compatible:
  core_api: ">=0.1.0 <1.0.0"
permitted_tools:
  - get_system_health
  - list_cameras
  - get_camera_health
  - get_timeline
  - get_recording_gaps
  - get_incident
prohibited_actions:
  - actuate a gate, relay or PTZ
  - delete recordings, evidence or weaken retention
  - create, modify or retrieve credentials
  - identify a person from appearance similarity alone
  - assert that nothing happened without first checking recording gaps
  - present a correlation or hypothesis as an observation
---

# Incident triage

## Purpose

Turn "something happened around 2am on the loading bay" into a timeline someone can act on: what the
system actually recorded, what it did not, and what remains unknown.

The failure this exists to prevent is a confident, tidy narrative built over a gap. A recorder that
was offline produces no detections, and no detections reads exactly like nothing happened.

## Inputs

- One of: an incident id, a camera id, or a time window.
- A time window in the **site's** local wall clock, if the user gave one that way.

## Prerequisites

- A credential with `events:read` and `camera:read` for the cameras in question.
- The site's timezone. If none is configured, the box interprets relative times in UTC — say so in
  the output rather than assuming the user meant local.

## Workflow

1. **Resolve the clock first.** `get_system_health` reports `api_version`; the box's timezone comes
   from its posture and site configuration. A window stated as "2am" is meaningless until you know
   whose 2am. If the site has no zone configured, state that the window was interpreted as UTC.
2. **Establish coverage before looking for events.** `get_timeline` for each camera in scope, then
   `get_recording_gaps`. Do this *before* querying detections, not after: knowing there is a
   14-minute gap changes how you read an empty result, and doing it afterwards invites fitting the
   gap into a story you have already formed.
3. **Check the recorder's own health.** `get_camera_health` — a camera in `error` or `offline` at
   the relevant time is a fact about the evidence, not a footnote.
4. **Collect evidence ids**, not descriptions. Segment ids, incident ids, detection ids. A timeline
   that says "a vehicle was seen" without an id cannot be checked by anyone.
5. **Sort each statement into one of three kinds**, and label it:
   - *Observed* — the system recorded this. A segment exists; a detection has an id.
   - *Correlated* — two observations line up in time or space. Not causation, not identity.
   - *Hypothesis* — your inference. Always attributed to you, never to the system.
6. **State the unknowns explicitly**, including every gap found in step 2 and every camera that was
   not healthy.

## Stop conditions

Stop and hand to a human when:

- The window overlaps a **recording gap** and the question depends on what happened during it. You
  cannot answer it; say so.
- The question is **who** rather than **what**. Appearance similarity and ReID are probabilistic;
  identifying a person from them is out of scope for this skill and for this product's claims.
- A camera in scope was **unhealthy** for a material part of the window.
- The user asks you to **confirm** that nothing happened. You can report that nothing was *recorded*,
  which is a different sentence and the only one the data supports.
- Answering would require a tool this skill does not permit.

## Output

```json
{
  "window": {"from": "UTC", "to": "UTC", "site_local_note": "…"},
  "coverage": {
    "cameras": [{"id": "…", "recorded_seconds": 0, "gaps": [{"from": "…", "to": "…"}],
                 "health_during_window": "recording|offline|error|unknown"}]
  },
  "observed":    [{"statement": "…", "evidence_ids": ["…"], "at": "UTC"}],
  "correlated":  [{"statement": "…", "evidence_ids": ["…"], "confidence": "low|medium|high"}],
  "hypotheses":  [{"statement": "…", "rests_on": ["…"], "how_to_test": "…"}],
  "unknowns":    ["…"],
  "next_human_action": "…"
}
```

Every timestamp in the structured output is **UTC**. Site-local times may appear in prose for the
reader, and must be labelled with the zone.

`observed` may be empty. An empty `observed` with a non-empty `unknowns` is a valid and often correct
answer, and is much better than a populated `hypotheses` presented as fact.

## Security notes

This skill reads. It cannot export evidence — that is `heldar-evidence-export`, which produces a
signed bundle and has its own stop conditions. It cannot identify people. It cannot actuate anything.

Include the request/correlation id from any failed tool call in the output, so a support engineer can
join your timeline to the box's own logs.
