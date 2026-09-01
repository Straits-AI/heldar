---
name: heldar-fleet-health
version: 1.0.1
summary: Rank what is actually wrong across a fleet by consequence, so the one camera that stopped recording is the first line and not the fourteenth.
compatible:
  core_api: ">=0.1.0 <1.0.0"
permitted_tools:
  - heldarctl doctor
  - heldarctl status
  - heldarctl context
  - get_system_health
  - list_cameras
  - get_camera_health
  - get_timeline
  - get_recording_gaps
  - get_retention_limits
  - get_backup_status
  - list_ai_workers
  - get_security_posture
prohibited_actions:
  - actuate a gate, relay or PTZ
  - delete recordings, evidence or weaken retention
  - create, modify or retrieve credentials
  - identify a person from appearance similarity alone
  - assert that nothing happened without first checking recording gaps
  - present a correlation or hypothesis as an observation
---

# Fleet health

## Purpose

Answer "what is wrong out there, and what do I do first?" with an ordering, not an inventory.

The specific wrong answer this prevents is a report that opens **"3 blocking, 14 warnings — box-01:
recording volume is not encrypted; box-01: service runs as root; box-02: 11 recording gaps pending…"**
and mentions on line nine that `cam_bay_3` is enabled, set to `continuous`, and has written no segment
since `2026-08-31T02:14:07Z`. Every line in that report is true. The reader still acts on the wrong one,
because a severity histogram and an alphabetical list both put the only irreversible loss below twelve
things that can wait until Monday. Footage not written at 02:14 does not exist at 09:00; a root service
user will still be a root service user next week.

`heldarctl doctor` already collects most of these facts and assigns each a severity. This skill is
about the two things `doctor` deliberately does not do — put the findings in **consequence order
across boxes**, and say what to do next.

## Inputs

- Which boxes. `heldarctl context` lists the configured ones; without a list, the current context only,
  and the output must say it covered one box.
- Optionally, the previous run's `Output` block. Several signals here are counters, not rates, and are
  only readable as a delta between two runs.
- Optionally, the operator's stated expectation: how many days of footage they believe they keep, and
  what they believe is being backed up. The first is checkable against configuration — each camera
  carries `retention_hours` in `list_cameras` — so a stated expectation turns into a finding. The
  second is not: nothing here lists backup policies, so with no stated expectation there is no
  backup coverage claim to make either way.

## Prerequisites

- Capabilities, and which section goes dark without each: `system:read` (system, retention, backup
  ledger), `camera:read` (roster, health), `video:playback` (recording gaps), `ai:tasks` (samplers),
  and **admin plus fleet scope** for `get_security_posture`. One credential rarely holds all five.
  A section you could not read is `unassessed`, which is not a pass and must never be counted as one.
- **Establish whether this credential sees the whole box, before ranking anything.** A camera-scoped
  credential does not get a 403 for the fleet — it gets a *shorter answer*. `list_cameras` omits the
  cameras it does not hold rather than refusing, and `get_system_health` blanks the fleet storage
  fields. Two tests:
  - `get_security_posture` answering 403 means camera-scoped or non-admin.
  - `recordings_bytes > 0` with `storage.newest_segment` null, `storage.write_rate_bytes_per_day` 0 and
    `storage.projected_days_remaining` null is **redaction, not an idle box**. Reading those zeroes
    literally produces "no footage is being written fleet-wide", which is the most alarming wrong answer
    this skill can generate.
  Record the result as `credential_view`, and if it is `camera_scoped`, the output is a report about
  some cameras and must not be titled a fleet report.
- No tool here lists boxes, backup policies, or backup schedules. The fleet is whatever contexts are
  configured, and the backup section can only describe jobs that ran.

## Workflow

1. **Reach every box first, and record the ones you did not.** `heldarctl doctor` per context. The exit
   code is the finding: `3` unreachable, `2` auth, `6` server error, `4` contract incompatible, `5`
   blocking findings, `0` clean. A box that did not answer contributes zero rows, and a ranking
   assembled from the boxes that answered — presented as the fleet — is the flat-list failure in its
   worst form. Unreachable boxes go at the **top** of the ranking, never in a footnote.
   On exit `4`, stop for that box: `doctor` itself stops there, because every finding below it was
   parsed from shapes this client may be reading wrong.
2. **Take `doctor`'s findings rather than re-deriving them.** Each carries `code`, `severity`,
   `resource`, `detail` and `remediation`. Re-deriving camera health yourself is how a scheduled camera
   outside its window — which `doctor` deliberately does not report — becomes a fabricated emergency,
   and an operator who has been shown one of those learns to skim the section that will one day hold
   the real one.
3. **Add the checks `doctor` does not make**, using the read tools: `get_system_health` (`storage`,
   `disk_health_ok`, `cameras_recording` against `cameras_total`), `get_retention_limits`,
   `get_backup_status`, `list_ai_workers`, `get_camera_health` for `last_segment_at` and
   `reconnect_count`, and `list_cameras` for each camera's `record_mode`, `segment_seconds`,
   `retention_hours` and `storage_quota_bytes`.
4. **Sort every finding into a consequence tier.** The tier decides the order; count never does.
   1. `unreachable_or_untrusted` — the box did not answer, or its answers cannot be trusted. Nothing
      below this line is a statement about that box.
   2. `not_recording_now` — footage that should exist is not being written, this minute. Two shapes,
      both in this tier:
      - an enabled camera whose `record_mode` is `continuous` and whose `state` in `get_camera_health`
        is anything but `recording`;
      - the one a dashboard shows green for: `state: "recording"` with `last_segment_at` null, or older
        than **three times that camera's own `segment_seconds`** (from `list_cameras`; 60 s is the
        common value, so ~3 minutes).
      The recorder's `state` is its belief; `last_segment_at` is the evidence.
   3. `recording_will_stop` — `storage.disk.free_bytes` at or below `min_free_disk_bytes` from
      `get_retention_limits`; or `storage.projected_days_remaining` below 7 (read only alongside
      `cameras_recording`, see below).
   4. `footage_already_lost` — gaps with `fill_state: "failed"`; a camera whose `retention_hours` is
      shorter than the days the operator said they keep; or `recordings_bytes` at or above 95% of
      `max_recordings_bytes`, which means the sweeper is deleting the oldest unlocked segments on
      every pass and the horizon is shortening now.
   5. `evidence_not_reproducible` — backup jobs with `status: "error"`, or a ledger whose newest
      `finished_at` is older than the operator's stated backup expectation.
   6. `analysis_degraded` — a camera the operator expects AI on that is absent from `list_ai_workers`,
      or one present with `state` in `error` / `offline` / `stopped`.
   7. `posture` — `weak` findings. Real, rarely first. `unknown` is not a finding, it is `unassessed`.
5. **Within a tier, order by what is being lost and how fast**, not by row count. One camera writing
   nothing outranks twelve posture warnings, and it outranks eleven `pending` gaps that ANR is already
   refilling.
6. **Attach a next action to every ranked row.** `doctor` findings already carry `remediation`; use it
   verbatim rather than paraphrasing it into something softer. A row an operator cannot act on is the
   padding that buried the real one.
7. **State each tier's status explicitly** — `findings`, `empty`, or `unassessed`. An empty tier you
   checked and a tier you could not read are different sentences, and collapsing them is how a missing
   `ai:tasks` capability becomes "AI workers healthy".

### What these fields cannot tell you

Fold these into `cannot_tell` rather than reasoning past them:

- **`reconnect_count` is cumulative for the life of the status row**, with no window and no reset: the
  recorder only ever increments it. `847` may be three years of uptime or the last forty minutes. A
  single sample cannot show a camera flapping. Report the counter and mark it as needing a second
  sample, or take two readings at least ten minutes apart and report the delta.
- **`projected_days_remaining` is free bytes divided by the last 24 hours of written footage.** A box
  whose cameras were down for those 24 hours writes little, so it projects a comfortable number — and
  it is null, not zero, when nothing was written at all. Never read it apart from `cameras_recording`.
- **Retention has three independent limits, and the horizon is the shortest of them.** Per camera,
  `retention_hours` is an age cap the sweeper enforces and `storage_quota_bytes` (when set) a per-camera
  size cap — both readable in `list_cameras`. Box-wide, `get_retention_limits` gives the recordings size
  cap and the free-disk floor. A configured age cap is checkable against what the operator believes;
  the size limits are not a stated horizon at all, so when the fleet's write rate rises the actual
  horizon shortens and nothing alerts, because no target was breached. `max_overridden` and
  `min_free_overridden` distinguish a limit a human set from an env default.
  `storage.oldest_segment` is the horizon that resulted, not the horizon anyone asked for.
- **`disk_health_ok: true` means no SMART or RAID alert fired recently**, which is also what a box with
  disk checks disabled reports. It is not a healthy disk. `last_disk_alert_at` is the last alert at any
  time, so a null there with `disk_health_ok: true` is "never alerted", not "verified good".
- **A `completed` backup job means the copy loop finished.** `files_copied` and `bytes_copied` say what
  moved. Nothing in the ledger says the archive was opened, checksummed or restored, so never write
  "backups verified". And a policy that quietly stopped scheduling produces *no rows*, which looks
  exactly like a healthy quiet fleet — with no tool here that lists policies or their schedules, "no
  backup failures" is not a claim you can make.
- **`list_ai_workers` carries no timestamp** — `camera_id`, `stream_profile`, `state`, `fps`, `width`
  and nothing else — so despite what the tool description promises, freshness is not readable from it.
  `state` is one of `connecting`, `sampling`, `error`, `offline`, `stopped`; there is no `running`.
- **An absent camera and a stepped-down `width` are both ambiguous here.** A camera is absent from
  `list_ai_workers` when it has no enabled AI task *and* when the budget shed it, and no permitted tool
  reads the task list, so absence alone is not a fault — it is only a finding against an operator who
  says that camera should be analysed. `width` is the effective decode width after the resolution
  ladder; the requested width lives on the AI task, which is likewise not readable here, so a lone
  `width` cannot be called a step-down. Report the value, not a verdict.
- **Recording gaps are the persisted ANR rows, not computed coverage holes.** `filled` means coverage
  was restored, `pending` may yet be, `failed` (`fill_attempts` exhausted) is gone. A single gap count
  that mixes the three is meaningless. Gaps also age out of the table once `filled` or `failed`, so an
  empty list is not proof of continuous coverage.
- Pull `get_timeline` only for the cameras already on your shortlist. A per-camera timeline sweep across
  a fleet is how a triage run becomes a two-hundred-call crawl and still ranks nothing.

## Stop conditions

Stop and hand to a human when:

- **A configured box did not answer** — `doctor` exit `2` (auth), `3` (unreachable) or `6` (server
  error). Report it first and stop ranking that box. Do not publish a ranking that silently excludes it.
- **`doctor` exited `4`.** The client and the box disagree on the contract major, so every finding below
  it was read from shapes that may have changed. A confident wrong diagnosis is worse than none.
- **`credential_view` is `camera_scoped`** — `get_security_posture` returned 403, or `recordings_bytes`
  is above zero while `storage.newest_segment` is null. Report the cameras you can see and say so in the
  title. You cannot state a fleet position, and the blanked storage fields will read as zeroes if you try.
- **Any tier-2 row exists** — an enabled `continuous` camera whose `state` is not `recording`, or one
  whose `state` is `recording` with `last_segment_at` null or older than `3 × segment_seconds`. That is
  the answer. Hand it over with the camera id, the box and that timestamp now; do not keep triaging
  warnings so the report looks thorough.
- **`storage.disk.free_bytes <= min_free_disk_bytes`**, or a camera's `retention_hours` is below the
  days the operator said they keep. Changing either is an admin mutation this skill cannot make and
  must not describe as done.
- **You are asked whether footage from a particular time still exists**, or whether a specific backup can
  be restored. The first is `heldar-incident-triage`'s question against the timeline and the gap table;
  the second is not answerable from the ledger at all.
- **You are asked to confirm the fleet is healthy** and any entry in `tier_status` reads `unassessed`,
  or `boxes_reached` is below `boxes_configured`. You can report which tiers were checked and empty and
  which were not read; a fleet with no posture read and no `ai:tasks` is not a fleet you have checked.
- Answering would require a tool this skill does not permit — a backup policy listing, an AI task
  listing, a per-box inventory service, or any mutation.

## Output

```json
{
  "sampled_at": "2026-08-31T09:00:00Z",
  "scope": {"boxes_configured": 0, "boxes_reached": 0,
            "credential_view": "fleet|camera_scoped|unknown"},
  "boxes": [
    {"context": "…", "reached": true, "doctor_exit": 0, "contract": "0.1.0",
     "cameras_total": 0, "cameras_recording": 0, "error": null, "request_id": null}
  ],
  "ranked": [
    {"rank": 1,
     "tier": "unreachable_or_untrusted|not_recording_now|recording_will_stop|footage_already_lost|evidence_not_reproducible|analysis_degraded|posture",
     "box": "…", "resource": "camera id, job id or null",
     "doctor_code": "camera.not_recording|null",
     "consequence": "what is being lost, and since when",
     "evidence": {"state": "recording", "last_segment_at": "2026-08-31T02:14:07Z",
                  "segment_seconds": 60, "reconnect_count": 0},
     "next_action": "…", "needs_human": true}
  ],
  "tier_status": [{"tier": "analysis_degraded", "status": "findings|empty|unassessed",
                   "reason": "credential lacks ai:tasks"}],
  "cannot_tell": ["…"],
  "needs_second_sample": [{"signal": "reconnect_count", "box": "…", "resource": "…", "value": 0}],
  "next_human_action": "…"
}
```

**Every timestamp in this JSON is UTC, RFC 3339 with a trailing `Z`** — `sampled_at`, every
`last_segment_at`, every `oldest_segment`/`newest_segment`, every backup `started_at`/`finished_at`, and
every gap boundary. The box serves them that way; do not convert them. Site-local times may appear in
prose, labelled with the zone; a box with no zone configured has none to label them with, so leave those
in UTC and say so.

`evidence` carries field values, never adjectives. "cam_bay_3 is unhealthy" cannot be checked; `state:
"recording"` with `last_segment_at: "2026-08-31T02:14:07Z"` beside `segment_seconds: 60` can.

An empty `ranked` is only good news beside a `tier_status` in which every tier reads `empty` rather than
`unassessed`, and a `scope` where `boxes_reached` equals `boxes_configured`.

## Security notes

This skill reads. It cannot change a retention limit, retry a recording gap, start a backup or restart a
recorder — every remediation it emits is for a human or an admin credential to carry out, and the output
must never be written as though the fix has happened.

`resource` values are ids. Do not paste camera addresses, `record_url_masked`, `output_path`,
`output_url` or anything from `created_by` into a report; a backup row's absolute path and a credential
id are disclosive on their own, and the kernel already blanks the latter for a scoped reader. Include the
request id from any failed tool call so a support engineer can join this ranking to the box's own logs.
