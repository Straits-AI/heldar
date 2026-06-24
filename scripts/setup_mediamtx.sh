#!/usr/bin/env bash
# Download the MediaMTX binary into infra/mediamtx/ (gitignored).
set -euo pipefail
DEST="$(cd "$(dirname "$0")/.." && pwd)/infra/mediamtx"
cd "$DEST"
TAG="$(curl -fsSL https://api.github.com/repos/bluenviron/mediamtx/releases/latest \
  | grep -oP '"tag_name":\s*"\K[^"]+')"
echo "Installing MediaMTX ${TAG} -> ${DEST}/mediamtx"
curl -fsSL -o mediamtx.tar.gz \
  "https://github.com/bluenviron/mediamtx/releases/download/${TAG}/mediamtx_${TAG}_linux_amd64.tar.gz"
tar xzf mediamtx.tar.gz mediamtx
rm -f mediamtx.tar.gz
./mediamtx --version
