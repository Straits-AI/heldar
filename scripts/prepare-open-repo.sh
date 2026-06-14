#!/usr/bin/env bash
# Produce the OPEN subset of Heldar as a fresh, clean git repo — the public `heldar` repo.
#
# Open-core split (see docs/OPEN-CORE-SPLIT.md): the current repo stays PRIVATE (full history,
# proprietary verticals) as `heldar-proprietary`; this script copies ONLY the Apache-2.0 open subset
# into a NEW directory with a single squashed initial commit, so no proprietary code or history leaks.
#
# This script is LOCAL and reversible: it only writes a new directory. It does NOT create a GitHub
# repo, push, or publish — you do those (see the runbook). Re-runnable: refuses a non-empty target.
#
# Usage:  scripts/prepare-open-repo.sh [TARGET_DIR]   (default: ../heldar)
set -euo pipefail

SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${1:-$(dirname "$SRC")/heldar}"

if [ -e "$TARGET" ] && [ -n "$(ls -A "$TARGET" 2>/dev/null || true)" ]; then
  echo "ERROR: target '$TARGET' exists and is non-empty. Remove it or pass a fresh path." >&2
  exit 1
fi
echo "Source (private): $SRC"
echo "Target (public):  $TARGET"
mkdir -p "$TARGET"

# --- The OPEN allowlist (Apache-2.0). Anything not listed here does NOT go public. ---
OPEN_PATHS=(
  # Open Rust workspace crates
  crates/heldar-kernel
  crates/heldar-entry
  crates/heldar-movement
  crates/heldar-search
  crates/heldar-server
  # Open reference AI worker (model weights excluded below) + reference dashboard
  apps/ai
  apps/web
  # Docs / infra / scripts / root files
  docs
  infra
  scripts
  .github
  ARCHITECTURE.md
  ROADMAP.md
  README.md
  CONTRIBUTING.md
  LICENSE
  LICENSING.md
  .env.example
  .gitignore
  Makefile
  Dockerfile
  docker-compose.yml
  Cargo.lock
)

# Explicitly PRIVATE — never copied (proprietary, sensitive, or business-internal):
#   crates/heldar-bakery, crates/heldar-campus-*   (proprietary verticals)
#   memo.md, research.md                                  (business strategy — add manually if desired)
#   auto.crt, auto.key, data/, target/                    (secrets / gitignored / build output)

echo "== copying open subset =="
for p in "${OPEN_PATHS[@]}"; do
  if [ -e "$SRC/$p" ]; then
    mkdir -p "$TARGET/$(dirname "$p")"
    cp -a "$SRC/$p" "$TARGET/$(dirname "$p")/"
    echo "  + $p"
  else
    echo "  ? skip (absent): $p"
  fi
done

echo "== scrubbing proprietary / heavy / generated leftovers from the copy =="
rm -rf "$TARGET/crates/heldar-bakery"                 # belt-and-braces (not in allowlist anyway)
rm -f  "$TARGET/docs/BAKERYSENSE.md"                     # vertical doc -> private
rm -f  "$TARGET/scripts/validate_bakery.sh"              # vertical validation -> private
# Strip the BakerySense (proprietary vertical) page + all its wiring from the open dashboard. The
# generic pages (Entry/Movement/Search) stay; only the bakery vertical's client code is removed.
# Anchored on stable section markers + col-0 braces; keeps the web app compiling.
WEB="$TARGET/apps/web/src"
rm -f "$WEB/pages/Bakery.tsx"
sed -i '/from "\.\/pages\/Bakery"/d; /path="\/bakery"/d' "$WEB/App.tsx" 2>/dev/null || true
sed -i '/to: "\/bakery"/d; /^function BakeryIcon/,/^}/d' "$WEB/components/AppShell.tsx" 2>/dev/null || true
sed -i 's#mirrors Entry.tsx / Bakery.tsx#mirrors Entry.tsx#' "$WEB/pages/Movement.tsx" 2>/dev/null || true
# api.ts: drop the bakery type imports + the BakerySense methods block (marker..next-section).
sed -i '/^  BakeryObservation,$/d; /^  BakeryReport,$/d; /^  BakerySummary,$/d' "$WEB/lib/api.ts" 2>/dev/null || true
sed -i '/\/\/ ---- BakerySense (Stage 5) ----/,/\/\/ ---- Movement intelligence (Stage 6) ----/{/\/\/ ---- Movement intelligence (Stage 6) ----/!d}' "$WEB/lib/api.ts" 2>/dev/null || true
# types.ts: drop the Bakery* interface block (marker..next-section).
sed -i '/\/\/ ---- Stage 5: BakerySense/,/\/\/ ---- Stage 6: Movement intelligence/{/\/\/ ---- Stage 6: Movement intelligence/!d}' "$WEB/lib/types.ts" 2>/dev/null || true
# .env.example: drop the BakerySense (proprietary vertical) tunables block (marker..next-section).
# The open build has no bakery crate, so HELDAR_BAKERY_* vars are dead here and reveal vertical config.
sed -i '/^# ---- Stage 5: BakerySense/,/^# ---- Stage 6: Movement intelligence/{/^# ---- Stage 6: Movement intelligence/!d}' "$TARGET/.env.example" 2>/dev/null || true
rm -rf "$TARGET/apps/ai/__pycache__"                     # python build artifact
find "$TARGET/apps/ai" -maxdepth 1 -name '*.pt' -delete 2>/dev/null || true  # model weights: download, don't vendor
rm -rf "$TARGET/apps/web/node_modules" "$TARGET/apps/web/dist"  # heavy/gitignored — pointless to copy
find "$TARGET" -type d -name 'target' -prune -exec rm -rf {} + 2>/dev/null || true
find "$TARGET" -name '.DS_Store' -delete 2>/dev/null || true

# --- Rewrite the workspace + server manifests to drop the proprietary vertical ---
echo "== writing open workspace Cargo.toml =="
cat > "$TARGET/Cargo.toml" <<'TOML'
# Heldar — open-core kernel + generic reference apps (Apache-2.0).
#
# `heldar-kernel` is the domain-agnostic platform (media/DVR, perception ingest + sampler, zone
# engine, auth, observability, retention, remote-access overlay awareness, worker SDK contract).
# `heldar-entry` (access control), `heldar-movement`, and `heldar-search` are generic
# reference apps. `heldar-server` composes them into a runnable deployment. Proprietary vertical
# products depend on these crates in a separate (private) workspace.
[workspace]
resolver = "2"
members = [
    "crates/heldar-kernel",
    "crates/heldar-entry",
    "crates/heldar-movement",
    "crates/heldar-search",
    "crates/heldar-server",
]

[profile.dev]
opt-level = 0

[profile.release]
opt-level = 2
TOML

echo "== writing open server Cargo.toml (no proprietary deps) =="
cat > "$TARGET/crates/heldar-server/Cargo.toml" <<'TOML'
[package]
name = "heldar-server"
version = "0.1.0"
edition = "2021"
rust-version = "1.85"
description = "Heldar composing server: links the open kernel with the generic apps (entry/movement/search) into a runnable deployment."
license = "Apache-2.0"
publish = false  # composition bin, not a library

[[bin]]
name = "heldar-core"
path = "src/main.rs"

[dependencies]
heldar-kernel = { path = "../heldar-kernel", version = "0.1.0" }
heldar-entry = { path = "../heldar-entry", version = "0.1.0" }
heldar-movement = { path = "../heldar-movement", version = "0.1.0" }
heldar-search = { path = "../heldar-search", version = "0.1.0" }
tokio = { version = "1", features = ["full"] }
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio", "sqlite"] }
axum = { version = "0.8", features = ["macros"] }
tower-http = { version = "0.6", features = ["cors", "trace", "fs"] }
chrono = { version = "0.4", features = ["serde"] }
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
anyhow = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
dotenvy = "0.15"
TOML

echo "== writing open verticals.rs stub (no proprietary references) =="
cat > "$TARGET/crates/heldar-server/src/verticals.rs" <<'RS'
//! Proprietary vertical composition seam — a NO-OP stub in the open repo.
//!
//! `main.rs` calls these functions unconditionally. In the open build they do nothing and reference
//! no proprietary crate. The private workspace replaces this file with the real composition module
//! (BakerySense, the Campus suite, …) — `main.rs` is identical across both repos.

use axum::Router;
use sqlx::SqlitePool;
use heldar_kernel::state::AppState;

pub async fn init_schema(_pool: &SqlitePool) -> anyhow::Result<()> {
    Ok(())
}

pub fn spawn_loops(_pool: &SqlitePool) {}

pub fn merge_routes(router: Router<AppState>) -> Router<AppState> {
    router
}
RS

echo "== regenerating Cargo.lock for the open workspace (drops the bakery entry) =="
# The copied lock still references the proprietary bakery crate, which isn't a member here; regenerate
# so `cargo build/publish --locked` in CI matches the 5-crate open workspace.
( cd "$TARGET" && cargo generate-lockfile >/dev/null 2>&1 ) || echo "  (cargo generate-lockfile skipped/failed — run 'cargo build' once in the open repo)"

echo "== initializing fresh git repo (single squashed commit) =="
( cd "$TARGET"
  git init -q
  git add -A
  git -c user.name="${GIT_AUTHOR_NAME:-Heldar}" -c user.email="${GIT_AUTHOR_EMAIL:-noreply@heldar.dev}" \
      commit -q -m "Heldar open core (Apache-2.0): kernel + generic apps

Open-core release: the domain-agnostic media/perception kernel plus the generic
reference apps (access control, movement intelligence, semantic search) and the
composing server. Apache-2.0. Proprietary vertical products live separately."
)

echo
echo "DONE. Open repo staged at: $TARGET"
echo "Next (you run these — see docs/OPEN-CORE-SPLIT.md):"
echo "  1. cd '$TARGET' && cargo build --workspace   # sanity check"
echo "  2. review apps/web/src/pages/Bakery.tsx (vertical page — strip if unwanted)"
echo "  3. gh repo create Straits-AI/heldar --public --source=. --remote=origin --push"
echo "  4. publish crates (kernel first):  see the runbook's publish sequence"
