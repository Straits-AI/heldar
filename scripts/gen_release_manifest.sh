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
# Usage: gen_release_manifest.sh <version>
#   REQUIRE_DIGESTS=1  refuse to emit a manifest whose image digests did not resolve. Set this for a
#                      real release: an unpinned manifest defeats the point of having one.
set -euo pipefail
VERSION="${1:?usage: gen_release_manifest.sh <version>}"
REQUIRE_DIGESTS="${REQUIRE_DIGESTS:-0}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# The migration CEILING, read from the tree rather than hand-maintained: a number typed into a
# manifest is a number that goes stale on the next migration, and the failure is a box refusing to
# start (or worse, agreeing to start) on the wrong schema.
#
# The COMPONENT LIST is discovered the same way, and for the same reason. The first version of this
# named kernel and entry by hand; movement and search also carry schemas, so their migrations shipped
# outside the ceiling entirely — a routine feature release could move a schema this manifest did not
# describe and the verifier had nothing to check it against. A hardcoded list of components goes
# stale exactly the way a hardcoded number does.
max_migration() {
  local dir="$1"
  ls "$dir" 2>/dev/null | sed -n 's/^0*\([0-9]\{1,\}\)_.*\.sql$/\1/p' | sort -n | tail -1
}

MIGRATIONS=""
for d in crates/heldar-*/migrations; do
  [ -d "$d" ] || continue
  comp="$(basename "$(dirname "$d")")"; comp="${comp#heldar-}"
  m="$(max_migration "$d")"
  # A migrations directory we cannot read a version out of is a parser failure, not an empty release.
  [ -n "$m" ] || { echo "gen_release_manifest: no migration version parsed from $d" >&2; exit 1; }
  MIGRATIONS="${MIGRATIONS}    \"${comp}\": ${m},\n"
done
[ -n "$MIGRATIONS" ] || { echo "gen_release_manifest: found no migrations directories" >&2; exit 1; }
MIGRATIONS="${MIGRATIONS%,\\n}"

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
  elif [ "$REQUIRE_DIGESTS" = 1 ]; then
    # On a real release this is not "the registry was unreachable", it is almost always "the image
    # push has not finished yet" — the manifest job and the image build trigger on the same tag push
    # with no ordering between them. Emitting null there would ship an unpinned manifest for a
    # perfectly good release, which is worse than failing the job and re-running it.
    echo "gen_release_manifest: no digest for ${ref} and REQUIRE_DIGESTS=1" >&2
    exit 1
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
$(printf "%b" "$MIGRATIONS")
  },
  "components": {
$(printf "%b" "$COMPONENTS")
  },
  "artifacts": {
$(printf "%b" "$ARTIFACTS")
  }
}
EOF
