## What & why

<!-- The problem this PR solves and the approach. Link any issue. -->

## Quality bar

CI checks all of these — running them locally first saves a round-trip (see
[CONTRIBUTING.md](../CONTRIBUTING.md)):

- [ ] `cargo fmt --all -- --check` — formatted
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings` — warning-free
- [ ] `cargo test --workspace --locked` — tests pass (new behavior has tests)
- [ ] `cargo build -p heldar-server --no-default-features --locked` — the OPEN build compiles
- [ ] `./scripts/check-read-seam.sh` — cross-app reads go through `*_read` views
- [ ] `cd apps/web && npm run typecheck && npm run build` — dashboard builds (if `apps/web` changed)
- [ ] Docs updated (README / ARCHITECTURE.md / CHANGELOG.md when the change is architectural)
