---
name: heldar-privacy-audit
version: 1.0.0
summary: Report this box's privacy and security posture as pass / fail / unverified findings, counting every control that could not be assessed as unverified rather than clean.
compatible:
  core_api: ">=0.1.0 <1.0.0"
permitted_tools:
  - get_security_posture
  - get_system_health
  - get_retention_limits
  - get_backup_status
  - list_cameras
  - get_camera_health
  - get_timeline
  - get_recording_gaps
  - list_ai_workers
  - heldarctl status
  - heldarctl doctor
  - heldarctl version
prohibited_actions:
  - actuate a gate, relay or PTZ
  - delete recordings, evidence or weaken retention
  - create, modify or retrieve credentials
  - identify a person from appearance similarity alone
  - assert that nothing happened without first checking recording gaps
  - present a correlation or hypothesis as an observation
---

# Privacy audit

## Purpose

Report what this box's privacy controls actually are: credential breadth, camera scope, retention,
encryption at rest and in flight, remote exposure, and what the AI/agent surface is allowed to reach.

The specific wrong answer this exists to prevent is the sentence **"the audit found no issues"** on a
box whose volume encryption, `/proc` visibility and service account were never examined — a clean
bill of health assembled from the controls that happened to be checkable.
`GET /api/v1/system/posture` returns `unknown` for every control it could not assess from inside the
container, and on a healthy-looking box `unknown` outnumbers `weak`. An agent that reads the
response's `weak` count, sees `0`, and stops has issued a pass for every control that came back
`unknown` — controls whose state it never saw.

There is exactly one mapping, and it is not negotiable:

| Posture `status` | Audit verdict |
|---|---|
| `ok` | **pass** |
| `weak` | **fail** |
| `unknown` | **unverified** — never a pass, never a fail |

## Inputs

- Optional: the specific question being asked ("is footage encrypted at rest?", "who can see the car
  park camera?"). Answer that question *and* return the full finding set; a single-question audit is
  how the other five controls go unlooked-at.
- Optional: a prior audit's output to diff against. Posture is evaluated at request time and holds no
  history, so a diff is only ever between two readings you can both date.

## Prerequisites

- A **fleet-scoped admin** credential. `get_security_posture` requires `can_admin` *and* refuses a
  camera-scoped credential outright — the findings describe the host, which is the reconnaissance an
  attacker with a narrow foothold would want.
- `docs/PRODUCTION.md` and `docs/ACCESS-CONTROL.md` are the reference for what each control means.
  Read them before writing remediation text; do not invent a remediation.
- **No permitted tool lists credentials, their capabilities, or their camera scopes.** There is no
  read-only MCP tool over `GET /api/v1/api-keys`. Credential breadth is therefore an *unverified*
  item in every run of this skill, with a named human action attached — not an omission, and not a
  pass.

## Workflow

1. **Establish what your own credential proves, before reading anything else.** Call
   `get_security_posture` first.
   - **403** — you are non-admin or camera-scoped. The audit has not started; it has failed to start.
     Every host-level control is `unverified`. Do not substitute `heldarctl doctor` (step 8).
   - **200** — this does **not** prove you hold a privileged key. With `HELDAR_AUTH_ENABLED=false`
     every request is a synthetic fleet-wide admin, so a 200 is equally consistent with a box that
     has no authentication at all. Nothing in the read surface distinguishes the two. Say so.
2. **Transcribe every entry in `findings[]` verbatim** (six on a current box) — `id`, `status`,
   `detail` — and report the response's
   `weak` **and** `unknown` counts side by side. A report quoting only `weak` is the wrong answer
   this skill exists to prevent. What `unknown` means per finding:
   - `secret_key_source` — the key is in use; where it came from could not be re-read. `ok` means it
     is a file or a systemd credential, not an env var.
   - `process_visibility` — non-Linux host, or `/proc/self/mountinfo` unreadable. Note regardless of
     status: camera credentials **are** in ffmpeg's argv, because RTSP authenticates from the URL.
     `ok` means `/proc` is mounted `hidepid`, not that the exposure is gone (ADR 0006, still open).
   - `service_user` — not a unix host.
   - `recording_volume_encryption` — the common case, including "not on a device-mapper volume",
     which is `unknown` **not** `weak`: provider-side and filesystem-level encryption are invisible
     from inside the container. And `ok` is "consistent with LUKS", not proof — `cryptsetup status`
     is the check and it is off-box.
   - `rtsp_transport` — never `unknown`; it counts enabled cameras whose main or sub URL is plain
     `rtsp://`. It says nothing about whether the camera VLAN is segmented.
   - `plaintext_credentials` — never `unknown`; counts camera passwords stored without the `enc:v1:`
     marker. Encryption applies **on write**, so setting a key seals nothing retroactively; the only
     remediation is re-saving each camera (`heldar-core rekey-secrets` is key *rotation* and refuses
     to run without `HELDAR_SECRET_KEY_OLD`).
3. **State what is not encrypted at all.** Heldar does not encrypt recorded segments; footage at rest
   depends entirely on the volume, which is the finding that most often reads `unknown`. Then call
   `get_backup_status`: a completed job with a non-null `destination_id` copied footage somewhere,
   and the job row does **not** say where — it carries the destination's *id*, not its kind, and no
   permitted tool reads `/api/v1/backup/destinations`. An off-box destination (`sftp`/`ftp`/`s3`) and
   an on-box one (`local`) are indistinguishable from here, so list the destination ids as
   `unverified` with the human action; never report "backups stay on the box". Whatever did leave is
   past every later scope check, revocation and retention sweep.
4. **Report the retention *policy* and the retention *caps* as two different mechanisms.** The period
   is per camera: `list_cameras` returns `retention_hours` (and `storage_quota_bytes`), and the
   sweeper's first pass deletes every unlocked segment older than that camera's `retention_hours`.
   `get_retention_limits` is a *separate* eviction path — a fleet-wide **size cap**
   (`max_recordings_gb`) and a **free-disk floor** (`min_free_disk_gb`), each with an `*_overridden`
   flag saying whether an operator set it or it defaulted — and it evicts oldest-first regardless of
   age, so footage can vanish well before its camera's `retention_hours`. Report both, plus
   `get_timeline`'s oldest range per camera as **observed so far**: where the observed horizon is
   shorter than `retention_hours`, the caps are what is actually deciding, and it moves with the
   write rate. Evidence-locked segments are exempt from all of it, so locked footage is retained
   indefinitely whatever the policy says. Audit-log retention is a fourth, separate horizon
   (`HELDAR_AUDIT_RETENTION_DAYS`, default 365): the setting has no read route and no permitted tool
   reads `GET /api/v1/audit` — `unverified`.
5. **Do not use camera counts as a scope oracle.** `list_cameras` and `get_system_health`'s
   `cameras_total` are confined to the *same* scope — the caller's — so comparing them detects
   nothing and proves nothing about the fleet's size. `get_system_health.enforcement.machine_auth` of
   `off` or `warn` means credentials carrying no explicit grant keep their full role reach; *which*
   credentials those are appears only in the boot banner, not in any API response. Record credential
   breadth as `unverified` with the human action: read `GET /api/v1/api-keys` (capabilities and
   `scope_cameras` per key) and `GET /api/v1/users` with a fleet admin, and read the box's
   machine-credential boot banner.
6. **Read remote exposure from the two fields that carry it, and stop there.**
   `get_system_health.remote_access` describes only the WireGuard-style overlay interface;
   `relay` describes the WebRTC rendezvous dial-out. `relay.configured: true` does imply auth is
   enabled — the relay refuses to run otherwise, and the kernel refuses to boot with a remote path
   configured and auth off. `relay.configured: false` implies nothing. Neither field says anything
   about the bind address, host firewall, CORS allowlist, cookie `Secure` flag, session TTL or idle
   timeout: all of those are boot configuration with no read route. `unverified`, every one, and
   never inferred from the absence of a warning.
7. **Report what the AI/agent surface actually reaches.** `list_ai_workers` returns per-camera
   sampler status — which cameras have frames continuously decoded for AI consumption, which is
   itself a privacy fact worth naming. It does **not** report which credential holds a lease, despite
   its description. `enforcement.ingest_provenance` of `warn` or `off` means ticketless AI ingest is
   accepted, so a detection can be attributed to a camera with no server-issued frame ticket. Add,
   from `docs/MCP.md`: the MCP sidecar strips passwords, tokens and stream URLs but **discloses
   camera device addresses** to whatever model is on the other end, by design.
8. **Cross-check with `heldarctl doctor --json`, and know its two blind spots first.** The flag is
   `--json` or `--output=json`; `--output json` is not parsed, and silently yields human output. On a
   credential that cannot read the posture, doctor's posture call 403s to stderr and the run prints
   *"no warnings or blocking findings"* — a clean report from an audit that read nothing. And its
   human-readable output drops every `info` finding, which is exactly where posture's `unknown`
   findings land. Read `findings[].code` and `severity` from the JSON. Doctor's severities are its
   own (`weak`→warning, `unknown`→info); do not adopt them as audit verdicts, and never quote its
   exit code as an audit result.

## Stop conditions

Stop and hand to a human when:

- **`get_security_posture` answers 403.** The audit is not clean, it is not started. Report that and
  the credential's apparent limitation, nothing else.
- **A finding the question depends on is `unknown`.** "Is footage encrypted at rest?" with
  `recording_volume_encryption: unknown` has no answer from in here. Hand over the off-box command:
  `cryptsetup status <device>` for the volume, `findmnt /proc` for `hidepid`.
- **The question is which credentials exist, what they may do, or which cameras they are scoped to.**
  No permitted tool answers it, and `list_cameras` returning a short list is not evidence of a scope.
- **The question is about auth being on, TLS, CORS, cookie flags, session lifetime, the bind address
  or the firewall.** These are boot configuration; a running box does not expose them.
- **Someone asks for a score, a percentage, or a compliance statement** (GDPR, PDPA, SOC 2, a DPIA
  sign-off). This skill produces findings. A compliance conclusion is a named human's, and a score
  hides the one control that mattered.
- **Someone asks you to confirm that footage older than N days no longer exists.** Three sweeps can
  delete a segment and none of them is a guarantee about a period: per-camera age
  (`retention_hours`), per-camera quota, and the fleet-wide size cap and disk floor, all oldest-first.
  Evidence-locked segments are exempt from every one. Orphaned files are reclaimed on a slow sweep,
  so a row can be gone while bytes remain. An absence in a timeline is not proof of deletion. Check `get_recording_gaps`
  before making any statement about a period being uncovered, and even then report *not recorded*
  rather than *deleted*.
- **`get_backup_status` returns any job with a non-null `destination_id`.** Whether that job's
  footage left the box cannot be answered from here — the row carries the destination id, not its
  kind. Report the ids and hand over `GET /api/v1/backup/destinations`, read with a fleet admin.
- **A camera's oldest recorded range is newer than `now - retention_hours`.** Something other than
  the age policy is evicting it: the fleet size cap, that camera's `storage_quota_bytes`, or the
  disk floor. Report the two numbers side by side and hand over which one is binding; do not pick a
  cause from in here, and do not quote `retention_hours` as the footage the box holds.
- Answering would require a tool this skill does not permit.

## Output

```json
{
  "assessed_at": "UTC",
  "box": {"api_version": "…", "version": "…", "credential_reach": "fleet_admin|scoped_or_unprivileged|indistinguishable_auth_may_be_off"},
  "counts": {"pass": 0, "fail": 0, "unverified": 0},
  "findings": [
    {"id": "secret_key_source|process_visibility|service_user|recording_volume_encryption|rtsp_transport|plaintext_credentials|…",
     "verdict": "pass|fail|unverified",
     "source": "posture|system_health|retention|backup|ai_samplers|heldarctl_doctor",
     "observed": "the tool's own detail string, verbatim",
     "why_it_matters": "…",
     "remediation": "… (from docs/PRODUCTION.md, not invented)"}
  ],
  "unverified_requires_human": [
    {"control": "…", "why_not_assessable": "…", "human_action": "the exact off-box command or route"}
  ],
  "retention": {"per_camera": [{"camera_id": "…", "retention_hours": 0, "storage_quota_bytes": null,
                                "observed_oldest_footage_at": "UTC|null"}],
                "max_recordings_gb": 0, "max_overridden": false, "min_free_disk_gb": 0,
                "min_free_overridden": false,
                "audit_log_retention_days": "unverified — the setting has no read route",
                "note": "retention_hours is the per-camera age policy; the size cap and disk floor are a separate oldest-first eviction path that can delete footage sooner"},
  "exposure": {"overlay": "…", "relay_configured": false, "relay_healthy": false,
               "machine_auth": "off|warn|enforce", "ingest_provenance": "off|warn|enforce",
               "cameras_sampled_for_ai": ["…"],
               "backup_destination_ids": ["… (kind not readable from a job row)"],
               "not_assessable": ["auth_enabled", "cors_origins", "cookie_secure", "session_ttl", "bind_address", "firewall"]},
  "not_assessable": ["credential inventory", "camera scopes", "audit-log retention",
                     "backup destination kinds", "…"],
  "next_human_action": "…"
}
```

Every timestamp is **UTC** — `assessed_at`, `observed_oldest_footage_at`, and any time quoted inside
a `detail` string. Posture carries no timestamps of its own: it is evaluated at request time, so
`assessed_at` is the only date the whole finding set has, and it dates all of it. Every value here is
live and mutable — scope, retention and enforcement tiers can change a minute after you read them —
so an undated copy of this output is not a statement about the box.

`counts.unverified` may legitimately be the largest of the three. An audit with zero `fail` and five
`unverified` is an honest audit of a box nobody has finished hardening; the same audit reported as
"no issues found" is the wrong answer.

## Security notes

This skill reads. It cannot mint, read, rotate or narrow a credential, cannot change a retention
setting, and cannot enable a control it reports as missing — every remediation goes to a human with
the route or command written out.

The posture output is designed to be safe to paste into a support ticket: it never carries a secret
or a camera URL. Nothing else you collect has that guarantee — device addresses reach model context
through the MCP sidecar by design, so treat the audit itself as containing network topology and hand
it over accordingly.

Include the request/correlation id from any 403 or failed call in the output. A 403 is an
authorization boundary doing its job, and the id is what joins your finding to the box's audit log.
