---
name: heldar-camera-commissioning
version: 1.0.0
summary: Verify a newly installed camera against the field checklist — reachability, stream role, recording continuity, retention maths and the night case — and say which parts cannot be verified from a terminal at all.
compatible:
  core_api: ">=0.1.0 <1.0.0"
permitted_tools:
  - get_system_health
  - list_cameras
  - get_camera_health
  - get_timeline
  - get_recording_gaps
  - get_retention_limits
  - get_security_posture
  - heldarctl doctor
  - heldarctl context
prohibited_actions:
  - actuate a gate, relay or PTZ
  - delete recordings, evidence or weaken retention
  - create, modify or retrieve credentials
  - identify a person from appearance similarity alone
  - assert that nothing happened without first checking recording gaps
  - present a correlation or hypothesis as an observation
---

# Camera commissioning

## Purpose

Check a newly installed camera against [`docs/commissioning-checklist.md`](../../docs/commissioning-checklist.md)
and produce the evidence a human needs in order to sign it off — or the specific reason they must not.

The wrong answer this exists to prevent is the sentence **"`gate-north-anpr` is commissioned — it is
in the registry and health says `recording`"**. `state: "recording"` means a recorder process is up
and an ffmpeg pipeline wrote a segment. It does not mean the right stream is being recorded, that it
recorded *continuously*, that the footage will still exist tomorrow, or that anything is legible
after dark. Those are four separate questions; three are answerable from here, and the fourth — image
quality — is not answerable from here at all.

Sign-off is a human act. This skill never returns "commissioned".

## Inputs

- One or more camera ids, or a `site_id`. `list_cameras` takes no query parameters — it returns the
  whole roster this credential can see, so a name or site is matched locally against that list rather
  than searched for. A `name` is not the `id` and is not unique; resolve to the `id` and quote it.
- Each camera's **declared purpose**: `anpr`, `face`, `reid`, or `overview`. The registry does not
  store purpose, and the acceptance thresholds are purpose-dependent. If nobody supplies it, say so
  and do not guess it from the camera's name.
- The site's timezone, and local sunset/sunrise, if the night case is in scope.
- Whether a human has already taken the Phase 7 snapshot and the Phase 8 night snapshot, and when.
  This is an input to the verdict, not something you can obtain.

## Prerequisites

- A credential with `camera:read` (registry and health), `video:playback` (timeline and gaps) and
  `system:read` (box and retention). `get_security_posture` is admin-only and fleet-wide: a
  camera-scoped credential gets a 403 there, which is a fact about your credential, not about the
  camera.
- **Time on the box.** Continuity cannot be assessed on a camera that started ten minutes ago.
  Require at least three segment lengths (~3 min at the 60 s default) before reading health at all,
  and at least one full dusk→dawn cycle before saying anything about the night. A camera enabled this
  afternoon is `not_yet_verifiable`, and saying so is the correct answer.
- **Tools that do not exist, and are not coming via this skill.** There is no probe/test tool, no
  snapshot, no live view, no clip export, no per-segment listing, no schedule reader, no discovery,
  and no way to create or edit a camera. Every act in checklist Phases 0–6 is a field action
  performed by a person. This skill verifies the *outcome* of those acts and nothing else.
- **`fps_observed` and `bitrate_kbps` are the last indexed segment's numbers**, overwritten as each
  segment lands (bytes ÷ duration, and that segment's probed fps). They are a sample, not an average
  and not a history: no permitted tool returns last night's bitrate, so a night-against-day
  comparison needs two runs, one taken while it is actually dark. The same write sets `last_error` to
  NULL, so a camera that failed an hour ago and has since written one segment shows no error at all —
  `reconnect_count` is the surviving evidence.
- The MCP tools take **no time window**. `get_timeline` returns the whole retained range for the
  camera, which is bounded by retention — you cannot ask about a period longer than the box still
  keeps, and the absence of footage from before that period is retention working, not a gap.
- Confirm which box you are pointed at with `heldarctl context` before anything else. Commissioning
  findings filed against the wrong site are worse than no findings.

## Workflow

1. **Box first, camera second.** `heldarctl context`, then `get_system_health` for `api_version`,
   `recorder_enabled`, `cameras_total` and the `storage` block. If `recorder_enabled` is false, no
   camera on this box is commissionable and the rest of the run is noise.

2. **Read the registry, and read it against itself.** `list_cameras`. The fields that decide a
   sign-off, and the contradictions to look for:
   - `record_stream` against `resolution_main` / `resolution_sub`. **`record_stream: "sub"` on an
     ANPR, face or ReID camera is a failure**, however green the tile: what is being kept as evidence
     is the low-resolution preview rather than the main stream, and no amount of continuity fixes it.
     The checklist permits `sub` only for low-value overview cameras.
   - `record_mode`. `continuous` is the only mode where a night gap is unambiguously a fault. See
     the stop conditions for the others.
   - `enabled` and `record_enabled`. A disabled camera that has a status row reports
     `state: "disabled"` in health — the kernel overrides whatever the recorder teardown left
     behind — and is skipped entirely by `heldarctl doctor`. One that never recorded has no row and
     is simply absent, the same as a camera whose recorder never started.
   - `codec`, `fps_main`, `segment_seconds`, `retention_hours`, `storage_quota_bytes`,
     `anr_enabled`, `has_password`. Record them; steps 4 and 5 are arithmetic on them.

3. **Health, including the row that is missing.** `get_camera_health` returns rows from
   `camera_status`. **A camera whose recorder has never run has no row at all** — it is absent from
   the list, not `offline`. Diff the enabled cameras from step 2 against the health rows first; an
   absent camera is the worst state, and it reads as a clean list. Then, per camera:
   - `state` — the recorder writes `recording`, `connecting`, `error` or `offline`, and the kernel
     overrides it to `disabled` for a disabled camera. `last_error`, and how stale `last_segment_at`
     is against now. A null `last_error` is not evidence of a clean hour: see the prerequisites.
   - `fps_observed` against `fps_main`, and `bitrate_kbps` against what the encoder was set to.
     A stream at half the configured fps is a configuration fault that a timeline will not show.
   - `reconnect_count`. Cumulative, so one reading gives a total and not a rate; two readings hours
     apart give the rate, and the rate is what matters.

4. **Continuity, not "it recorded once".** `get_timeline` per camera:
   - More than one entry in `ranges` over the retained window means dropouts. Investigate before
     sign-off; do not average them away.
   - `recorded_seconds` against the wall-clock span of the window. The shortfall is the missing
     footage, in seconds, and it belongs in the output as a number.
   - `segment_count` against `span / segment_seconds`. **A count materially above that means short
     segments, which means the recorder kept restarting** — and this is the case a continuous-looking
     timeline hides: the coalescer treats a hole of **2 seconds or less** between segments as
     contiguous, so a camera flapping every thirty seconds with 1.5 s losses reports one unbroken
     range. `segment_count` and `reconnect_count` are the only evidence of it.
   - Then `get_recording_gaps`. Note what this is: the **persisted ANR gap table with fill state**
     (`pending` / `filled` / `failed`), not the coverage holes computed from segments. There is no
     permitted tool for the computed holes, so the timeline in this step is your coverage evidence
     and the gap table only tells you whether the box noticed and whether ANR back-filled. A gap with
     `fill_state: "pending"` may still be filled; on a camera with `anr_enabled: false` every hole is
     permanent. The two thresholds do not line up, and the mismatch is a blind band: the indexer
     writes a gap row only for a hole of **more than 3 seconds**, so a 2–3 s hole splits `ranges`
     while appearing nowhere in this table, and a hole of 2 s or less appears in neither.

5. **Retention maths — what will actually still be here.** `get_retention_limits` gives the box-wide
   `max_recordings_bytes` and the free-disk floor. `retention_hours` on the camera is an age policy,
   and the size cap can evict footage far younger than it. The sweeper splits the cap into
   per-camera shares:

   ```
   share_bytes     ≈ max_recordings_bytes × weight / Σ weights
                     # weight = storage_quota_bytes when an operator set one, else 1
                     # Σ runs over the N cameras that currently HOLD footage, so with no
                     # quotas anywhere this is an equal split: max_recordings_bytes / N
   bytes_per_hour  ≈ bitrate_kbps × 450_000           # 1 kbps = 125 B/s × 3600 s
   effective_hours ≈ min(retention_hours, share_bytes / bytes_per_hour)
   ```

   Report `effective_hours`, label it an estimate, and give its basis. Three things make it optimistic
   and all three belong in the output:
   - `bitrate_kbps` is one segment's figure, whichever segment landed last. VBR climbs at night under
     IR, and again in rain.
   - **N includes the camera you are commissioning.** Adding a seventeenth camera to a sixteen-camera
     box shortens the other sixteen. If the fleet was already near the cap, this camera's sign-off
     costs its neighbours retention, and that is a finding about the site, not about this camera.
   - Evidence-locked footage counts against that camera's own share, so a camera holding locked
     footage has less room for new recording than the arithmetic suggests.

   `storage.projected_days_remaining` from `get_system_health` is **free disk ÷ recent write rate**.
   It is not retention: the sweeper deletes long before the disk fills, so the disk never reaches the
   projected day. Never quote it as a retention figure. `write_rate_bytes_per_day` covers the last
   24 hours, so a camera enabled an hour ago has contributed a twenty-fourth of its steady-state cost
   and every projection currently flatters the install.

6. **The night case — say what you measured and what you did not.** No permitted tool returns a
   frame. Pixels-on-target, plate legibility under IR, retroreflective blowout, focus, dome flare,
   glare and the day↔night switch are **not assessable from here at any confidence**, and bitrate is
   not a proxy for image quality. What you can measure over a retained window that contains a full
   dark period:
   - continuity across the dusk and dawn boundaries specifically. IR-cut switching renegotiates the
     stream, and the resulting churn clusters at those two times;
   - `reconnect_count` sampled before dusk and after dawn;
   - `bitrate_kbps` and `fps_observed` **sampled during the dark period** against the daytime
     figures — a night bitrate at the encoder ceiling means IR noise is eating the budget the plate
     needed. Both fields hold only the last indexed segment's numbers, so this is two runs at two
     times of day, not one run that reads history. A single daytime run cannot fill the night ones.

   Everything else about the night is the human snapshot, and it is an input (see Inputs), not a
   conclusion.

7. **Posture and handover.** `get_security_posture` returns findings by stable `id` — branch on the
   id, not on `detail`: `secret_key_source`, `process_visibility`, `service_user`,
   `volume_encryption`, `rtsp_transport`, `plaintext_credentials`. Then `heldarctl doctor` for the
   box's own view. Two limits, both of which go in the output rather than being quietly dropped:
   - a posture finding of `unknown` is **not a pass**;
   - **no permitted tool reports `HELDAR_AUTH_ENABLED` or the API bind address.** Exactly one
     inference is available and it is one-directional: `get_system_health` returns
     `enforcement.deployment_mode`, and a box whose mode starts with `production` **refuses to boot**
     with auth off on a non-loopback bind. So a box that is answering you while reporting
     `deployment_mode: "production…"` has auth on or is bound to loopback — record
     `enforced_by_deployment_mode`. Any other value, empty included, proves nothing in either
     direction: record `unverified` and name the human check (the boxed startup warning, per the
     checklist). Never read the absence of a warning as a pass.

   `heldarctl doctor` is a cross-check, not coverage evidence. It is silent for `scheduled` and
   `event` cameras that are not recording, it skips disabled cameras entirely, and when the
   credential cannot read the posture it drops those findings **without saying so** — a clean
   `doctor` is consistent with a camera recording nothing and a posture nobody read. A camera with no
   status row it reports as `camera.state_unknown` — a warning, not a blocker, and the exit code
   stays 0.

## Stop conditions

Stop and hand to a human when:

- **No range spans a full dark period** and the sign-off touches night performance. Test it: no entry
  in `ranges` has `start` at or before the most recent local sunset *and* `end` at or after the
  following sunrise. A daytime timeline says nothing about 3am, and this is the default state of a
  camera enabled this afternoon.
- **The night bitrate/fps cannot be sampled.** The output asks for `night_bitrate_kbps`, and both
  fields hold only the last indexed segment's numbers. Test it: `last_segment_at` is not inside the
  dark period. Fill the day figures, leave the night ones null, and say a second run between sunset
  and sunrise is required — do not derive a night figure from a daytime reading.
- **`record_mode` is `scheduled`, `scheduled_event` or `event`.** No permitted tool reads the
  schedule or the trigger windows, so a hole in the timeline cannot be classified as policy or as
  fault. Report the holes with their times and stop; a human compares them to the schedule.
- **`record_stream` is `sub` and the declared purpose is ANPR, face or ReID.** That is a
  configuration failure, and the fix is a field change on the camera and in the registry — not
  another read.
- **The oldest footage sits at the retention edge.** Test it: `now − earliest ranges[*].start` is
  within 5% of `retention_hours`, or within 5% of the `effective_hours` estimate from step 5. At the
  edge, an eviction and a dropout are the same shape in `ranges` and no permitted tool separates
  them. Name both, and say which you cannot rule out.
- **Any question that needs a picture**: is the plate big enough, is the face legible, is the lens
  focused, is IR blowing out the plate, is anything occluded. There is no snapshot tool here. Never
  infer image quality from bitrate, fps or segment size.
- **The camera is absent from `get_camera_health` while enabled** — its recorder has never run.
  That is a field fault, not a reporting delay, once the three-segment wait has passed.
- **`last_error` names an authentication failure** — it carries `401`, `Unauthorized`,
  `authentication failed`, or an RTSP handshake rejection. Report it once and stop. Do not suggest retrying,
  do not sequence further attempts, and never propose a credential sweep: the HikVision test units at
  `192.168.0.2`–`192.168.0.12` lock the account and can lock the IP after a few failures, which
  bricks a shared unit for everyone. The correct next step is confirming the credential out-of-band.
- **`get_security_posture` returns 403, or any finding is `unknown`.** Neither is a pass.
- **The box clock is suspect.** Any one of: `|now − (started_at + uptime_seconds)| > 60 s`; two
  entries in `ranges` overlap; any range `end` is later than now. Each makes every timestamp in the
  run unreliable, the retention maths included, and none of them is fixable from here.
- **`heldarctl context` does not show the box you were asked about.** It prints one starred (`*`)
  row — context name, base URL, token source — and no site at all. If that name and URL are not the
  box in the request, stop before reading anything else; and once `list_cameras` returns, stop if any
  camera in scope carries a `site_id` other than the site you were given.

## Output

```json
{
  "box": {
    "context": "…", "api_version": "…", "recorder_enabled": true,
    "cameras_total": 0,
    "retention": {"max_recordings_gb": 0, "max_overridden": false,
                  "min_free_disk_gb": 0, "min_free_overridden": false},
    "deployment_mode": "…",
    "auth_posture": "enforced_by_deployment_mode|unverified",
    "posture_findings": [{"id": "…", "status": "ok|weak|unknown"}]
  },
  "cameras": [{
    "id": "…",
    "declared_purpose": "anpr|face|reid|overview|not_supplied",
    "registry": {"record_stream": "main|sub", "record_mode": "…", "codec": "…",
                 "fps_main": 0, "segment_seconds": 60, "retention_hours": 24,
                 "anr_enabled": true, "enabled": true},
    "live": {"state": "recording|connecting|error|offline|disabled|absent",
             "last_segment_at": "UTC", "fps_observed": 0, "bitrate_kbps": 0,
             "reconnect_count": 0, "last_error": null},
    "continuity": {"window": {"from": "UTC", "to": "UTC"},
                   "range_count": 1, "recorded_seconds": 0, "window_seconds": 0,
                   "shortfall_seconds": 0, "segment_count": 0, "expected_segments": 0,
                   "anr_gaps": [{"from": "UTC", "to": "UTC", "fill_state": "pending|filled|failed"}],
                   "losses_2s_or_less": "invisible in ranges and in the gap table — see segment_count"},
    "retention_estimate": {"effective_hours": 0, "basis": "share_bytes/bytes_per_hour at the observed bitrate",
                           "share_divisor_n": 0, "caveats": ["…"]},
    "night": {"dark_period_covered": true, "dusk_dawn_dropouts": 0,
              "night_bitrate_kbps": 0, "day_bitrate_kbps": 0,
              "image_quality": "not assessable — no snapshot tool",
              "human_night_snapshot": {"done": false, "by": null, "at": null}},
    "unverifiable": ["…"],
    "blocking": [{"finding": "…", "evidence": "…"}],
    "verdict": "blocked|outstanding|ready_for_sign_off"
  }],
  "next_human_action": "…"
}
```

Every timestamp in this structure — `last_segment_at`, both ends of `continuity.window`, every
`anr_gaps` bound, `human_night_snapshot.at` — is **UTC**, as the API returns it. Site-local times may
appear in prose for the reader and must carry the zone; if the site has no zone configured, say the
times were read as UTC rather than assuming the reader's local clock.

`live.state` carries the four values the recorder writes (`recording`, `connecting`, `error`,
`offline`) plus `disabled`, which the kernel substitutes for a disabled camera. `absent` is this
skill's own label for a camera that is enabled and has **no** `camera_status` row — the API returns
no row rather than a state, and that absence is the finding.

`verdict` has three values and "commissioned" is not one of them:

- `blocked` — a finding that a field change must fix (wrong stream role, dropouts, a recorder that
  never started, retention shorter than the site requires).
- `outstanding` — nothing is wrong yet, but something required is not yet knowable: not enough
  elapsed time, no dark period on the timeline, no human snapshot, a schedule you cannot read.
  `outstanding` is the honest default for a camera commissioned today.
- `ready_for_sign_off` — every check available here passes **and** the human day and night snapshots
  are recorded in the input. It means a person may now sign. It is not itself a sign-off.

An empty `blocking` list next to a populated `unverifiable` list is a normal and useful result. It is
much better than a `ready_for_sign_off` earned by not looking.

## Security notes

This skill reads. It cannot register, edit, enable or disable a camera, cannot probe an address,
cannot fetch a frame or a clip, and cannot read or set a credential — the sidecar is read-only by
construction, so none of that is a matter of the agent behaving.

Camera addresses are deliberately visible in `list_cameras` output — that decision is documented, not
an oversight. Passwords never leave the kernel (`has_password` is a boolean), the username is
`<redacted>` by the sidecar, and `record_url_masked` arrives with its credentials masked. None of the
three may be reconstructed, guessed or requested, and a masked or `<redacted>` field is the design
rather than an error to work around.

Include the request/correlation id from any failed tool call in the output, so a support engineer can
join your commissioning report to the box's own logs.
