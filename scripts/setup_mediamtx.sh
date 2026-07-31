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

# Resolve the latest tag with a portable parser (`grep -oP` is GNU-only and fails on macOS/BSD).
TAG="${MEDIAMTX_TAG:-$(curl -fsSL https://api.github.com/repos/bluenviron/mediamtx/releases/latest \
  | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)}"
[ -n "$TAG" ] || { echo "could not resolve the latest MediaMTX tag (rate-limited?) — retry or set MEDIAMTX_TAG" >&2; exit 1; }

echo "Installing MediaMTX ${TAG} (${OS}/${ARCH}) -> ${DEST}/mediamtx"
curl -fsSL -o mediamtx.tar.gz \
  "https://github.com/bluenviron/mediamtx/releases/download/${TAG}/mediamtx_${TAG}_${OS}_${ARCH}.tar.gz"
tar xzf mediamtx.tar.gz mediamtx
rm -f mediamtx.tar.gz
chmod +x mediamtx
./mediamtx --version
