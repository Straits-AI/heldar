# Supply chain: image pinning & reproducibility

Heldar runs as a small set of containers (kernel/core, web, optional AI worker) plus the MediaMTX
media server. This document is the policy for how those images — and the base images they are built
from — are pinned, why, and how to update them safely.

## Why we pin

A floating tag (`latest`, `1`, `bookworm-slim`) resolves to *whatever the registry points it at
today*. For a box that records continuously for months, that is a liability: a container restart or
a `docker compose pull` can silently swap in a new MediaMTX, a new Debian base, or a new kernel build
with no changelog and no decision. A single bad segment or a codec regression then shows up as
"recording is black" long after the change that caused it.

The fix is boring on purpose: every image reference is pinned to **a concrete version tag plus an
`@sha256` digest**. The digest is the actual immutability guarantee — if the tag is ever re-pointed,
the digest still resolves to the exact bytes we tested. The human-readable tag stays alongside it so
a reader can see the version at a glance.

## What is pinned

| Where | Image | Pin style |
|-------|-------|-----------|
| `Dockerfile` (builder) | `rust:1.98.0-bookworm` | tag + `@sha256` |
| `Dockerfile` (runtime) | `debian:bookworm-slim` | codename tag + `@sha256` |
| `apps/web/Dockerfile` (builder) | `node:24.20.0-bookworm-slim` | tag + `@sha256` |
| `apps/web/Dockerfile` (runtime) | `nginx:1.31.4-alpine` | tag + `@sha256` |
| `apps/ai/Dockerfile` | `python:3.14.7-slim` | tag + `@sha256` |
| `deploy/compose.tls.yml` | `caddy:2.11.4` | tag + `@sha256` |
| `deploy/compose.yml`, `docker-compose.yml` | `bluenviron/mediamtx:1.20.1` | tag + `@sha256` |

`scripts/check_pinned_versions.py` holds this table to the tree in CI. It is a policy document that
names concrete versions, so it goes stale exactly when someone merges the bump it describes — four of
these six rows were wrong before that check existed, three of them from a single afternoon of
dependency merges.

The Rust builder tag tracks the workspace MSRV (`rust-version = "1.85"` in `crates/*/Cargo.toml`);
keep the builder at or above that.

### The one deliberate exception: the ghcr `heldar-*` images

`deploy/compose.yml` pulls `ghcr.io/straits-ai/heldar-{core,web,ai}` at
`${HELDAR_VERSION:-latest}`. This is intentional so the open-source quickstart — a plain
`docker compose up -d` — works with zero configuration.

**Production deployments MUST set `HELDAR_VERSION` to a pinned release tag** (e.g.
`HELDAR_VERSION=v1.4.0` in `.env`). With it set, a restart or re-pull can never jump the kernel to a
newer build; without it, `latest` floats exactly like the tags we removed everywhere else. The
`compose.yml` header repeats this warning next to the images themselves.

## Resolving and updating a digest

Digests are the **multi-arch image index** digest (the manifest list), so the same pin works on
amd64 and arm64. To resolve one:

```sh
docker buildx imagetools inspect rust:1.85.1-bookworm
# The top-level "Digest:" line is the index digest — that is the one to pin.
```

To bump a base image:

1. Pick the new concrete tag (a specific minor.patch, not `latest`/`1`).
2. Resolve its index digest with the command above.
3. Update the `FROM ...@sha256:...` line (or the compose `image:` line) with **both** the new tag and
   the new digest. Never hand-edit a digest or copy one from another arch/tag — an invalid digest
   fails closed (the pull errors) but wastes a build; a *valid digest for the wrong image* is worse.
4. Verify the tag and digest agree: re-run `imagetools inspect <tag>` and confirm the printed digest
   matches what you wrote.

Never invent or guess a digest. If you cannot reach a registry, pin to the concrete tag alone and
leave a `TODO pin @sha256 digest` comment rather than fabricating one.

## Automated bumps: Dependabot

`.github/dependabot.yml` runs weekly for the `docker` ecosystem (the three Dockerfiles), the
`docker-compose` ecosystem (both compose files), `github-actions`, `cargo`, `npm` (`apps/web`), and
`pip` (`apps/ai`). Dependabot opens a PR that updates both the tag and the digest together, so pins
move on a reviewed, deliberate cadence instead of drifting silently on a re-pull. Treat those PRs as
the normal path for base-image and MediaMTX updates.

## Verifiable outputs: SBOMs, signatures and provenance

Pinning answers "what did we build from". These answer "is this the thing we built", which a sha256
sidecar cannot: a checksum only detects corruption once you already trust where the checksum came
from, and it authenticates nobody.

Every published musl binary and every pushed `ghcr.io/straits-ai/*` image carries:

- an **SPDX SBOM** (syft) — what is actually inside, without unpacking it,
- a **build-provenance attestation** binding the artifact to the repository, commit, workflow and
  tag that produced it,
- an **SBOM attestation** binding that SBOM to the same artifact.

The **API contract** (`openapi.json`, #120) is published with every release. It is what a generated
client is built from, and it is what the next release's contract diff compares against — a breaking
change is allowed on this pre-1.0 appliance, but shipping one by accident is not, because it breaks
a client at runtime in someone else's deployment.

The **release manifest** (`heldar-release-manifest.json`, #112) carries a build-provenance
attestation too. It is the one artifact that is not a thing you run — it is the document naming
which binaries, images and deployment files belong to one release, so an unattested one could be
swapped for a manifest pinning a different combination and the pinning would authenticate nothing.

All of these are signed **keylessly** through Sigstore using the workflow's short-lived OIDC identity.
There is no private signing key in repository secrets — which is the point, since a stolen key
forges everything a signature is supposed to prevent.

### Verifying before you run something

```bash
# A published binary. The asset name carries the version and arch — see the download snippet in
# docs/PRODUCTION.md, which builds the same string.
gh attestation verify "heldar-core-$V-$ARCH-linux-musl" --repo Straits-AI/heldar

# A pushed image, by DIGEST
gh attestation verify oci://ghcr.io/straits-ai/heldar-core@sha256:<digest> \
  --repo Straits-AI/heldar

# The release manifest, then the deployment it describes. Verify the manifest FIRST — checking a
# deployment against an unverified manifest proves only that it matches something.
gh release download "$V" -p heldar-release-manifest.json
gh attestation verify heldar-release-manifest.json --repo Straits-AI/heldar
HELDAR_DB=/var/lib/heldar/heldar.db \
  ./scripts/verify_release_manifest.py heldar-release-manifest.json
```

Verify the **digest**, never the tag. A tag is mutable, so verifying one certifies whatever it points
at right now — exactly the substitution these attestations exist to detect. Resolve the digest first
(see "Resolving and updating a digest" above), then verify that.

Image attestations are pushed to the registry as OCI referrers (`push-to-registry: true`), so they
travel with the image and can be checked from a mirror.

### Immutability of a published tag

A tag is immutable to everyone who already pinned it, so re-running a release must not change the
bytes under one. Every upload goes through `scripts/release_upload.sh`, which compares the local
digest against what is already published and **refuses a change**:

```
::error::heldar-core-v0.6.0-x86_64-linux-musl is already published under v0.6.0 with DIFFERENT bytes.
::error::  published: 9f2c…
::error::  building:  4ab1…
```

An *identical* re-upload stays a no-op, deliberately: a release workflow that cannot be re-run after
an infrastructure flake is its own hazard. An existing asset that cannot be fetched to compare is a
refusal, not a pass.

`gh release upload --clobber` did the opposite — it replaced silently — and
`crates/heldar-server/tests/supply_chain.rs` now fails if any raw upload reappears in the workflow,
because adding one more upload line is the easiest way to reopen this and would look normal in review.

### Still open

- **Offline verification.** `gh attestation verify` reaches Sigstore's transparency log. An appliance
  with no egress needs the trust material cached first (`gh attestation trusted-root`); we do not yet
  ship that bundle or a verifier that uses it. Tracked in #115.
- **The release job does not verify its own attestations.** It creates provenance and SBOM
  attestations but never runs `gh attestation verify` against the artifacts before completing the
  release, so a broken attestation would publish successfully. #115 asks for this explicitly.
- **No negative test proves verification fails on a modified artifact.** #115 asks for one; the
  digest guard above covers replacement at upload time, which is a different thing from proving the
  published verification path rejects tampering.
- **Private/full images** built outside this repository are not covered here; the same three steps
  belong in whatever workflow publishes them.

## Follow-up (not implemented here)

Nothing outstanding here beyond the two items listed under "Still open" above — SBOMs, signing and
provenance are implemented in `.github/workflows/release.yml` and `docker-open.yml`, and the release
manifest they pin together is described in `docs/PRODUCTION.md`.
