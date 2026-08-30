---
name: heldar-retention-planning
version: 1.0.1
summary: Size storage against a retention target, and say plainly when the target is arithmetically impossible under the cap, the floor and the evidence locks.
compatible:
  core_api: ">=0.1.0 <1.0.0"
permitted_tools:
  - get_system_health
  - get_retention_limits
  - list_cameras
  - get_camera_health
  - get_timeline
  - get_recording_gaps
  - get_backup_status
  - heldarctl status
  - heldarctl doctor
prohibited_actions:
  - actuate a gate, relay or PTZ
  - delete recordings, evidence or weaken retention
  - create, modify or retrieve credentials
  - identify a person from appearance similarity alone
  - assert that nothing happened without first checking recording gaps
  - present a correlation or hypothesis as an observation
---

# Retention planning

## Purpose

Answer "will this box hold 30 days of these cameras?" with a number that survives contact with the
retention sweeper.

The specific wrong answer this exists to prevent is **"yes, 30 days" on a box that holds 9** — and
then footage from day 12 is gone when someone asks for it in a hearing. It is produced by an
arithmetic that looks careful: nameplate bitrate x camera count x 30 days, compared against the
disk's total size, verdict *fits*. Five things make it wrong, and each one only shortens the answer:

- the **cap** (`max_recordings_bytes`) is usually well below the disk, and it is what the sweeper
  enforces;
- the **free-disk floor** (`min_free_disk_bytes`) reserves space that the disk's `free_bytes`
  otherwise looks like it is offering you;
- **evidence-locked footage is never evicted**, so it is subtracted from the budget permanently, not
  cycled through it;
- a camera with `storage_quota_bytes` set has its **own, smaller ceiling**, so the fleet answer can
  be 30 days while that one camera holds 6;
- retention here is a **size** policy, not a promise of days. The box keeps the last N bytes. Days
  are what falls out of that, and they shrink whenever the write rate rises.

The second wrong answer, cheaper to produce and just as bad: quoting
`storage.projected_days_remaining`. That field is `disk.free_bytes / write_rate_bytes_per_day`. It
knows nothing about the cap, the floor or the locked bytes, and it is almost always larger than the
truth.

## Inputs

- A retention target in days, and whether it is an aspiration or a **legal/contractual minimum**.
  The two have different stop conditions.
- The cameras in scope, or the whole fleet.
- Optionally a proposed cap in GB. Absent one, compute the cap the target would require.
- Any bitrate figure the user supplies, labelled as measured or nameplate. They are not the same
  number and must not be mixed silently.

## Prerequisites

- A credential with `system:read` and `camera:read`, **unscoped**. A camera-scoped credential is
  disqualifying: `GET /api/v1/system` deliberately sets `storage.write_rate_bytes_per_day` to 0 and
  nulls `projected_days_remaining`, `oldest_segment` and `newest_segment` for a scoped caller,
  because the fleet's retention horizon discloses cameras outside the scope. A suppressed zero reads
  exactly like an idle box.
- **The dry-run plan is not callable from this skill, and no permitted tool can issue it.** The plan
  is `PUT /api/v1/system/retention` with `dry_run: true` (#121), and the MCP surface is GET-only by
  construction. Compose the request, hand it to a human holding an admin, fleet-scoped credential,
  and read their result. Never report a plan you did not receive.
- The fleet-wide locked total is likewise only reported by that plan, as
  `effect.evidence_locked_bytes`. No GET returns it — the closest is a per-camera COUNT of locked
  segments, which is not bytes. Until you have the plan, the evidence-lock headroom in your
  arithmetic is an assumption and must be labelled as one.

## Workflow

1. **Read the limits, not the disk.** `get_retention_limits` gives the effective cap and floor plus
   `max_overridden` / `min_free_overridden`, which distinguish "an operator configured 500 GB" from
   "nobody set anything and this is the env default". Then `get_system_health` for `storage.disk`,
   `storage.recordings_bytes`, `storage.segment_count` and `storage.write_rate_bytes_per_day`.
   Ignore `storage.projected_days_remaining` for this question — see Purpose.
   *Units trap:* the API's `max_recordings_gb` and `min_free_disk_gb` are computed with 1024^3 bytes
   — GiB wearing a GB label — while a disk sold as 8 TB holds 7.28 TiB. Do every step in bytes and
   convert once, at the end.

2. **Establish the ceiling.** It is the smaller of:
   - the cap: `max_recordings_bytes`;
   - the disk: `recordings_bytes + disk.free_bytes - min_free_disk_bytes`.

   Record which one binds. They measure different things: the cap sums `size_bytes` over indexed
   `segments` rows, whereas the floor is a statvfs reading of the **filesystem holding
   `recordings_dir`**. The metadata DB, snapshots and exported clips therefore spend against the
   floor while being invisible to the cap, whenever they sit on that same filesystem.
   Mirror copies (`mirror_enabled`, written to `HELDAR_MIRROR_RECORDINGS_DIR`) carry no segment row
   at all: the cap will never restrain them, and the floor only sees them if the mirror shares the
   recordings filesystem — which it usually does not, that being the point of a mirror. They are
   pruned on their own, by file mtime against the camera's `retention_hours`. Count them as a second
   full copy of that camera's footage on whatever volume they land on.
   Then check per-camera ceilings: `list_cameras` gives `storage_quota_bytes`. A non-null quota is
   enforced separately from the cap, so that camera's days come from its quota, not from the fleet
   budget, and it is the smaller number that gets quoted back at you later.

3. **Subtract the evidence-lock headroom.** The sweeper excludes `evidence_locked = 1` from every
   prune. Evictable budget = ceiling - `evidence_locked_bytes`. If that is zero or negative the cap
   cannot converge: the sweeper deletes nothing else and (when locked bytes strictly exceed the cap)
   logs a `disk_pressure` warning carrying `reason: locked_exceeds_cap`, so the footprint stays over
   cap indefinitely. Do not describe such a cap as "tight" or "slow to settle". The sentence is
   **this cap will never be met**.
   Locked bytes only ever grow. Carry an explicit growth allowance for the planning horizon, state
   the number you assumed and what you based it on, and mark it an assumption until the dry run
   returns the real figure.

4. **Get a write rate you can defend.** `write_rate_bytes_per_day` is the sum of `size_bytes` for
   segments whose **`end_time`** falls in the last 24 hours. Two consequences:
   - it under-reports if any camera was down, disabled or gapped during that window;
   - it is keyed on footage time, not index time, so footage backfilled from before the window (an
     ANR fill, a post-restart re-index) does not inflate it — and equally does not appear in it.

   Before using it, run `get_camera_health` (`state`, `last_segment_at`, `bitrate_kbps`) and
   `get_recording_gaps` for each camera in scope, and `get_timeline` where you need to see what a
   camera actually recorded in that window. If any camera was offline, in `error`, disabled or inside
   a gap, the sample is not the steady state — reconstruct per camera instead.
   `bitrate_kbps` is *observed*, not nameplate: the indexer computes it as
   `size_bytes x 8 / duration_s / 1000` for the most recently indexed segment and overwrites the row
   each time. It is one segment's worth of evidence, which is better than a datasheet and much worse
   than a week.
   Per camera, from bitrate: `bytes/day = kbps x 1000 / 8 x 86400`, i.e. about 10.8 MB/day per kbps,
   so 1 Mbps is roughly 10.06 GiB/day and about 302 GiB over 30 days.
   The recorder writes **one** stream per camera — `record_stream` selects main or sub — so size it
   from that stream's resolution/fps, not from both. Audio is muxed into the same file when
   `record_audio` is set, so an observed `bitrate_kbps` already includes it and a nameplate video
   figure does not.
   Multiply by 24 hours only for `record_mode: continuous`. For `event`, `scheduled` and
   `scheduled_event`, bitrate x 24h is an upper bound, commonly three to ten times the truth, and
   presenting a bound as an estimate is how a plan silently buys the wrong disk.

5. **Apply a failure reserve.** Not optional, at least 20% of the evictable budget, and name what it
   covers: a camera that reconnects at a higher bitrate than the one you measured, VBR rising with
   the scene (rain, headlights, a busy night), an incident batch locking segments, and a re-encode
   after someone changes resolution or codec. A plan sized to exactly 100% of the budget converts an
   ordinary bad-weather week into deleted footage.
   `achievable_days = (ceiling - evidence_locked_bytes - reserve) / bytes_per_day`.

6. **State the arithmetic plainly.** "At the measured rate this box holds N days; you asked for M."
   If N < M, do not soften it, and do not offer to shorten a camera's `retention_hours` as though
   that were a fix — per-camera age policy changes *which* footage is lost first, never how much
   fits. Only three levers move N: fewer bytes per day (bitrate, fps, resolution, choosing the sub
   stream, event-mode recording), more usable bytes (a larger disk, a higher cap where the disk
   allows one, a lower floor), or footage leaving the box. For the last, check `get_backup_status`:
   if a backup target already holds copies, the retention question may be answerable off-box, which
   is a different question with a different owner.

7. **Hand over the dry run; do not issue it.** Emit the exact request:
   `PUT /api/v1/system/retention {"max_recordings_gb": X, "dry_run": true}`.
   Have the operator reconcile the returned `effect.would_evict_bytes` and
   `effect.evidence_locked_bytes` against your figures *before* committing with the returned
   `plan_hash`. Two things the plan does not say, which you must:
   - `would_evict_bytes` is `recorded_bytes - new_cap_bytes`, with no allowance for locked segments.
     Where locks exist the sweeper will delete **less** than that figure, and stop short of the cap.
   - the plan only models a cap change. A request carrying just `min_free_disk_gb` returns
     `would_evict_bytes: 0` and "Committing this evicts nothing now", while raising the floor is
     exactly what makes the sweeper's free-floor pass prune footage on its next run.

   If the commit is refused with 409 the state moved between planning and committing: that refusal
   is the system working. Re-plan. Never suggest retrying with `plan_hash` omitted, which is a legal
   request and precisely the wrong one.

## Stop conditions

Stop and hand to a human when any of these is true — each is a check you can run against the data,
not a judgement about whether you feel confident:

- `max_recordings_bytes - evidence_locked_bytes <= 0`, or the cap being proposed would make it so.
  The cap can never be met, and the remedies — raising the cap, or releasing evidence holds — are
  neither yours to perform nor yours to recommend unprompted.
- `min_free_disk_bytes >= disk.total_bytes`. The floor cannot be satisfied, the sweeper refuses to
  prune for it (`reason: floor_unsatisfiable`), and every days-remaining number is meaningless until
  the floor is corrected.
- `storage.write_rate_bytes_per_day == 0` while `cameras_recording > 0`. Your credential is
  camera-scoped and the field is suppressed, not measured. Every projection built on it is fiction.
  Ask for an unscoped `system:read` credential and stop.
- For any camera in scope: `get_camera_health` reports `state` other than `recording`, or
  `last_segment_at` is more than one hour old, or `get_recording_gaps` returns an interval
  overlapping the last 24 hours. You may report the sample. You may not project from it.
- `storage.disk` is null (statvfs failed). You then have a cap and no ceiling, and those two
  disagree in exactly the case that matters.
- The target is a **legal or contractual minimum** and `achievable_days.low < requested_days`. That
  is a procurement decision. Do not propose dropping cameras, cutting bitrate or shortening
  `retention_hours` to make the number fit a duty someone signed.
- A camera in scope has `record_mode` other than `continuous` and you cannot compute its duty cycle
  as recorded seconds / elapsed seconds over at least 7 days of `get_timeline`. Report the range
  with both ends — bitrate x 24h as the high, your best evidenced duty cycle as the low — never a
  point estimate.
- The dry run comes back with `effect.would_evict_bytes > 0`. Deleting footage on the strength of
  arithmetic you produced is outside this skill and outside every read-only credential; the dry run
  exists to make this exact handover.
- `evidence_lock.source` is not `dry_run` **and** the assumed locked bytes exceed the margin they
  have to survive: `assumed_locked_bytes > (achievable_days.point - requested_days) x bytes_per_day`.
  The one number you guessed is then deciding the verdict. Ask for the plan instead.

## Output

```json
{
  "generated_at": "RFC3339 UTC",
  "sample_window": {"from": "RFC3339 UTC", "to": "RFC3339 UTC"},
  "credential_scope": "fleet|camera_scoped|unknown",
  "limits": {"max_recordings_bytes": 0, "max_overridden": false,
             "min_free_disk_bytes": 0, "min_free_overridden": false},
  "disk": {"total_bytes": 0, "free_bytes": 0, "read_ok": true},
  "ceiling": {"bytes": 0, "binding_constraint": "cap|disk_floor", "note": "…"},
  "evidence_lock": {"locked_bytes": null, "source": "dry_run|assumed|unknown",
                    "growth_allowance_bytes": 0, "basis": "…"},
  "write_rate": {"bytes_per_day": 0, "basis": "measured_24h|computed_from_bitrate|mixed",
                 "sample_representative": false,
                 "cameras_not_recording_during_sample": ["…"]},
  "cameras": [{"id": "…", "record_mode": "continuous|scheduled|event|scheduled_event",
               "record_stream": "main|sub", "bitrate_kbps": null,
               "bitrate_basis": "observed_last_segment|nameplate|user_supplied",
               "bytes_per_day": 0, "duty_cycle": null,
               "storage_quota_bytes": null, "quota_limited_days": null,
               "mirror_enabled": false}],
  "reserve": {"fraction": 0.2, "bytes": 0, "covers": ["…"]},
  "achievable_days": {"point": 0.0, "low": 0.0, "high": 0.0},
  "requested_days": 0,
  "target_type": "aspiration|legal_minimum",
  "verdict": "meets|does_not_meet|cap_can_never_converge|cannot_be_determined",
  "what_would_change_it": ["…"],
  "dry_run_request": {"method": "PUT", "path": "/api/v1/system/retention",
                      "body": {"max_recordings_gb": 0, "dry_run": true},
                      "issued_by_this_skill": false},
  "unknowns": ["…"],
  "next_human_action": "…"
}
```

Every timestamp is **UTC**, RFC3339: `generated_at` and both ends of `sample_window`. `generated_at`
matters because the write rate is a 24-hour sample taken at that moment and stales quickly;
`sample_window` is the window that sample covers, so a reader can tell whether a known outage falls
inside it. Site-local times may appear in prose for the reader, and must be labelled with the zone.

Every `*_bytes` field is bytes: the API's `*_gb` fields are GiB (1024^3) despite the name, so convert
once at the boundary and never mid-calculation. `bitrate_kbps` is kilobits per second with a 1000
multiplier, as the indexer computes it.

`verdict` must be `cannot_be_determined` whenever `write_rate.sample_representative` is false,
`disk.read_ok` is false, or `evidence_lock.source` is not `dry_run` and the last stop condition
above fires. `meets` is a claim about the future and needs all three settled.

## Security notes

This skill reads and computes. It cannot change a limit, cannot run the dry run, and cannot delete
anything. The kernel refuses that PUT to a camera-scoped credential even though the request itself
looks scope-clean, because the eviction happens later, in a sweeper loop that holds no principal.

What that sweeper does when it evicts is worth stating correctly, because "oldest-first, fleet-wide"
is the intuitive answer and it is not the current one: it picks the camera furthest **over its share**
of the cap (the cap split across cameras holding footage, weighted by `storage_quota_bytes` where
set) and deletes that camera's oldest deletable segments first. It falls back to fleet-wide
oldest-first only when no camera is over its share, and it refuses to delete anything when the
over-share camera's footage is entirely evidence-locked (`reason: over_share_all_protected`). So a
camera cannot spend another camera's budget by extending its own `retention_hours` or by locking
evidence — and a plan that assumes one camera's locks shorten everyone else's days is wrong in the
safe direction, but still wrong.

Include the request/correlation id from any failed tool call, so the numbers in this plan can be
joined to the box's own logs by whoever commits the change.
