#!/usr/bin/env bash
# One-command Heldar release: bump → verify → regenerate open tree → (force-push + tag → OIDC publish).
#
# The slow parts of a release are all *verification* (fresh compiles, the open-tree dry-run); this script
# runs them unattended and back-to-back instead of across interactive round-trips. The only irreversible
# steps (force-push public main, push the version tag that triggers crates.io publishing) are gated behind
# `--publish` — without it the script does every local + dry-run step and then STOPS, leaving the bump
# committed and the open tree staged for inspection.
#
# Usage:
#   scripts/release.sh X.Y.Z                 # full local prep + verify + dry-run, stop before any push
#   scripts/release.sh X.Y.Z --publish       # ...then force-push main + tag vX.Y.Z + poll crates.io
#   scripts/release.sh X.Y.Z --skip-gate     # skip the private clippy/test gate (rely on CI; faster)
#
# Preconditions you own (the script refuses otherwise):
#   * clean working tree
#   * CHANGELOG.md already has a "## [X.Y.Z]" section (you write the human-facing notes)
#   * X.Y.Z is not already on crates.io
#
# Optional secret sweep: set HELDAR_RELEASE_DENYLIST to a file (OUTSIDE the repo) of regex patterns, one
# per line — real credential strings, internal hostnames, etc. The script fails if any match the generated
# open tree. This generalizes the "a real password ended up in a test fixture" class of leak. Keep that
# file out of the repo (scripts/ ships to the public repo).
set -euo pipefail

# ---- args ----
VERSION="${1:-}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "usage: scripts/release.sh X.Y.Z [--publish] [--skip-gate]" >&2; exit 2; }
shift
PUBLISH=0; SKIP_GATE=0
for arg in "$@"; do
  case "$arg" in
    --publish)   PUBLISH=1 ;;
    --skip-gate) SKIP_GATE=1 ;;
    *) echo "unknown flag: $arg" >&2; exit 2 ;;
  esac
done

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PUB="${HELDAR_OPEN_DIR:-$(dirname "$ROOT")/heldar}"
PUBLIC_REMOTE="${HELDAR_PUBLIC_REMOTE:-git@github.com:Straits-AI/heldar.git}"
CRATES=(heldar-kernel heldar-entry heldar-movement heldar-search)   # the published libs, dep order
APPS=(entry movement search)                                        # carry a heldar-kernel version pin
TAG="v$VERSION"

say() { printf '\n\033[1m== %s ==\033[0m\n' "$*"; }
die() { echo "ERROR: $*" >&2; exit 1; }

# ---- 0. preconditions ----
say "preconditions (v$VERSION, publish=$PUBLISH)"
cd "$ROOT"
# Require a pristine tree (incl. untracked) — step 3 does `git add -A`, so anything lying around would
# otherwise be swept into the release commit. Commit your tooling (this script included) first.
[ -z "$(git status --porcelain)" ] || die "working tree not clean (tracked or untracked changes) — commit or stash first"
grep -q "## \[$VERSION\]" CHANGELOG.md || die "CHANGELOG.md has no '## [$VERSION]' section — write the notes first"
for c in "${CRATES[@]}"; do
  code=$(curl -fsS -A heldar-ci -o /dev/null -w '%{http_code}' "https://crates.io/api/v1/crates/$c/$VERSION" 2>/dev/null || true)
  [ "$code" = "200" ] && die "$c@$VERSION is already on crates.io (versions are immutable — bump again)"
done
echo "  clean tree, changelog has [$VERSION], crates.io has no $VERSION yet"

# ---- 1. bump the 4 crate versions + the kernel dep pins (idempotent) ----
say "bump → $VERSION"
CUR=$(sed -nE 's/^version = "([^"]+)"/\1/p' crates/heldar-kernel/Cargo.toml | head -1)
if [ "$CUR" != "$VERSION" ]; then
  CUR_RE=${CUR//./\\.}
  for c in "${CRATES[@]}"; do
    sed -i -E "s/^version = \"$CUR_RE\"/version = \"$VERSION\"/" "crates/$c/Cargo.toml"
  done
  for c in "${APPS[@]}"; do
    sed -i -E "s#(heldar-kernel = \{ path = \"\.\./heldar-kernel\", version = )\"$CUR_RE\"#\1\"$VERSION\"#" "crates/heldar-$c/Cargo.toml"
  done
  echo "  bumped from $CUR"
else
  echo "  already at $VERSION (skipping bump)"
fi
# verify: every published crate + pin reads $VERSION, none still at the old one
for c in "${CRATES[@]}"; do
  grep -q "^version = \"$VERSION\"" "crates/$c/Cargo.toml" || die "$c version not at $VERSION"
done
for c in "${APPS[@]}"; do
  grep -q "version = \"$VERSION\" }" "crates/heldar-$c/Cargo.toml" || \
    grep -q "\"../heldar-kernel\", version = \"$VERSION\"" "crates/heldar-$c/Cargo.toml" || die "heldar-$c kernel pin not at $VERSION"
done

# ---- 2. local verification gate (the CLAUDE.md gate; skippable) ----
say "private verify gate"
cargo build --workspace >/dev/null            # also refreshes Cargo.lock to the new versions
cargo fmt --all -- --check
if [ "$SKIP_GATE" = 0 ]; then
  cargo clippy --workspace --all-targets --locked -- -D warnings
  cargo test --workspace --locked >/dev/null
  cargo build -p heldar-server --no-default-features --locked >/dev/null   # the OPEN build must compile
  echo "  fmt + clippy + test + open-build: clean"
else
  echo "  --skip-gate: ran fmt + build only (clippy/test left to CI)"
fi

# ---- 3. commit the release prep (only if anything changed) ----
say "commit"
if ! git diff --quiet || ! git diff --cached --quiet; then
  git add -A
  git commit -q -m "release: v$VERSION (kernel + entry/movement/search)"
  echo "  committed $(git rev-parse --short HEAD)"
else
  echo "  nothing to commit (already prepared)"
fi

# ---- 4. regenerate the public open tree (runs its own leak/forbidden-token gates) ----
say "regenerate open tree → $PUB"
rm -rf "$PUB"
"$ROOT/scripts/prepare-open-repo.sh" "$PUB" >/tmp/heldar-release-prep.log 2>&1 || { tail -30 /tmp/heldar-release-prep.log; die "prepare-open-repo.sh failed"; }
grep -q "forbidden-token gate: clean" /tmp/heldar-release-prep.log || die "open-tree gates did not report clean (see /tmp/heldar-release-prep.log)"
echo "  open tree staged; built-in proprietary-code + forbidden-token gates clean"

# ---- 5. secret denylist sweep on the generated tree (opt-in via env) ----
say "secret sweep"
if [ -n "${HELDAR_RELEASE_DENYLIST:-}" ] && [ -f "$HELDAR_RELEASE_DENYLIST" ]; then
  hits=0
  while IFS= read -r pat; do
    [ -z "$pat" ] && continue
    case "$pat" in \#*) continue ;; esac
    if ( cd "$PUB" && git grep -InE "$pat" -- $(git ls-files) >/dev/null 2>&1 ); then
      echo "  ::leak:: pattern matched in open tree: $pat"; hits=1
    fi
  done < "$HELDAR_RELEASE_DENYLIST"
  [ "$hits" = 0 ] || die "denylist pattern(s) found in the open tree — fix before publishing"
  echo "  denylist ($HELDAR_RELEASE_DENYLIST): clean"
else
  echo "  no HELDAR_RELEASE_DENYLIST set — skipping (recommended: a file of your real cred strings, OUTSIDE the repo)"
fi

# ---- 6. build-verify the open tree + kernel publish dry-run (what the release CI does) ----
say "open-tree build + publish dry-run"
( cd "$PUB"
  cargo build --workspace --locked >/dev/null
  cargo publish -p heldar-kernel --locked --dry-run >/dev/null
)
echo "  open workspace builds locked; heldar-kernel $VERSION packages + verifies"

# ---- 7. publish (gated) ----
if [ "$PUBLISH" = 0 ]; then
  say "STOPPING before push (no --publish)"
  cat <<EOF
  Everything local is verified and staged. To publish, either re-run with --publish, or run:
    git push origin HEAD                              # private monorepo
    cd '$PUB'
    git remote add origin $PUBLIC_REMOTE 2>/dev/null || git remote set-url origin $PUBLIC_REMOTE
    git fetch origin --tags
    git push --force origin HEAD:main                 # replace public main with the fresh squash
    git tag -a $TAG -m "Heldar $TAG" && git push origin $TAG   # triggers release.yml → OIDC publish
EOF
  exit 0
fi

say "PUBLISH"
git push origin HEAD                                  # private monorepo keeps the release commit
cd "$PUB"
git remote add origin "$PUBLIC_REMOTE" 2>/dev/null || git remote set-url origin "$PUBLIC_REMOTE"
git fetch origin --tags >/dev/null 2>&1 || true
git ls-remote --tags origin 2>/dev/null | grep -q "refs/tags/$TAG" && die "$TAG already exists on the public remote"
git push --force origin HEAD:main
git tag -a "$TAG" -m "Heldar $TAG"
git push origin "$TAG"
echo "  pushed public main + $TAG — release.yml is now publishing via OIDC"

# ---- 8. wait for crates.io ----
say "waiting for crates.io"
for i in $(seq 1 20); do
  live=0
  for c in "${CRATES[@]}"; do
    code=$(curl -fsS -A heldar-ci -o /dev/null -w '%{http_code}' "https://crates.io/api/v1/crates/$c/$VERSION" 2>/dev/null || true)
    [ "$code" = "200" ] && live=$((live+1))
  done
  echo "  $live/4 crates live at $VERSION"
  [ "$live" = 4 ] && { say "DONE — v$VERSION is live"; exit 0; }
  sleep 30
done
die "timed out waiting for all 4 crates (check the Release workflow run)"
