#!/usr/bin/env bash
# Publish a synthetic H.264 RTSP stream to MediaMTX so the kernel can be tested without
# real cameras / credentials. Requires MediaMTX running (scripts/dev.sh or run it directly).
#
# Usage: scripts/synth_camera.sh [path] [size] [fps]
#   path  MediaMTX path name (default: cam_test)
#   size  WxH (default: 1280x720)
#   fps   frames per second (default: 15)
set -euo pipefail
PATH_NAME="${1:-cam_test}"
SIZE="${2:-1280x720}"
FPS="${3:-15}"
RTSP="rtsp://127.0.0.1:8554/${PATH_NAME}"

echo "Publishing synthetic camera -> ${RTSP} (${SIZE} @ ${FPS}fps). Ctrl-C to stop."
exec ffmpeg -nostdin -hide_banner -loglevel warning -re \
  -f lavfi -i "testsrc=size=${SIZE}:rate=${FPS}" \
  -c:v libx264 -preset ultrafast -tune zerolatency -g $((FPS * 2)) -pix_fmt yuv420p \
  -f rtsp -rtsp_transport tcp "${RTSP}"
