# api-client — integrating against the generated contract

A worked example of talking to a Heldar box **without hand-writing HTTP**.

```bash
python3 recording_health.py --url http://box:8000 --token vok_...
```

Answers the question a recorder exists to answer — *did every camera actually record last night?* —
and names the gaps, which is the part a green dashboard will not tell you.

```
recording health · 24h to 2026-08-30T16:27:06Z
  cameras checked: 3
  cam_gappy                1 gap(s), 1h30m missing
  cam_off                  recording disabled — not checked

  total missing: 1h30m
```

Exit **0** when every camera recorded continuously, **1** when a gap was found, **2** when the box
could not be reached or refused the credential — so a monitoring system can branch on it.

## Why a generated client and not `curl`

A `curl` snippet is a fine way to *see* one endpoint and a bad model for an *integration*. It
hardcodes paths, invents its own error handling, and goes stale silently when a field is renamed.

This script names methods generated from the contract the server publishes
([#120](https://github.com/Straits-AI/heldar/issues/120)), so a breaking change is an
`AttributeError` at the top of a run rather than a `KeyError` three hours into a night shift. CI
regenerates the clients and fails if they drift from the contract, and diffs the contract against the
last release to catch a breaking change before it ships.

Regenerate:

```bash
cargo test -p heldar-server --test openapi_contract write_the_served_document
python3 scripts/gen_clients.py target/openapi.json clients
```

TypeScript and Rust clients are generated from the same document — see `clients/`.

## What it deliberately does not do

**It does not decide whether a gap is acceptable.** A camera powered off for maintenance and a camera
that silently stopped recording produce *identical* gaps, and only your operations know the
difference. The script reports; the judgement is yours. A tool that guessed here would be wrong in
exactly the cases that matter.

## Credentials

Needs `camera:read` and `video:playback`. A camera-scoped key works and sees only its own cameras —
that is the correct answer for it, not an error. Mint one with:

```bash
curl -sX POST $HELDAR/api/v1/api-keys -H "Authorization: Bearer $ADMIN" \
  -H 'content-type: application/json' \
  -d '{"name":"nightly-health","role":"viewer","capabilities":["camera:read","video:playback"]}'
```
