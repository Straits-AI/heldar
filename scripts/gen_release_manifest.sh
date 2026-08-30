#!/usr/bin/env bash
# Emit the immutable release manifest for one tag (#112).
#
# The quickstart deliberately floats `latest` and pulls deployment files from main. That is right for
# evaluation and wrong for a recorder: without one document pinning them together, an operator can end
# up running compose files from one commit, a core image from another release, and a web image that
# expects a different API — and `docker compose pull` can change a production recorder on restart.
#
# This is that document. It states, for one tag: the source commit, the migration ceiling the binaries
# in this release actually carry, the exact image digests, and the hashes of every deployment file.
# A verifier can then refuse any combination that is not this one.
#
# Usage: gen_release_manifest.sh <version>   (digests resolved from ghcr when reachable)
set -euo pipefail
VERSION="${1:?usage: gen_release_manifest.sh <version>}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# The migration CEILING, read from the tree rather than hand-maintained: a number typed into a
# manifest is a number that goes stale on the next migration, and the failure is a box refusing to
# start (or worse, agreeing to start) on the wrong schema.
max_migration() {
  local dir="$1"
  ls "$dir" 2>/dev/null | sed -n 's/^0*\([0-9]\{1,\}\)_.*\.sql$/\1/p' | sort -n | tail -1
}
KERNEL_MAX="$(max_migration crates/heldar-kernel/migrations)"
ENTRY_MAX="$(max_migration crates/heldar-entry/migrations)"

sha() { sha256sum "$1" 2>/dev/null | cut -d' ' -f1; }

# Best-effort: a digest needs the registry. Absent one the field is null rather than a guess — a
# manifest that invents a digest is worse than one that admits it does not have it.
digest_of() {
  local ref="$1"
  if command -v docker >/dev/null 2>&1; then
    docker buildx imagetools inspect "$ref" --format '{{json .Manifest.Digest}}' 2>/dev/null \
      | tr -d '"' || true
  fi
}

GIT_SHA="$(git rev-parse HEAD)"
ARTIFACTS=""
for f in deploy/compose.yml deploy/compose.prod.yml deploy/compose.hardened.yml deploy/compose.tls.yml deploy/mediamtx.yml; do
  [ -f "$f" ] || continue
  ARTIFACTS="${ARTIFACTS}    \"$(basename "$f")\": {\"sha256\": \"$(sha "$f")\"},\n"
done
ARTIFACTS="${ARTIFACTS%,\\n}"

COMPONENTS=""
for name in core web ai; do
  ref="ghcr.io/straits-ai/heldar-${name}:${VERSION}"
  d="$(digest_of "$ref")"
  if [ -n "$d" ]; then
    COMPONENTS="${COMPONENTS}    \"${name}\": {\"image\": \"ghcr.io/straits-ai/heldar-${name}\", \"digest\": \"${d}\"},\n"
  else
    COMPONENTS="${COMPONENTS}    \"${name}\": {\"image\": \"ghcr.io/straits-ai/heldar-${name}\", \"digest\": null},\n"
  fi
done
COMPONENTS="${COMPONENTS%,\\n}"

cat <<EOF
{
  "schema": 1,
  "heldar_version": "${VERSION}",
  "git_sha": "${GIT_SHA}",
  "migrations": {
    "kernel_max": ${KERNEL_MAX},
    "entry_max": ${ENTRY_MAX}
  },
  "components": {
$(printf "%b" "$COMPONENTS")
  },
  "artifacts": {
$(printf "%b" "$ARTIFACTS")
  }
}
EOF
