# Evidence bundles

*Issue [#118](https://github.com/Straits-AI/heldar/issues/118). Format `heldar-evidence/1`.*

Heldar has always been able to lock recording segments against retention and export a clip. That
protects evidence from routine deletion, which is a different requirement from making it verifiable
once it has left the appliance.

A plain MP4 cannot answer which camera and site produced it, which exact UTC interval was requested,
whether that interval contained recording gaps, which source segments it was built from, who
exported it and under what authorization, or whether a single byte changed afterwards.

An **evidence bundle** answers all of those, in a document that is signed and checkable without
Heldar, without the appliance, and without a network.

## What a bundle contains

```text
manifest.json             the claim: what this is, where it came from, what is missing from it
media/clip.mp4            the footage, remuxed (-c copy) — recorded frames, not a re-encode
metadata/events.jsonl     operational events in the window (gaps, reconnects, offline)
metadata/detections.jsonl detections in the window, with their confidences
metadata/audit.jsonl      the audit trail for this camera in the window, plus this export's own row
metadata/coverage.json    requested vs actually-recorded seconds, and every gap
metadata/camera.json      the camera as configured at export time
hashes.sha256             the manifest's hashes, in `sha256sum -c` format
signature.json            Ed25519 over the canonical bytes of manifest.json
```

The manifest carries a sha256 for every file above, the source segment ids with their own hashes,
the exporting principal, the audit and request ids, and the Heldar version that produced it.

## Exporting

```bash
# 1. Plan (the default). Nothing is written; you see the gaps and the size first.
curl -sX POST $HELDAR/api/v1/evidence/exports -H "Authorization: Bearer $KEY" \
  -H 'content-type: application/json' \
  -d '{"camera_id":"cam_front","from":"2026-08-30T02:00:00Z","to":"2026-08-30T02:05:00Z"}'

# 2. Produce it.
curl -sX POST $HELDAR/api/v1/evidence/exports -H "Authorization: Bearer $KEY" \
  -H 'content-type: application/json' \
  -d '{"camera_id":"cam_front","from":"...","to":"...","dry_run":false}'
```

`incident_id` may be given instead of `camera_id`, in which case the camera is derived from the
incident's segments. An incident spanning several cameras is refused rather than resolved to one of
them: a bundle attests to one camera's footage, and silently picking one would produce a document
narrower than the incident an investigator believes it covers.

Requires `video:export`. Camera-scoped credentials can export only their own cameras — including
through `incident_id`, where the scope check runs against the *derived* camera, not the id supplied.

## Verifying — offline

```bash
./scripts/verify_evidence_bundle.py incident.heldar-evidence --key-id sha256:<expected>
```

Needs `python3` and `openssl`. It does **not** need Heldar, the appliance, its database, or a
network. That is the property that makes a bundle evidence rather than an export.

Get the expected key id from `GET /api/v1/evidence/signing-key` — **out of band**, by some route
other than the bundle itself. Without `--key-id` or `--key` the verifier reports `UNKNOWN-KEY`, not
`VALID`, because a bundle carrying its own public key proves only that whoever made the bundle also
made the key.

| Exit | State | Meaning |
|---|---|---|
| 0 | `VALID` | signature good, every hash matches, key is the one you expected |
| 1 | `MODIFIED` | a file's content differs from the signed manifest, or the signature fails |
| 2 | `MISSING` | the manifest lists a file the bundle does not contain |
| 3 | `UNKNOWN-KEY` | self-consistent, but the key is unverified or not the one you named |
| 4 | `UNSUPPORTED` | a format version this verifier does not understand |
| 5 | `MALFORMED` | not a bundle: unreadable zip, absent manifest, unparseable JSON |

`MISSING` and `MODIFIED` are deliberately distinct: "this was altered" and "part of it was not handed
to you" are different accusations.

## What a signature here does and does not establish

**It does establish** that the appliance holding this key produced this bundle, and that its bytes
have not changed since.

**It does not establish when.** The appliance stamps its own clock. An appliance whose clock is wrong
signs the wrong time faithfully. This is not a trusted timestamp, and pairing it with an external
timestamping authority is future work.

**It does not establish that any detection is correct.** An included detection is a record of what a
model reported, at the stated confidence, at that time.

**It does not hide gaps.** `media.gaps` and `covered_seconds` are in the *signed* manifest, so a
bundle spanning an outage says so in the same document that attests to it. The clip concatenates
across gaps because the missing footage does not exist — presenting that as continuous video of a
discontinuous night would be worse than not exporting at all. The verifier prints the gap count on a
`VALID` result for the same reason.

## The signing key

An Ed25519 key pair at `$HELDAR_DATA_DIR/evidence-signing-key.pkcs8`, generated at first use, mode
0600. It signs evidence manifests and nothing else.

It is deliberately **not** `HELDAR_SECRET_KEY`. That key encrypts camera credentials at rest: it is
symmetric, every process that can build a camera URL holds it, and possession of it proves nothing
to a third party. An evidence key is the opposite shape — the private half never leaves the box, the
public half is published, and the point is that someone who does *not* trust the operator can still
check the signature.

A corrupt key file is an error, never quietly replaced: minting a fresh key would invalidate every
bundle already handed out while reporting success, and an investigator holding one would be told the
signature came from an unknown key.

Back it up with the appliance. If it is lost, previously exported bundles remain verifiable by
anyone who recorded the key id, but the appliance can no longer produce bundles under the identity
recipients have already pinned.

On a box where an attacker has root, they can sign bundles. An appliance-held key raises the bar from
"anyone with a hex editor" to "root on the recorder" — that is the honest description of what it
buys. [#126](https://github.com/Straits-AI/heldar/issues/126) is where the key becomes loadable from
an HSM or external secret provider.

## Relationship to plain clip export

`POST /api/v1/cameras/{id}/clip` is unchanged and stays available. It is the right tool for showing
someone a few minutes of footage. It is not evidence: nothing about the resulting MP4 is verifiable
once it leaves the box. Use a bundle whenever the file may need to be defended later.

## Retention

Bundles live in `$HELDAR_EVIDENCE_DIR` (default `$HELDAR_DATA_DIR/evidence`) and are indexed in the
`evidence_bundles` table so `GET /api/v1/evidence/exports` can list what has left the appliance. The
bundle itself is self-contained and verifiable without that table — the index is a record of
exports, not the evidence.
