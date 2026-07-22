// Client codec capability. The box records H.265+ (a standards-compliant HEVC bitstream, ~4× smaller
// than H.264), and ships it untouched so the client's hardware decoder does the work — the most
// efficient path. Chrome/Edge (with HW HEVC), Safari, and virtually all phones decode it; Firefox and
// some old devices don't. We detect that tail and show a clear note instead of a black frame.

let cached: boolean | null = null;

/** Whether this browser can decode HEVC/H.265 in-page — native `<video>` or via MSE (hls.js). Cached. */
export function hevcDecodeSupported(): boolean {
  if (cached !== null) return cached;
  const types = ['video/mp4; codecs="hvc1.1.6.L93.B0"', 'video/mp4; codecs="hev1.1.6.L93.B0"'];
  let ok = false;
  try {
    const v = document.createElement("video");
    const native = types.some((t) => {
      const r = v.canPlayType(t);
      return r === "probably" || r === "maybe";
    });
    const mse =
      typeof MediaSource !== "undefined" &&
      typeof MediaSource.isTypeSupported === "function" &&
      types.some((t) => MediaSource.isTypeSupported(t));
    ok = native || mse;
  } catch {
    ok = false;
  }
  cached = ok;
  return ok;
}

/** A user-facing explanation for the no-HEVC tail. */
export const HEVC_UNSUPPORTED_NOTE =
  "These recordings are H.265 (HEVC) — efficient on storage, but this browser can't decode it in-page. " +
  "Open the dashboard in Chrome, Edge, or Safari (or a phone) to play recorded footage. Live view still works everywhere.";
