# Contributing to Heldar

Thanks for your interest. Heldar is the **open core** of a visual event-intelligence platform: an
Apache-2.0 media/perception kernel plus generic reference apps. This repo is the place to improve the
*platform* and the *generic* apps — vertical/client-specific products live in a separate private repo
and are out of scope here.

## What belongs in this repo

| In scope (Apache-2.0) | Out of scope (proprietary, elsewhere) |
| --- | --- |
| `heldar-kernel` — media/DVR, perception ingest + sampler, zone engine, auth/RBAC, retention, remote-access awareness, worker SDK | Vertical/client products (retail analytics, the campus suite, etc.) |
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
cargo build --workspace
cargo clippy --workspace --all-targets   # must be warning-free
cargo test --workspace
cargo fmt --all -- --check                # formatted

# the OPEN reference build must link zero proprietary code:
cargo build -p heldar-server --no-default-features

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
3. Open a PR describing the problem and the approach. Link any issue.
4. By contributing, you agree your contributions are licensed under **Apache-2.0** (the repo license).

## Reporting security issues

Please do **not** open public issues for vulnerabilities. Report them privately to the maintainers
(security contact in the repo settings). See the security posture in
[ARCHITECTURE.md](./ARCHITECTURE.md) and the auth/RBAC + remote-access docs.

## Docs

User/operator/architecture docs live in [`docs/`](./docs) and the top-level `ARCHITECTURE.md`,
`ROADMAP.md`, and `LICENSING.md`. Update the relevant doc in the same PR as a behavior change.
