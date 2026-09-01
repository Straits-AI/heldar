---
name: heldar-evidence-export
version: 1.1.0
summary: Plan a signed evidence bundle, then verify the file that actually landed before calling the export done.
compatible:
  core_api: ">=0.1.0 <1.0.0"
permitted_tools:
  - heldarctl context
  - get_system_health
  - list_cameras
  - get_camera_health
  - get_timeline
  - get_recording_gaps
  - get_incident
  - get_retention_limits
prohibited_actions:
  - actuate a gate, relay or PTZ
  - delete recordings, evidence or weaken retention
  - create, modify or retrieve credentials
  - identify a person from appearance similarity alone
  - assert that nothing happened without first checking recording gaps
  - present a correlation or hypothesis as an observation
---

# Evidence export

## Purpose

Get a `heldar-evidence/1` bundle out of the appliance for one camera and one UTC window, and
establish — before anyone is told it is done — that the file in hand covers the window it claims,
names its gaps, hashes to what the appliance said it wrote, and is signed by the key the recipient
expected.

The specific wrong answer this exists to prevent is this sentence:

> "Evidence exported for 2026-08-30 02:00–02:05Z, signed and verified."

said on the strength of a `200` from `POST /api/v1/evidence/exports`, when the file on disk holds 90
of those 300 seconds, or was truncated in transit, or is signed by a key nobody has pinned. A `200`
means the appliance wrote a file. It does not mean the file arrived intact, that it covers the whole
window, or that its signature matches any key the recipient trusts. That sentence gets defended in a
room where the bundle is opened for the first time.

The second wrong answer is inside the bundle's own attestation: **the signature is not a
timestamp**. It establishes that this appliance produced these bytes and that they have not changed.
The times inside come from the appliance's own clock, signed faithfully whether or not that clock is
right.

See `docs/EVIDENCE.md` for the format, the verifier's exit codes, and the three adversarial passes
that shaped it.

## Inputs

- A UTC window (`from`, `to`), and **one of** a `camera_id` or an `incident_id`.
- The expected signing key id, `sha256:<hex>`, obtained **out of band**. The appliance serves it at
  `GET /api/v1/evidence/signing-key`, but a key that arrives by the same route as the bundle
  corroborates nothing — get it from the operator, a prior pinning, or a case file.
- The path the produced bundle can be read from, if you are expected to verify it.

## Prerequisites

- Capabilities, per tool — they are not one set:
  - `video:playback` — `get_timeline`, `get_recording_gaps`, `get_incident`.
  - `camera:read` — `list_cameras`, `get_camera_health`.
  - `system:read` — `get_system_health`, `get_retention_limits`.
  - `video:export` — the produce step, which is not in this skill.
- **Every MCP tool here takes at most one path argument and sends no query string.** So
  `get_timeline` returns the camera's *whole* recorded history, not your window, and its
  `recorded_seconds` is the total for that history. Intersect its `ranges` with `[from, to]`
  yourself. Never quote `recorded_seconds` as the window's coverage.
- `get_recording_gaps` is `/api/v1/cameras/{id}/recording-gaps`: the **persisted ANR rows**
  (`gap_start`, `gap_end`, `gap_seconds`, `fill_state`, `filled_at`), newest first, capped at 500 and
  not windowed. A row with `fill_state: "filled"` is a hole that was later backfilled. These are not
  the coverage holes the export plan computes from the segment table, and the route that does compute
  those (`/api/v1/cameras/{id}/gaps`) is not exposed to MCP. Treat the two as different measurements.
- **No tool in this skill creates an export, and none verifies a bundle.** `heldar-mcp` is GET-only
  by construction (#123), so the step that writes and the step that checks are outside it:
  - producing the bundle is `POST /api/v1/evidence/exports` with `dry_run: false`;
  - verifying it is `scripts/verify_evidence_bundle.py <bundle> --key-id sha256:<hex>` (or `--key`
    with the base64 public key), which needs `python3` and `openssl` and needs no network, no
    appliance and no database.
  This skill's job is everything either side of those. If you cannot reach the bundle file or run the
  verifier, you cannot complete this skill — see Stop conditions.
- A camera-scoped credential sees only its own cameras. A camera outside scope is **absent** from
  `list_cameras`, not refused.

## Workflow

1. **Pin which box you are talking to.** `heldarctl context` prints the *local client* config — the
   context name, its `base_url`, and which is current. It is a label you chose, not an attestation
   from the appliance. `get_system_health` adds `version` and `api_version`. The only appliance
   identity that travels with the bundle is the signing key id in `signature.json`, so record the
   `base_url` beside it and let the verifier settle the rest: a bundle from the wrong box verifies
   perfectly and attests to the wrong recorder.
2. **Resolve exactly one camera.** With an `incident_id`, call `get_incident` first, with two
   caveats:
   - its segment list is **scope-filtered to your cameras**; the export's camera resolution is not —
     the kernel takes the distinct `camera_id`s of *all* the incident's segments and then scope-checks
     the one it derived. An incident that looks single-camera to you can still come back
     `400 … spans N cameras`.
   - an unknown incident and one held entirely on other cameras both answer with an **empty list**,
     not a 404.
3. **Establish coverage before you plan, not after.** Take `get_timeline`, intersect its `ranges`
   with `[from, to]`, and sum — that is your own covered-seconds figure, and the only independent
   number you will ever have. Then `get_recording_gaps`, keeping the rows that overlap the window and
   noting each `fill_state`. `get_camera_health` reports **live** recorder state
   (`recording|offline|error|idle|disabled`) as of now, not state during the window; its
   `last_segment_at` and `reconnect_count` are leads, not findings about a past window. Taken after
   the plan, none of this is a check — it is agreement.
4. **Check retention pressure with the numbers that exist.** `get_system_health` carries
   `recordings_gb` against `max_recordings_gb`, plus `storage` (`disk`, `oldest_segment`,
   `newest_segment`, `projected_days_remaining`). `get_retention_limits` reports only the *effective*
   cap and free-disk floor and which of them are operator overrides — it carries no usage at all, so
   it cannot tell you the box is near its cap. For a **camera-scoped** credential
   `storage.oldest_segment`, `newest_segment` and `projected_days_remaining` come back null and
   `recordings_gb` is your cameras' footprint measured against a fleet-wide cap, which understates
   pressure: report that you cannot assess it rather than inferring headroom.
   Whether the window's footage is *held* is `evidence_locked_segments` in the plan (step 5) — none
   of the tools here answer it.
5. **Plan the export.** `POST /api/v1/evidence/exports` — `dry_run` defaults to true, so this writes
   nothing. The plan returns `requested_seconds`, `covered_seconds`, `gaps[{from,to}]`, `segments[]`
   (each with `sha256: null` — the plan deliberately does not hash sources), `source_bytes`,
   `detection_count`, `event_count` and `evidence_locked_segments`. Compare it against step 3 within
   the tolerances below. Note that the plan does **not** 404 on a window with no footage; the produce
   step does.
6. **Hand the produce step to whoever holds `video:export`.** Give them the exact request body. Do
   not report an export that has run: you composed a command, which is a different sentence.
7. **Verify the file that landed**, not the response that described it:
   - `verify_evidence_bundle.py <file> --key-id <expected>` and read the **exit code**.
     `0 VALID`, `1 MODIFIED`, `2 MISSING`, `3 UNKNOWN-KEY`, `4 UNSUPPORTED`, `5 MALFORMED`.
   - Compare the file's own sha256 with the `sha256` in the export response, and the response's
     `manifest_sha256` with the one in `signature.json`. **`key_id` lives in `signature.json`, not in
     `manifest.json`** — the verifier recomputes it from the public key and reports `MODIFIED` if the
     two disagree.
   - Then read `media.covered_seconds`, `media.gaps` and `site.timezone` out of the **signed
     manifest**. `VALID` means unaltered, never complete: a bundle spanning an outage is valid and
     says so.
8. **Report the limits with the result, every time**, not only when asked. They are in the signed
   manifest under `attestation.limits` and the verifier prints them beneath a `VALID` line: not a
   trusted timestamp; detections are what a model reported at a stated confidence, not findings; the
   clip is concatenated across gaps and is not continuous video of that period.

## Stop conditions

Stop and hand to a human when:

- **The verifier exits non-zero.** Report the state and stop. `MODIFIED` and `MISSING` are different
  accusations — "this was altered" versus "part of it was not handed to you" — and `MALFORMED` is
  neither: it means *no conclusion was reached*, which is not a finding of integrity. Never re-run
  and report the better outcome; a verdict that changes between runs on the same bytes was, in this
  codebase's history, exactly what a forged bundle looked like.
- **You have no expected key id from out of band.** The verifier exits `3 UNKNOWN-KEY` and that is
  the honest answer. A key id fetched from the same appliance in the same session is
  self-consistency; if you use it anyway, label it as such and leave `key_obtained_out_of_band`
  false.
- **You cannot run the verifier** — no access to the file, no `python3` or `openssl`. Report the
  export as *requested and unverified*, never as complete.
- **The plan's coverage disagrees with what you measured in step 3**, by any of these tests:
  - `abs(plan.covered_seconds − your window-intersected timeline sum) > 2` seconds;
  - the plan lists a gap of 2 seconds or longer that your timeline intersection does not, or your
    intersection shows a hole of 2 seconds or longer that `plan.gaps` omits;
  - a persisted `recording_gaps` row overlaps the window with `fill_state` other than `filled`, while
    `plan.gaps` is empty.
  Differences under 2 seconds are expected and are not a finding: the plan treats seams longer than
  1000 ms as gaps, while the timeline coalesces anything under 2 s as contiguous. That difference is
  the tolerance, which is why the thresholds above are 2 seconds and not zero.
- **`plan.covered_seconds` is 0, or `plan.segments` is empty.** The produce step will answer `404 no
  footage in the range`. Report that the window holds no footage — do not report a failed export.
- **More than one `camera_id` appears in `get_incident`'s segments**, or the export answers `400 …
  spans N cameras`. List them, ask the investigator which they want, and export one bundle per
  camera.
- **The camera is absent from `list_cameras`.** It is out of scope. Do not attempt to reach it
  through an `incident_id`: the scope check runs on the derived camera precisely because that is the
  bypass shape, and trying it is an audited access attempt against footage you were not granted.
- **`plan.gaps` is non-empty and the question depends on continuity** — "show the vehicle arriving
  and leaving" across a window with a 90-second hole. Export it if asked, but every gap interval and
  the total gap seconds go in the covering statement, not a footnote.
- **The footage may not survive the approval wait**: `from` is earlier than `storage.oldest_segment`,
  or `storage.projected_days_remaining` is fewer than the days until the export can be approved, or
  `plan.evidence_locked_segments` is 0 while `recordings_gb` is within 10% of `max_recordings_gb`.
  Say so with the numbers rather than scheduling the export for later. On a camera-scoped credential
  those storage fields are null — report that the assessment could not be made.
- **The requested change would shape the evidence**: trimming the window to the covered part so the
  clip looks continuous, re-exporting to obtain a bundle without gaps, or exporting a window narrower
  than the incident. Refuse and say why.
- **The question is when.** "Prove this happened at 02:14" is not answerable from a signature. Report
  what the manifest asserts and who stamped it.

## Output

```json
{
  "window": {
    "from": "2026-08-30T02:00:00Z",
    "to": "2026-08-30T02:05:00Z",
    "site_timezone": "IANA name or null — copied from the VERIFIED manifest's site.timezone; null until a bundle is verified, and display only"
  },
  "target": {
    "camera_id": "…",
    "incident_id": "… or null",
    "heldarctl_context": "local context name",
    "base_url": "https://…",
    "api_version": "from get_system_health"
  },
  "coverage_observed": {
    "window_recorded_seconds": 0,
    "requested_seconds": 0,
    "timeline_ranges_in_window": [{"start": "…Z", "end": "…Z", "seconds": 0}],
    "persisted_gap_rows_in_window": [
      {"gap_start": "…Z", "gap_end": "…Z", "gap_seconds": 0, "fill_state": "pending|filled|failed"}
    ],
    "camera_health_now": "recording|offline|error|idle|disabled|unknown",
    "source": "get_timeline intersected with the window + get_recording_gaps, both read before the plan"
  },
  "plan": {
    "covered_seconds": 0, "requested_seconds": 0,
    "gaps": [{"from": "…Z", "to": "…Z"}],
    "segment_count": 0, "source_bytes": 0,
    "detection_count": 0, "event_count": 0, "evidence_locked_segments": 0,
    "agrees_with_coverage_observed": true,
    "disagreement": "null, or which test in Stop conditions failed and by how much"
  },
  "bundle": {
    "id": "…", "filename": "…", "size_bytes": 0,
    "sha256": "…", "manifest_sha256": "…",
    "key_id": "sha256:… (from signature.json)",
    "exported_by": "manifest export.principal_id",
    "audit_id": "manifest export.audit_id",
    "request_id": "manifest export.request_id"
  },
  "verification": {
    "performed": true,
    "verdict": "VALID|MODIFIED|MISSING|UNKNOWN-KEY|UNSUPPORTED|MALFORMED|not_performed",
    "exit_code": 0,
    "expected_key_id": "… or null",
    "key_obtained_out_of_band": true,
    "manifest_covered_seconds": 0,
    "manifest_gaps": [{"from": "…Z", "to": "…Z"}]
  },
  "complete": false,
  "limitations": [
    "Not a trusted timestamp: the appliance stamps its own clock and signs it faithfully, right or wrong.",
    "Detections are what a model reported at the stated confidence, not findings of fact.",
    "The clip is concatenated across gaps; it is not continuous video of the window."
  ],
  "unknowns": ["…"],
  "next_human_action": "…"
}
```

Every timestamp in this object is **UTC**, RFC3339 with a trailing `Z` — including the ones copied
out of the signed manifest, which is UTC throughout. `site_timezone` is the only exception and is not
a timestamp: it is the IANA zone from the manifest's `site.timezone`, present so a reader can render
a local wall clock, and `null` means no zone is configured — say that rather than letting UTC be read
as the operator's clock.

`complete` is `true` only when `verdict` is `VALID` **and** `key_obtained_out_of_band` is `true`.
Every other combination, including a bundle that verified against a key you took from the same box in
the same breath, is an unverified export and must be reported as one.

## Security notes

This skill reads and plans. It does not produce the bundle, does not lock or unlock segments, and
does not touch retention. Holding evidence against retention is
`POST /api/v1/segments/{id}/evidence-lock`, which requires a registry-manager principal — a different
caller, deliberately.

The signature's ceiling is the box: on an appliance where an attacker has root, they can sign
bundles. That raises the bar from "anyone with a hex editor" to "root on the recorder", which is the
honest description and the one to give a recipient who asks what it proves.

Carry `export.request_id` and `export.audit_id` from the signed manifest into the output. They are
how a support engineer joins your report to the box's own logs, and the audit row is written *before*
the export so the bundle can name it.
