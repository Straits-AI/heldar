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

## Vulnerability scanning: what is blocking, and what is deliberately not

Four gates run on every PR and on a weekly schedule: `npm audit` (high+), `pip-audit`, `cargo
audit`, Gitleaks, and Trivy over the filesystem. All of them fail the build. Two policy decisions
inside that are worth stating plainly, because both look like holes otherwise.

### AI dependencies are audited from pinned locks, not from the requirements files

`apps/ai/requirements*.txt` carry open floors (`ultralytics>=8.4.115`). Auditing those directly
answers "what does the resolver pick this morning?", which is not a question anyone can ship. Worse,
until #114 only `requirements-core.txt` was audited at all — so torch, opencv, the CUDA wheels and
PaddleOCR, by far the largest and fastest-moving native trees in the product, were invisible to the
blocking gate.

Each documented install profile therefore has a committed, fully pinned lock in
`apps/ai/constraints/`:

| profile | inputs | what it is |
| --- | --- | --- |
| `core` | `requirements-core.txt` | HTTP + frame decode; what the container image installs |
| `detect` | `requirements.txt` | core + YOLOv8 / ByteTrack (pulls torch) |
| `anpr` | `requirements.txt` + `requirements-anpr.txt` | detect + PaddleOCR plate reading |
| `embed` | `requirements.txt` + `requirements-embed.txt` | detect + CLIP embeddings for semantic search |

Locks are compiled for **linux x86_64, Python 3.12** — what the shipped image and the box both run.
Compiled anywhere else the `nvidia-*` wheels drop out of the tree entirely and the gate silently
stops covering them, so the platform is pinned in `scripts/lock_ai_profiles.sh` rather than left to
whoever regenerates.

Regenerate after any requirements edit, and commit the result:

```bash
./scripts/lock_ai_profiles.sh
```

CI does not regenerate — a lock produced by CI is a lock nobody reviewed. It runs
`scripts/check_ai_locks.py`, which fails if a lock is missing, unpinned, orphaned, or **stale**:
each lock records the sha256 of the exact inputs it was compiled from, so editing
`requirements.txt` without regenerating breaks the build instead of quietly auditing last month's
tree. The profile list lives in that script and nowhere else; the lock generator and the audit loop
both read it from there.

### Accepted advisories are owned and expire

A blocking gate with no pressure valve gets switched off wholesale the first time an advisory has no
fix. The valve is `security/dependency-exceptions.json`. An entry suppresses exactly one finding and
must carry the advisory id, the component, whether **our** code can reach the vulnerable path, why
it is accepted, the compensating control, an owner, a follow-up issue, and an expiry date.

`scripts/check_security_exceptions.py` is what makes "time-bounded" more than a word: it fails CI
from the expiry date onward, and rejects an expiry more than 180 days out as not a time-bound at
all. Extending an exception is a reviewable commit that says who re-accepted the risk and until
when. The audit steps take their suppression list from that file and nowhere else — a hardcoded
advisory id anywhere in the security workflow fails
`scripts/test_security_exceptions.py`.

The register is currently **empty**: all four AI profiles audit clean as of 2026-09-05.

### Trivy: `ignore-unfixed: true`, on purpose

The filesystem scan reports HIGH and CRITICAL findings and **fails** on them, but passes
`ignore-unfixed: true` — a finding with no released fix does not block a merge. This is a deliberate
choice, not an oversight: an unfixable finding blocks every unrelated PR for as long as upstream
takes, which is how a gate stops being taken seriously. Unfixed findings are still published to the
code-scanning dashboard (the SARIF upload runs `if: always()`, so it uploads even when the scan
fails), so they are visible, just not blocking.

`website/` is skipped for the same reason and is documented inline in the workflow: its HIGH
findings are Docusaurus's own build-time transitives, which never reach the deployed Worker.

Revisit both the day a finding lands on a runtime path.

### The gate self-tests, because it once did not block at all

`exit-code: "1"` was **absent** for a long stretch. The job produced SARIF, findings accumulated on
the code-scanning dashboard, and every PR reported success. From the outside a gate that never fires
and a gate that cannot fire look identical, and this one's failure path had never executed.

So before the real scan, the workflow runs the same action, at the same version, with the same
verdict-affecting inputs, against two fixtures in `scripts/fixtures/trivy-gate/` whose answer is
known:

| fixture | expected | proves |
| --- | --- | --- |
| `vulnerable/` | scan **fails** | the gate blocks on a real finding |
| `clean/` | scan **passes** | the gate is not simply red on everything |

The clean fixture is not decoration. A Trivy with a broken DB download or a bad flag fails on
everything, and would satisfy "the vulnerable fixture failed" while proving nothing.

`scripts/check_trivy_gate.py` keeps the arrangement from rotting: it compares the self-test's inputs
against the real scan rather than holding its own copy, so the self-test cannot drift into exercising
a configuration nobody ships. It also asserts `scripts/fixtures` stays in `skip-dirs` — those pins
are knowingly vulnerable, and without the exclusion they would fail every unrelated PR until someone
reasonably deleted them, taking the self-test along.

The vulnerable fixture pins four packages rather than one. Any single advisory can be re-scored or
withdrawn; four carry roughly 48 between them. **If the self-test ever reports that fixture clean,
the fixture has gone stale — add an older package, do not delete the check.**

### The images are scanned before they are published

The filesystem scan reads the source tree; it says nothing about the base image the product actually
ships on. A base digest bump could drag in a fixable HIGH and hand it to everyone who pulls
`latest`, and nothing would have looked wrong.

`docker-open.yml` now builds the amd64 half locally, scans it, and only then runs the real
multi-arch build and push. The ordering is the whole point: **a scan after the push reports on
something already handed out.** `scripts/check_trivy_gate.py` asserts a pushing build step still
follows the scan, so the two cannot be reordered back.

buildx cannot load a multi-arch image into the local daemon, hence the amd64-only pre-build. Its
layers land in the same `type=gha` cache the real build reads, so the marginal cost is the scan
rather than a second build. **arm64 is not scanned**: for the same pinned base digest the OS package
set is identical across architectures, and what differs is arch-specific wheels. Worth revisiting the
day a finding turns up in one of those.

`ci.yml` scans `heldar-ai:ci` and `heldar-web:ci` on any PR that touches an image input, under the
same policy. The release scan only runs at tag time, which is far too late to learn that a base bump
introduced a HIGH — this moves that answer to the PR that causes it. (`heldar-core` is not built at
PR time; the Rust build is too slow to be worth it on every image-touching PR.)

Every Trivy invocation in the repo is held to `exit-code: "1"` by the same guard, so a
report-only scan cannot be introduced anywhere.

### The weekly scan files an issue when it goes red

The Monday cron re-scans unchanged code. It is the only mechanism that catches an advisory published
*after* a release — there is no PR for that finding, because nothing was merged to cause it.

Which was also the problem: a scheduled run reports to nobody. No PR turns red, no author gets
notified, and a failing cron sat in the Actions tab until somebody happened to look. The one class of
finding nothing else could catch was the one nothing surfaced.

`advisory-report` now opens an issue when the weekly run fails, **updates** that same issue on
subsequent failures rather than filing a fresh one each Monday, and closes it once a later run is
clean. It matches its own issue by an HTML marker in the body, not by title or label alone, so it
cannot adopt — and later close — an issue somebody wrote by hand.

`scripts/check_weekly_report.py` enforces coverage rather than configuration: **every** job in
`security.yml` must be in the reporter's `needs`. Add a fifth scanner without listing it and its
failures become invisible again, which is exactly the state this fixed. Anything that is not
`success` counts as failing, cancelled and skipped included — a skipped scan has cleared nothing.

Note the interaction with the release gate above: while this issue is open, **releasing that commit
is refused**, because publishing requires a green security run. That is intended, and the issue body
says so.

### Publishing requires a green security run for that exact commit

`release.yml` and `docker-open.yml` both begin with a `security-gate` job
(`.github/actions/require-security-run`), and every other job in them depends on it. Publishing is
the irreversible step: a crates.io version can never be replaced, and an image tag — `latest`
especially — is pulled by everyone who trusts it. Nothing previously connected that to the scan, so
a tag could sit on a commit whose security run had failed or never finished.

The lookup is **by commit SHA**. `security.yml` does not trigger on tag pushes (it runs on main
pushes, PRs and the weekly cron), so the run being checked is the one from when that commit landed
on main. Tagging a commit that never reached main therefore blocks, by design.

The **most recent completed** run wins, deliberately: the weekly cron re-scans unchanged code, so a
newly published advisory turns a previously-green commit red — and that must block a release rather
than be overridden by an older success.

The gate fails closed. No run found, an API error, an unrecognised `enforce` value — all block. A
`workflow_dispatch` dry run reports without blocking; a real publish always enforces.
`scripts/check_release_gated.py` requires every job in both workflows to reach the gate through
`needs`, because detaching it is a one-line deletion that looks like nothing in a diff.

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
