import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import type { CameraStatus, CameraView } from "../lib/types";
import { api } from "../lib/api";
import { timeAgo } from "../lib/format";
import { cx, Spinner, StatusLed, StatusPill } from "./ui";

interface Props {
  camera: CameraView;
  status?: CameraStatus;
}

// Live thumbnails refresh on this cadence; the <img> element stays mounted and
// only its src cache-buster changes so frames swap without a black flash.
const SNAPSHOT_REFRESH_MS = 10000;

const TELE_TONE = {
  default: "text-fg-secondary",
  good: "text-rec",
  warn: "text-connecting",
  bad: "text-danger",
} as const;

/* Compact monospace telemetry cell. */
function Tele({
  label,
  value,
  unit,
  tone = "default",
}: {
  label: string;
  value: string | number;
  unit?: string;
  tone?: keyof typeof TELE_TONE;
}) {
  return (
    <div className="flex min-w-0 flex-col gap-0.5">
      <span className="font-mono text-[9px] uppercase tracking-micro text-fg-muted">
        {label}
      </span>
      <span className="flex items-baseline gap-0.5 truncate">
        <span className={cx("truncate font-mono text-xs font-medium tabular-nums", TELE_TONE[tone])}>
          {value}
        </span>
        {unit != null && <span className="font-mono text-[9px] text-fg-muted">{unit}</span>}
      </span>
    </div>
  );
}

/* Placeholder shown when a camera is disabled or its snapshot fails to load. */
function ThumbPlaceholder({ label }: { label: string }) {
  return (
    <div className="flex h-full w-full flex-col items-center justify-center gap-2 bg-[#0b0c0f] text-fg-muted">
      <svg
        viewBox="0 0 32 32"
        width="26"
        height="26"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
      >
        <rect x="4" y="9" width="24" height="16" rx="2.5" />
        <circle cx="16" cy="17" r="4" />
        <path d="M12 9l2-2.5h4L20 9" />
        <path d="M4 5l24 22" />
      </svg>
      <span className="font-mono text-[10px] uppercase tracking-micro">{label}</span>
    </div>
  );
}

export function CameraCard({ camera, status }: Props) {
  const [tick, setTick] = useState(() => Date.now());
  const [imgError, setImgError] = useState(false);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    const t = setInterval(() => setTick(Date.now()), SNAPSHOT_REFRESH_MS);
    return () => clearInterval(t);
  }, []);

  // Reset the error flag on each refresh so a recovering camera shows up again.
  useEffect(() => {
    setImgError(false);
  }, [tick]);

  const state = status?.state ?? (camera.enabled ? "unknown" : "disabled");
  const isRecording = state === "recording";
  const showThumb = camera.enabled && !imgError;
  const thumbSrc = `${api.snapshotUrl(camera.id)}?_=${tick}`;
  const resolution = camera.resolution_main ?? camera.resolution_sub ?? "—";

  const addressLine = camera.address
    ? `${camera.address}:${camera.rtsp_port}`
    : `${camera.vendor.toUpperCase()}${camera.model ? ` · ${camera.model}` : ""}`;

  const segments = status?.segments_written;
  const reconnects = status?.reconnect_count;
  const bitrate = status?.bitrate_kbps;
  const bitrateHigh = bitrate != null && bitrate >= 1000;
  const bitrateValue =
    bitrate == null ? "—" : bitrateHigh ? (bitrate / 1000).toFixed(1) : Math.round(bitrate).toString();

  return (
    <Link
      to={`/cameras/${encodeURIComponent(camera.id)}`}
      aria-label={`Open camera ${camera.name}`}
      className="group relative flex flex-col overflow-hidden rounded-panel border border-line bg-panel shadow-panel transition-all duration-150 hover:-translate-y-0.5 hover:border-[#34373e] hover:shadow-raised focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-canvas"
    >
      {/* ---- Live thumbnail ---- */}
      <div className="relative aspect-video w-full overflow-hidden bg-black">
        {showThumb ? (
          <img
            src={thumbSrc}
            alt={`${camera.name} snapshot`}
            className="h-full w-full object-cover transition-opacity duration-300"
            style={{ opacity: loaded ? 1 : 0 }}
            onLoad={() => setLoaded(true)}
            onError={() => setImgError(true)}
          />
        ) : (
          <ThumbPlaceholder label={camera.enabled ? "No signal" : "Disabled"} />
        )}

        {showThumb && !loaded && (
          <div className="absolute inset-0 flex items-center justify-center">
            <Spinner size={18} />
          </div>
        )}

        {/* Legibility scrims */}
        <div className="pointer-events-none absolute inset-x-0 top-0 h-12 bg-gradient-to-b from-black/65 to-transparent" />
        <div className="pointer-events-none absolute inset-x-0 bottom-0 h-10 bg-gradient-to-t from-black/60 to-transparent" />

        {/* Status pill */}
        <div className="absolute left-2 top-2">
          <StatusPill state={state} />
        </div>

        {/* Glowing REC indicator */}
        {isRecording && (
          <div className="absolute right-2 top-2 flex items-center gap-1.5 rounded-md bg-black/55 px-1.5 py-1 backdrop-blur-sm">
            <StatusLed state="recording" />
            <span className="font-mono text-[10px] font-semibold uppercase tracking-micro text-rec">
              REC
            </span>
          </div>
        )}

        {/* Observed framerate */}
        {status?.fps_observed != null && (
          <div className="absolute bottom-2 right-2 rounded bg-black/55 px-1.5 py-0.5 font-mono text-[10px] tabular-nums text-fg backdrop-blur-sm">
            {status.fps_observed.toFixed(1)} fps
          </div>
        )}
      </div>

      {/* ---- Identity + telemetry ---- */}
      <div className="flex flex-1 flex-col gap-2.5 p-3">
        <div className="flex items-start justify-between gap-2">
          <div className="min-w-0">
            <div className="truncate text-sm font-semibold text-fg transition-colors group-hover:text-accent">
              {camera.name}
            </div>
            <div className="mt-0.5 truncate font-mono text-[11px] text-fg-muted">
              {addressLine}
            </div>
          </div>
          <span
            aria-hidden="true"
            className="mt-0.5 shrink-0 text-fg-muted opacity-0 transition-all duration-150 group-hover:translate-x-0.5 group-hover:text-accent group-hover:opacity-100"
          >
            <svg
              viewBox="0 0 16 16"
              width="14"
              height="14"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d="M5 11l6-6M6 5h5v5" />
            </svg>
          </span>
        </div>

        <dl className="grid grid-cols-4 gap-2 border-t border-line pt-2.5">
          <Tele label="Res" value={resolution} />
          <Tele label="Seg" value={segments != null ? segments.toLocaleString() : "—"} />
          <Tele
            label="Rate"
            value={bitrateValue}
            unit={bitrate == null ? undefined : bitrateHigh ? "Mb/s" : "kb/s"}
          />
          <Tele
            label="Rcn"
            value={reconnects ?? "—"}
            tone={reconnects != null && reconnects > 0 ? "warn" : "default"}
          />
        </dl>

        <div className="flex items-center justify-between gap-2 font-mono text-[10px] text-fg-muted">
          <span className="truncate" title={camera.id}>
            {camera.id}
          </span>
          <span className="shrink-0">
            {status?.last_segment_at ? timeAgo(status.last_segment_at) : "no footage"}
          </span>
        </div>

        {state === "error" && status?.last_error && (
          <div
            className="truncate border-t border-line pt-2 font-mono text-[10px] text-danger"
            title={status.last_error}
          >
            {status.last_error}
          </div>
        )}
      </div>
    </Link>
  );
}
