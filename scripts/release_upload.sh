#!/usr/bin/env bash
# Upload a release asset, refusing to REPLACE one whose bytes differ (#115).
#
# Usage: scripts/release_upload.sh <tag> <file> [<file> ...]
#
# Every upload in release.yml used `gh release upload --clobber`, which is exactly the thing #115's
# criterion forbids: "re-running a release for an existing tag cannot silently replace artifacts
# with different digests". A re-run of a release workflow — after a flake, or a retried job — would
# quietly publish different bytes under a tag consumers have already pinned, while any attestation
# or checksum they cached still describes the old ones.
#
# `--clobber` is still used, because a re-run that uploads IDENTICAL bytes must stay idempotent: a
# release workflow that cannot be re-run after an infrastructure failure is its own hazard. What
# changes is that differing bytes are refused rather than published.
#
# Fails closed: if the existing asset cannot be fetched to compare, that is a refusal, not a pass.
set -euo pipefail

TAG="${1:?usage: release_upload.sh <tag> <file> [<file> ...]}"
shift

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

for file in "$@"; do
  [ -f "$file" ] || { echo "::error::$file does not exist"; exit 1; }
  name="$(basename "$file")"
  local_sha="$(sha256sum "$file" | cut -d' ' -f1)"

  # Is an asset by this name already published under the tag?
  if gh release view "$TAG" --json assets --jq '.assets[].name' 2>/dev/null | grep -qxF "$name"; then
    if ! gh release download "$TAG" --pattern "$name" --dir "$work" --clobber 2>/dev/null; then
      echo "::error::$name is already published under $TAG but could not be downloaded to compare."
      echo "::error::Refusing to overwrite bytes that cannot be checked."
      exit 1
    fi
    published_sha="$(sha256sum "$work/$name" | cut -d' ' -f1)"
    if [ "$published_sha" != "$local_sha" ]; then
      echo "::error::$name is already published under $TAG with DIFFERENT bytes."
      echo "::error::  published: $published_sha"
      echo "::error::  building:  $local_sha"
      echo "::error::A tag is immutable to everyone who already pinned it. Cut a new version rather"
      echo "::error::than replacing the artifact under this one."
      exit 1
    fi
    echo "$name: already published with identical bytes ($local_sha) — re-upload is a no-op"
    rm -f "$work/$name"
  fi

  gh release upload "$TAG" "$file" --clobber
  echo "$name: uploaded ($local_sha)"
done
