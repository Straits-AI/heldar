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
scripts/setup_mediamtx.sh        # fetch the MediaMTX live-view gateway
scripts/run_stack.sh             # MediaMTX + core (:8000) + web (Vite)
```

The per-stage `scripts/validate_*.sh` scripts exercise each capability end-to-end against a running
stack and write reports to `data/`.

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

## How your PR actually lands (read this — the repo is generated)

Honesty first: the public `Straits-AI/heldar` tree is **generated** from an internal monorepo
that also contains proprietary vertical products. The internal monorepo is the source of truth;
each release regenerates the open subset and commits it here as a snapshot on top of the existing
history (history, stars, links, and merged-PR commits are preserved — the snapshot supersedes by
content, never by erasing).

What that means for your PR:

1. You open a PR here; CI runs the same open-build quality bar maintainers use.
2. A maintainer reviews it **on the PR**, like any repo.
3. When accepted, it is **imported into the internal monorepo with your authorship preserved**
   (`git am` — your name and email stay on the commit) and the PR is merged here.
4. The next release's snapshot commit re-exports the same change from the source of truth; your
   merged commit remains in history and the release notes credit the PR.

The paths you see here are identical to the internal paths, so patches apply cleanly in both
directions. If a change you need touches something that is *not* in this tree (it will be obvious
— the seam is documented in `ARCHITECTURE.md`), open an issue describing the need instead.

## Reporting security issues

Please do **not** open public issues for vulnerabilities. Report them privately to the maintainers
(security contact in the repo settings). The disclosure process, response expectations, and scope
are in [SECURITY.md](./SECURITY.md). See the security posture in
[ARCHITECTURE.md](./ARCHITECTURE.md) and the auth/RBAC + remote-access docs.

## Docs

User/operator/architecture docs live in [`docs/`](./docs) and the top-level `ARCHITECTURE.md`,
`ROADMAP.md`, and `LICENSING.md`. Update the relevant doc in the same PR as a behavior change.
