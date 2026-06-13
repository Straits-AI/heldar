import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import type { CameraStatus, CameraView } from "../lib/types";
import { api } from "../lib/api";
import { timeAgo } from "../lib/format";
import { StatusBadge } from "./StatusBadge";

interface Props {
  camera: CameraView;
  status?: CameraStatus;
}

const SNAPSHOT_REFRESH_MS = 15000;

export function CameraCard({ camera, status }: Props) {
  const [tick, setTick] = useState(() => Date.now());
  const [imgError, setImgError] = useState(false);

  useEffect(() => {
    const t = setInterval(() => setTick(Date.now()), SNAPSHOT_REFRESH_MS);
    return () => clearInterval(t);
  }, []);

  // Reset the error flag on each refresh so a recovering camera shows up again.
  useEffect(() => {
    setImgError(false);
  }, [tick]);

  const state = status?.state ?? (camera.enabled ? "unknown" : "disabled");
  const showThumb = camera.enabled && !imgError;
  const thumbSrc = `${api.snapshotUrl(camera.id)}?_=${tick}`;
  const resolution = camera.resolution_main ?? camera.resolution_sub;

  return (
    <Link
      to={`/cameras/${encodeURIComponent(camera.id)}`}
      className="panel group block overflow-hidden transition-colors hover:border-accent/50"
    >
      <div className="relative aspect-video w-full overflow-hidden bg-black">
        {showThumb ? (
          <img
            key={tick}
            src={thumbSrc}
            alt={`${camera.name} snapshot`}
            className="h-full w-full object-cover"
            onError={() => setImgError(true)}
          />
        ) : (
          <div className="flex h-full w-full items-center justify-center text-xs text-slate-600">
            {camera.enabled ? "No frame available" : "Disabled"}
          </div>
        )}
        <div className="absolute left-2 top-2">
          <StatusBadge state={state} />
        </div>
        {status?.fps_observed != null && (
          <div className="absolute bottom-2 right-2 rounded bg-black/60 px-1.5 py-0.5 font-mono text-[10px] text-slate-200">
            {status.fps_observed.toFixed(1)} fps
          </div>
        )}
      </div>

      <div className="flex items-start justify-between gap-2 px-3 py-2.5">
        <div className="min-w-0">
          <div className="truncate text-sm font-semibold text-slate-100 group-hover:text-white">
            {camera.name}
          </div>
          <div className="mt-0.5 truncate text-xs text-slate-500">
            <span className="uppercase">{camera.vendor}</span>
            {camera.model ? ` · ${camera.model}` : ""}
            {resolution ? ` · ${resolution}` : ""}
          </div>
        </div>
        <div className="shrink-0 text-right">
          <div className="font-mono text-[11px] text-slate-400">{camera.id}</div>
          <div className="text-[11px] text-slate-600">
            {status?.last_segment_at ? timeAgo(status.last_segment_at) : "no footage"}
          </div>
        </div>
      </div>
    </Link>
  );
}
