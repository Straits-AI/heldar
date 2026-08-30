#!/usr/bin/env bash
# Download the MediaMTX binary into infra/mediamtx/ (gitignored).
#
# Detects OS/arch (linux + darwin; amd64/arm64/armv7/armv6) so it works on a dev Mac, an x86 server,
# and an arm64 appliance alike. Override the release with MEDIAMTX_TAG=v1.2.3.
set -euo pipefail
DEST="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/infra/mediamtx"
mkdir -p "$DEST"
cd "$DEST"

case "$(uname -s)" in
  Linux)  OS=linux ;;
  Darwin) OS=darwin ;;
  *) echo "unsupported OS: $(uname -s) — install MediaMTX manually into $DEST" >&2; exit 1 ;;
esac

case "$(uname -m)" in
  x86_64|amd64)   ARCH=amd64 ;;
  aarch64|arm64)  ARCH=arm64 ;;
  armv7l)         ARCH=armv7 ;;
  armv6l)         ARCH=armv6 ;;
  *) echo "unsupported arch: $(uname -m) — install MediaMTX manually into $DEST" >&2; exit 1 ;;
esac
# Upstream ships no darwin/arm-32 builds.
if [ "$OS" = darwin ] && [ "$ARCH" != amd64 ] && [ "$ARCH" != arm64 ]; then
  echo "no macOS build for $ARCH upstream" >&2; exit 1
fi

# The unauthenticated GitHub API allows 60 requests/hour PER IP, and CI runners share IPs — so this
# call returns 403 on a busy morning and every retry gets the same 403, because the limit is not
# transient. A token raises it to 1000/hour for the repository. Optional: locally there is usually no
# token and 60/hour is plenty.
GH_AUTH=()
if [ -n "${GITHUB_TOKEN:-}" ]; then
  GH_AUTH=(-H "Authorization: Bearer ${GITHUB_TOKEN}")
fi

# Resolve the latest tag with a portable parser (`grep -oP` is GNU-only and fails on macOS/BSD).
TAG="${MEDIAMTX_TAG:-$(curl -fsSL --retry 5 --retry-delay 3 --retry-all-errors "${GH_AUTH[@]}" https://api.github.com/repos/bluenviron/mediamtx/releases/latest \
  | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)}"
[ -n "$TAG" ] || { echo "could not resolve the latest MediaMTX tag (rate-limited?) — set MEDIAMTX_TAG to pin one, or export GITHUB_TOKEN to raise the API limit" >&2; exit 1; }

# Retried: the GitHub release CDN returns transient 503s, which have failed CI and blocked a
# release twice. `--retry-all-errors` is what makes curl retry an HTTP error rather than only a
# connection failure.
echo "Installing MediaMTX ${TAG} (${OS}/${ARCH}) -> ${DEST}/mediamtx"
curl -fsSL --retry 5 --retry-delay 3 --retry-all-errors -o mediamtx.tar.gz \
  "https://github.com/bluenviron/mediamtx/releases/download/${TAG}/mediamtx_${TAG}_${OS}_${ARCH}.tar.gz"
tar xzf mediamtx.tar.gz mediamtx
rm -f mediamtx.tar.gz
chmod +x mediamtx
./mediamtx --version
