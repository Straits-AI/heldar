#!/usr/bin/env bash
# Publish a synthetic RTSP stream to MediaMTX so the kernel can be tested without real cameras /
# credentials. Requires MediaMTX running (scripts/dev.sh or run it directly).
#
# Start heldar-core FIRST: MediaMTX delegates publish authorization to the kernel
# (`authMethod: http` -> /internal/mediamtx-auth), so publishing while the core is down is
# denied with `ANNOUNCE failed: 401 Unauthorized` and ffmpeg exits straight away.
#
# The ffmpeg incantation lives here and nowhere else — the benchmark harness (#119) shells out to
# this script rather than growing a second copy that drifts from it.
#
# Usage: scripts/synth_camera.sh [path] [size] [fps] [codec] [bitrate_kbps] [gop] [rtsp_base]
#   path         MediaMTX path name          (default: cam_test)
#   size         WxH                         (default: 1280x720)
#   fps          frames per second           (default: 15)
#   codec        h264 | h265                 (default: h264)
#   bitrate_kbps target bitrate, 0 = CRF     (default: 0)
#   gop          keyframe interval in frames (default: fps * 2)
#   rtsp_base    RTSP server base            (default: rtsp://127.0.0.1:8554)
set -euo pipefail
PATH_NAME="${1:-cam_test}"
SIZE="${2:-1280x720}"
FPS="${3:-15}"
CODEC="${4:-h264}"
BITRATE_KBPS="${5:-0}"
GOP="${6:-$((FPS * 2))}"
RTSP_BASE="${7:-rtsp://127.0.0.1:8554}"
RTSP="${RTSP_BASE}/${PATH_NAME}"

case "$CODEC" in
  h264) ENCODER=libx264 ;;
  h265|hevc) ENCODER=libx265 ;;
  *) echo "unsupported codec: $CODEC (want h264 or h265)" >&2; exit 2 ;;
esac

# A qualification run needs the DECLARED bitrate actually to arrive, not merely to be allowed.
#
# The obvious spelling — `-b:v X -maxrate X` — is capped VBV, and VBV is a CEILING. `testsrc` is a
# synthetic pattern that compresses far below any realistic camera bitrate, so a "2 Mbps" profile
# published 1017 kbps: half the bytes through the recorder, half the disk throughput, and a storage
# figure that did not match the arithmetic the sizing guide rests on. Measured, not assumed — an
# unloaded encoder at the 2000k cap emits 1017k, which is exactly what the fleet was emitting.
#
# So: true CBR with filler (`nal-hrd=cbr` / `strict-cbr`), which also matches what a real IP camera
# in CBR mode does. Verified at 1970/2000 kbps for x264 and 3839/4000 for x265.
RATE=()
CBR264=()
CBR265=""
if [ "$BITRATE_KBPS" -gt 0 ]; then
  # bufsize == bitrate is a one-second VBV window: the tighter the buffer, the closer the output
  # tracks the target instead of drifting under it across a segment.
  RATE=(-b:v "${BITRATE_KBPS}k" -minrate "${BITRATE_KBPS}k" -maxrate "${BITRATE_KBPS}k" \
        -bufsize "${BITRATE_KBPS}k")
  CBR264=(-x264-params "nal-hrd=cbr:force-cfr=1")
  CBR265=":strict-cbr=1"
fi

# libx265 does not read -tune zerolatency; it takes the equivalent through -x265-params.
TUNE=()
if [ "$ENCODER" = libx264 ]; then
  TUNE=(-tune zerolatency "${CBR264[@]}")
else
  TUNE=(-x265-params "keyint=${GOP}:min-keyint=${GOP}:scenecut=0:bframes=0:log-level=error${CBR265}")
fi

echo "Publishing synthetic camera -> ${RTSP} (${SIZE} @ ${FPS}fps, ${CODEC}, ${BITRATE_KBPS}kbps, gop ${GOP})."
exec ffmpeg -nostdin -hide_banner -loglevel warning -re \
  -f lavfi -i "testsrc=size=${SIZE}:rate=${FPS}" \
  -c:v "$ENCODER" -preset ultrafast "${TUNE[@]}" -g "$GOP" -pix_fmt yuv420p "${RATE[@]}" \
  -f rtsp -rtsp_transport tcp "${RTSP}"
