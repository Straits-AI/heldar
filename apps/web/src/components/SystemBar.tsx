import type { SystemInfo } from "../lib/types";
import { formatBytes, formatUptime } from "../lib/format";

interface Props {
  info: SystemInfo | null;
  error?: string | null;
}

function Stat({ label, value, accent }: { label: string; value: string; accent?: string }) {
  return (
    <div className="flex flex-col leading-tight">
      <span className="stat-k">{label}</span>
      <span className={`stat-v ${accent ?? ""}`}>{value}</span>
    </div>
  );
}

export function SystemBar({ info, error }: Props) {
  if (error && !info) {
    return (
      <div className="border-b border-line bg-red-950/30 px-4 py-1.5 text-xs text-red-300">
        Core unreachable — {error}
      </div>
    );
  }
  if (!info) {
    return (
      <div className="border-b border-line bg-panel px-4 py-2.5 text-xs text-slate-500">
        Connecting to VisionOps Core…
      </div>
    );
  }

  const usedBytes = info.recordings_bytes;
  const maxBytes = info.max_recordings_gb * 1024 ** 3;
  const usedPct = maxBytes > 0 ? Math.min(100, (usedBytes / maxBytes) * 100) : 0;
  const storagePctClass =
    usedPct > 90 ? "bg-red-500" : usedPct > 75 ? "bg-amber-500" : "bg-accent";

  return (
    <div className="flex flex-wrap items-center gap-x-6 gap-y-2 border-b border-line bg-panel px-4 py-2">
      <div className="flex items-center gap-2">
        <span
          className={`h-2 w-2 rounded-full ${info.recorder_enabled ? "bg-emerald-400" : "bg-zinc-500"}`}
          title={info.recorder_enabled ? "Recorder enabled" : "Recorder disabled"}
        />
        <span className="text-xs font-medium text-slate-300">
          Recorder {info.recorder_enabled ? "on" : "off"}
        </span>
      </div>

      <Stat
        label="Cameras"
        value={`${info.cameras_recording}/${info.cameras_total} rec`}
        accent={info.cameras_recording > 0 ? "text-emerald-300" : undefined}
      />
      <Stat label="Active recorders" value={String(info.active_recorders)} />
      <Stat label="Segments" value={info.segments_total.toLocaleString()} />

      <div className="flex min-w-[160px] flex-col leading-tight">
        <span className="stat-k">Storage</span>
        <div className="flex items-center gap-2">
          <div className="h-1.5 w-24 overflow-hidden rounded-full bg-line">
            <div className={`h-full ${storagePctClass}`} style={{ width: `${usedPct}%` }} />
          </div>
          <span className="stat-v whitespace-nowrap">
            {formatBytes(usedBytes)} / {info.max_recordings_gb.toFixed(0)} GB
          </span>
        </div>
      </div>

      <Stat label="Uptime" value={formatUptime(info.uptime_seconds)} />

      <div className="ml-auto text-[11px] text-slate-600">
        {info.name} v{info.version}
      </div>
    </div>
  );
}
