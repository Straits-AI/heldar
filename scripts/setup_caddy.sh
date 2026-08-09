#!/usr/bin/env bash
# Download the Caddy binary into infra/caddy/ (gitignored).
#
# Only needed for the TLS end-to-end suite (scripts/e2e_tls_stack.sh), which terminates real HTTPS in
# front of the stack so the secure live-view path is exercised the way an operator deploys it. The
# production deployment uses the pinned `caddy` container instead (deploy/compose.tls.yml).
#
# Detects OS/arch (linux + darwin; amd64/arm64). Override the release with CADDY_VERSION=2.10.2.
set -euo pipefail
DEST="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/infra/caddy"
mkdir -p "$DEST"
cd "$DEST"

case "$(uname -s)" in
  Linux)  OS=linux ;;
  Darwin) OS=mac ;;
  *) echo "unsupported OS: $(uname -s) — install Caddy manually into $DEST" >&2; exit 1 ;;
esac

case "$(uname -m)" in
  x86_64|amd64)   ARCH=amd64 ;;
  aarch64|arm64)  ARCH=arm64 ;;
  *) echo "unsupported arch: $(uname -m) — install Caddy manually into $DEST" >&2; exit 1 ;;
esac

# Keep the default in step with the pinned image in deploy/compose.tls.yml so the suite tests the
# version that actually ships.
VERSION="${CADDY_VERSION:-2.10.2}"

if [ -x ./caddy ] && ./caddy version 2>/dev/null | grep -q "v${VERSION}"; then
  echo "Caddy v${VERSION} already present -> ${DEST}/caddy"
  exit 0
fi

echo "Installing Caddy v${VERSION} (${OS}/${ARCH}) -> ${DEST}/caddy"
curl -fsSL -o caddy.tar.gz \
  "https://github.com/caddyserver/caddy/releases/download/v${VERSION}/caddy_${VERSION}_${OS}_${ARCH}.tar.gz"
tar xzf caddy.tar.gz caddy
rm -f caddy.tar.gz
chmod +x caddy
./caddy version
