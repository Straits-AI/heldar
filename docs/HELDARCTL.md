# heldarctl

*Issue [#122](https://github.com/Straits-AI/heldar/issues/122).*

The supported operator and automation interface. The dashboard is for watching; `heldarctl` is for
installing, supporting, and scripting — including on a box with no browser.

## Setting up a context

A context names a deployment. It records **where the token comes from, never the token** — a config
file full of bearer tokens satisfies "not in shell history" while being worse, because a file gets
committed, backed up and copied to a laptop.

```bash
heldarctl context add --name site-a \
  --url https://box.local:8000 \
  --token-env HELDAR_TOKEN            # or --token-file /run/secrets/token, or neither for stdin

heldarctl context list
heldarctl context use site-a
```

Add `--ca /etc/heldar/pki/ca.pem` for a box with a private certificate authority. The config lives at
`$XDG_CONFIG_HOME/heldar/contexts.json` (override with `$HELDARCTL_CONFIG`) and is written `0600` —
it holds no secret, but it holds the shape of your fleet.

## Commands

```bash
heldarctl version                # this CLI, and the API contract it speaks
heldarctl status                 # what the box is, and whether it is recording
heldarctl doctor                 # what is wrong with it
heldarctl retention              # the recording disk limits
heldarctl retention set --max-gb 40          # PLANS the change, applies nothing
heldarctl retention set --max-gb 40 --yes    # applies exactly the plan it printed
```

Every command takes `--context <name>` and `--output=json`.

## `doctor`

The flagship workflow, and the one to put in CI.

```bash
heldarctl doctor            # human
heldarctl doctor --json     # stable, for scripts
```

It does **not** re-derive what the box already knows. Security posture comes from
`/api/v1/system/posture`, camera health from the box's own status — a second implementation of "is
this healthy" is a second answer, and the one an operator sees would eventually disagree with the one
the box acts on. `doctor` adds what only a *client* can see: can I reach it, and does my contract
version match.

### Severity decides the exit code

| Severity | Meaning | Blocks? |
|---|---|---|
| `blocking` | the box is not doing its job, or its answers cannot be trusted | yes |
| `warning` | degraded or exposed, still recording | no |
| `info` | worth knowing | no |

**A camera that is enabled and not recording is always blocking.** That is the failure a video
recorder exists to prevent, and it is invisible from a dashboard showing a green tile because the
camera is reachable.

A scheduled camera outside its window is **not** a finding — reporting it would train an operator to
ignore this check, which is how the real one gets missed.

## Exit codes

```text
0  success
1  invalid input or usage
2  authentication failed
3  the server could not be reached
4  contract incompatibility — this CLI's answers would be unreliable
5  findings present at a blocking severity
6  the server returned an error
```

`5` is separate from `6` deliberately: `doctor` finding a broken camera is not the same event as
`doctor` failing to run, and CI wants to treat them differently.

```bash
# Gate a commissioning pipeline on it:
heldarctl doctor --json > findings.json || {
  test $? -eq 5 && jq '.findings[] | select(.severity=="blocking")' findings.json
  exit 1
}
```

## What it never prints

A token, a camera password, a signed media URL, or an RTSP URL with credentials in it. CLI output
gets pasted into tickets and chat far more readily than a server log does.

## A mutation is a dry run until you say otherwise

`retention set` without `--yes` prints what would happen and **changes nothing**. That is the same
way round as the evidence export's `dry_run` default: the destructive direction is the one you have
to ask for.

```console
$ heldarctl retention set --max-gb 3
plan eba495dadeb888b2957ce53c76bff96dfa9eab276b6df987f550b2f0209e6efd
  cap becomes 3.0 GB, 0.0 GB recorded now
  would evict 0.0 GB (0.0 GB is evidence-locked and cannot be freed)
  Committing this evicts nothing now.

Nothing changed. Re-run with --yes to apply exactly this plan.
```

With `--yes` it still plans first, prints the same effect, and then commits **carrying the plan hash
it just received**. That is what makes the printed effect meaningful rather than decorative: if
anything the plan depended on moved in between — another operator changed the cap, the recorded
footprint grew past it — the box refuses the commit rather than applying a change to a state nobody
looked at. You get exit `6` and the server's own explanation of what to do.

Shrinking the cap below what is already recorded **deletes the oldest footage fleet-wide** on the next
sweep, so the effect prints on the `--yes` path too. An operator who typed the wrong number should
read it in the terminal, not learn it from a retention sweep.

### The idempotency key is derived from the plan, not random

A `Idempotency-Key` generated per invocation would protect against almost nothing. The case that
matters is an operator whose command timed out and who runs it again — and a fresh key makes that a
second distinct request.

The key is derived from the plan hash instead. A re-run against an unchanged box produces the same
hash, so the same key, and the box replays its original answer rather than applying the change twice.
If the box *did* change, the hash differs and so does the key — and the plan check refuses the commit
anyway. The two guards agree by construction rather than by coincidence.

## Not here yet

The rest of the mutating surface: camera, event, incident, search, ai, backup, evidence and auth.
`retention` is first because the server already implements dry-run and plan hashes for it (#121), so
the safety pattern above could be demonstrated end to end before being copied.
