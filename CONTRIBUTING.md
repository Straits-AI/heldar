# Contributing to Heldar

Thanks for your interest. Heldar is the **open core** of a visual event-intelligence platform: an
Apache-2.0 media/perception kernel plus generic reference apps. This repo is the place to improve the
*platform* and the *generic* apps — vertical/client-specific products live in a separate private repo
and are out of scope here.

## What belongs in this repo

| In scope (Apache-2.0) | Out of scope (proprietary, elsewhere) |
| --- | --- |
| `heldar-kernel` — media/DVR, perception ingest + sampler, zone engine, auth/RBAC, retention, remote-access awareness, worker SDK | Vertical/client products (retail analytics, access-control suites, etc.) |
| `heldar-entry` (access control), `heldar-movement`, `heldar-search` — generic reference apps | Client-specific dashboards, integrations, or business logic |
| `heldar-server` reference bin, `apps/ai` reference worker, `apps/web` reference dashboard, docs | — |

If a change is specific to one customer or vertical, it doesn't go here. If it makes the kernel or a
generic app better for everyone, it does.

## Dev setup

Prerequisites: Rust (via `rustup`), FFmpeg + ffprobe on `PATH`, Node.js (frontend), Python 3 (AI worker).

```bash
rustup update
cargo build --workspace
cd apps/web && npm ci && cd -    # dashboard deps
scripts/setup_mediamtx.sh        # fetch the MediaMTX live-view gateway
scripts/run_stack.sh             # MediaMTX + core (:8000) + web (Vite)
```

The per-stage `scripts/validate_*.sh` scripts exercise each capability end-to-end against a running
stack and write reports to `data/` (override with `HELDAR_DATA_DIR`). Those that need a registered
camera default to `cam_192_168_0_2`; point them at yours with `CAM=<camera-id>`.

## Quality bar (CI will check these)

Before opening a PR, all of these must pass:

```bash
cargo fmt --all -- --check                                     # formatted
cargo clippy --workspace --all-targets --locked -- -D warnings # warning-free (CI denies warnings)
cargo build --workspace --locked
cargo test --workspace --locked

# cross-app reads must go through the owner-published *_read contract views:
./scripts/check-read-seam.sh

# the OPEN reference build must link zero proprietary code (and no wasm runtime by default):
cargo build -p heldar-server --no-default-features --locked

# off-by-default features must still compile + lint:
cargo clippy -p heldar-server --features wasm --all-targets --locked -- -D warnings
cargo build -p heldar-server --features smtp --locked

# frontend:
cd apps/web && npm ci && npm run build
```

- **Design decisions are recorded.** [`docs/adr/`](./docs/adr) holds the decision records — why the
  system is the way it is, as opposed to how it works today (that's [ARCHITECTURE.md](./ARCHITECTURE.md)).
  A decision that changes a seam or reverses one of these deserves a new ADR alongside the code.
- **A note on `issue #NN` in comments.** References predating 2026-07 point at the tracker this project
  used before it moved development into the open; they do **not** correspond to issue numbers in this
  repo. They are kept as provenance for the original design discussion — don't follow them here.
- **Architecture seams matter.** Apps plug into the kernel only through public seams (the
  `DetectionConsumer` trait, `Router<AppState>` merging, a self-installed schema, the auth primitive).
  Don't add app-specific knowledge to the kernel — see [ARCHITECTURE.md](./ARCHITECTURE.md).
- Match the surrounding code's style, comment density, and error-handling patterns.
- Keep commits focused; explain the *why* in the body.

## Pull requests

1. Fork + branch from `main`.
2. Make the change with tests; keep the quality bar green.
3. **Sign your work** (DCO): every commit needs a `Signed-off-by: Your Name <you@example.com>`
   trailer — `git commit -s` adds it. This certifies, per the
   [Developer Certificate of Origin](https://developercertificate.org), that you have the right to
   submit the change under Apache-2.0. CI enforces it; fix a missing sign-off with
   `git rebase --signoff origin/main`.
4. Open a PR describing the problem and the approach. Link any issue.
5. By contributing, you agree your contributions are licensed under **Apache-2.0** (the repo
   license). Heldar is open-core: the same Apache-2.0 code also ships inside the commercial
   builds, exactly as the license permits.

All participation is covered by the [Code of Conduct](./CODE_OF_CONDUCT.md).

## How your PR lands

This repository **is** the source of truth for the open platform — the kernel, the generic apps,
the reference AI worker and the dashboard are developed here, in the open. There is no mirror and
no import step: your commit, with your authorship, is the commit that ships.

1. You open a PR. CI runs the same quality bar maintainers run (`cargo fmt`/`clippy -D warnings`/
   tests, the open build, the dashboard build) plus the DCO sign-off check.
2. A maintainer reviews it here.
3. On merge it is on `main`, and goes out in the next release to crates.io / GitHub Releases /
   container images.

Proprietary vertical products (e.g. client-specific analytics apps) and the hosted relay live in
separate private repositories and consume the crates published from *this* repo through the
documented seams — they never reach back into it. If a change you need would touch something not
in this tree, open an issue describing the need and we will work out the seam.

## Reporting security issues

Please do **not** open public issues for vulnerabilities. Report them privately to the maintainers
(security contact in the repo settings). The disclosure process, response expectations, and scope
are in [SECURITY.md](./SECURITY.md). See the security posture in
[ARCHITECTURE.md](./ARCHITECTURE.md) and the auth/RBAC + remote-access docs.

## Docs

User/operator/architecture docs live in [`docs/`](./docs) and the top-level `ARCHITECTURE.md`,
`ROADMAP.md`, and `LICENSING.md`. Update the relevant doc in the same PR as a behavior change.
