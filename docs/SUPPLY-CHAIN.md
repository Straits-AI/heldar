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
| `Dockerfile` (builder) | `rust:1.85.1-bookworm` | tag + `@sha256` |
| `Dockerfile` (runtime) | `debian:bookworm-slim` | codename tag + `@sha256` |
| `apps/web/Dockerfile` (builder) | `node:22.23.2-bookworm-slim` | tag + `@sha256` |
| `apps/web/Dockerfile` (runtime) | `nginx:1.27.5-alpine` | tag + `@sha256` |
| `apps/ai/Dockerfile` | `python:3.13.14-slim` | tag + `@sha256` |
| `deploy/compose.yml`, `docker-compose.yml` | `bluenviron/mediamtx:1.20.0` | tag + `@sha256` |

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

## Follow-up (not implemented here)

These are the next steps to close the loop from "pinned inputs" to "verifiable outputs". They are
**intentionally out of scope for this change** and tracked as follow-up work:

- **SBOM generation** — produce a CycloneDX/SPDX SBOM per published image with
  [`syft`](https://github.com/anchore/syft) in CI, and attach it to the release / push it as an OCI
  referrer. This makes "what is actually inside `heldar-core:v1.4.0`" answerable without unpacking
  the image, and feeds vulnerability scanning (e.g. `grype`).
- **Image & binary signing** — sign the published container images and the prebuilt musl binaries
  with [`cosign`](https://github.com/sigstore/cosign) (keyless / OIDC), and sign the SBOM
  attestation too. Deployments can then `cosign verify` before running, so a tampered or
  unrecognized image is rejected at pull time rather than trusted by tag alone.

Signing is deliberately not implemented in this change; pinning is the prerequisite that makes those
signatures meaningful.
