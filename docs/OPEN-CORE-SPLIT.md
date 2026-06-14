# Open-core split & publishing runbook

How to take this single repo to the agreed **polyrepo** layout and publish the open crates. The
file-producing parts are scripted and reversible; the **outward-facing, irreversible** parts (create
a public GitHub repo, `cargo publish`) are yours to run — they need your GitHub org + a crates.io
token, and a published crate version can only be *yanked*, never deleted.

## Target

| Repo | Visibility | Contents | License |
|---|---|---|---|
| **`heldar`** | **public** | kernel + generic apps (entry/movement/search) + reference server + reference AI worker + reference web + docs/infra/scripts | Apache-2.0 |
| **`heldar-proprietary`** | **private** | proprietary verticals (`heldar-bakery`; future `heldar-campus-*`), production bins, business docs (`memo.md`, `research.md`), deploy/secrets | Proprietary |

The private workspace depends on the open crates via **crates.io** (or a git tag pre-publish), with a
`[patch.crates-io]` pointing at a local checkout for side-by-side development.

## What is open vs private

The open allowlist is encoded in `scripts/prepare-open-repo.sh` (`OPEN_PATHS`). Explicitly **private**
(never copied): `crates/heldar-bakery`, `crates/heldar-campus-*`, `memo.md`, `research.md`,
`auto.crt`/`auto.key` (already gitignored), `data/`, `target/`. Scrubbed from the copy:
`docs/BAKERYSENSE.md`, `scripts/validate_bakery.sh`, `apps/ai/*.pt` (model weights — download, don't
vendor), `apps/web/node_modules`, `__pycache__`.

## Decisions / inputs you provide

- **GitHub org/owner** — the crate metadata uses `https://github.com/Straits-AI/heldar`. Change it in
  each open `Cargo.toml` (`repository = …`) if that's not your public namespace.
- **crates.io token** — `cargo login` once (https://crates.io/me).
- **Web vertical page** — `apps/web/src/pages/Bakery.tsx` (+ its nav/route in `App.tsx`/`AppShell.tsx`
  and the bakery bits of `lib/api.ts`/`lib/types.ts`) is a vertical UI. Strip it from the open repo if
  you don't want a dead page there. (The Rust build is unaffected either way.)
- **README** — `README.md` still describes "Stage 0"; refresh it for the public face before pushing.
- **Model weights** — `apps/ai` references `yolov8n.pt` (Ultralytics AGPL). The script drops the
  weight file; document the download step in `apps/ai/README.md` instead of committing it.

## Steps

### 0. Pre-flight (in this repo)
```bash
cargo build --workspace && cargo clippy --workspace --all-targets && cargo test --workspace
cargo build -p heldar-server --no-default-features   # open reference server links no proprietary crate
```

### 1. This repo becomes the private `heldar-proprietary`
It already holds the full history (open + proprietary), which is fine for a *private* repo. Point its
remote at the private repo (create it first on GitHub, private):
```bash
gh repo create Straits-AI/heldar-proprietary --private --source=. --remote=origin --push
```

### 2. Produce the open subset
```bash
scripts/prepare-open-repo.sh ../heldar      # copies the allowlist, rewrites manifests, swaps in
                                               # the no-op verticals stub, inits a fresh squashed repo
cd ../heldar && cargo build --workspace     # MUST build with zero proprietary deps
```
The script overwrites the workspace `Cargo.toml` (drops the bakery member) and
`crates/heldar-server/{Cargo.toml,src/verticals.rs}` (drops the optional bakery dep + the
`verticals` feature; installs the no-op `verticals` stub). `main.rs` is byte-identical to this repo —
all proprietary references live only in the private `verticals.rs`.

### 3. Review the open subset
Strip `apps/web/src/pages/Bakery.tsx` if unwanted (step decision above); refresh `README.md`; confirm
`git -C ../heldar ls-files | grep -Ei 'bakery|campus|memo|research|auto\.key'` returns only benign
doc mentions (no proprietary source).

### 4. Create the public repo
```bash
cd ../heldar
gh repo create Straits-AI/heldar --public --source=. --remote=origin --push
```

### 5. Publish the open crates to crates.io (dependency order)
Libraries only — the server bin and bakery are `publish = false`. Publish the kernel first (the others
depend on it); wait for each to land in the index before the next.
```bash
cd ../heldar
cargo publish -p heldar-kernel --dry-run     # verify (already passes here)
cargo publish -p heldar-kernel
# after it appears on crates.io:
cargo publish -p heldar-entry
cargo publish -p heldar-movement
cargo publish -p heldar-search
```

### 6. Wire the private repo to the published crates
In `heldar-proprietary`, future vertical crates depend on the registry versions, with a local patch
for development:
```toml
# heldar-campus-entry/Cargo.toml
heldar-kernel = "0.1"
heldar-entry  = "0.1"

# heldar-proprietary workspace Cargo.toml — local dev override (not committed to releases)
[patch.crates-io]
heldar-kernel = { path = "../heldar/crates/heldar-kernel" }
heldar-entry  = { path = "../heldar/crates/heldar-entry" }
heldar-movement = { path = "../heldar/crates/heldar-movement" }
heldar-search = { path = "../heldar/crates/heldar-search" }
```
Until the Campus verticals exist, `heldar-proprietary` is just this repo (path deps, dev convenience);
the registry-dep separation matters once you actively build verticals against a published kernel.

## Re-publishing later
Bump the crate `version`, `cargo publish` again (in dep order). Breaking changes to the kernel seams
(`DetectionConsumer`, the router/schema contracts) are a **major** bump that verticals opt into.
