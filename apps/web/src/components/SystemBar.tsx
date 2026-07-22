// Compatibility shim: the live telemetry now lives in the AppShell top bar.
// This remains a thin wrapper so any lingering imports keep compiling and
// render a compact, on-theme strip from the same SystemInfo payload.

import type { SystemInfo } from "../lib/types";
import { formatBytes, formatUptime } from "../lib/format";
import { StatusLed } from "./ui";

interface Props {
  info: SystemInfo | null;
  error?: string | null;
}

function Metric({ label, value, tone }: { label: string; value: string; tone?: string }) {
  return (
    <div className="flex flex-col gap-0.5 leading-none">
      <span className="font-mono text-[9px] uppercase tracking-micro text-fg-muted">{label}</span>
      <span className={`font-mono text-[13px] font-semibold ${tone ?? "text-fg"}`}>{value}</span>
    </div>
  );
}

export function SystemBar({ info, error }: Props) {
  if (error && !info) {
    return (
      <div className="flex items-center gap-2 border-b border-line bg-danger/10 px-4 py-2 font-mono text-xs text-danger">
        <StatusLed state="error" /> Core unreachable — {error}
      </div>
    );
  }
  if (!info) {
    return (
      <div className="flex items-center gap-2 border-b border-line bg-panel px-4 py-2 font-mono text-xs text-fg-muted">
        <StatusLed state="connecting" /> Linking to Heldar Core…
      </div>
    );
  }

  const usedBytes = info.recordings_bytes;
  const maxBytes = info.max_recordings_gb * 1024 ** 3;
  const usedPct = maxBytes > 0 ? Math.min(100, (usedBytes / maxBytes) * 100) : 0;
  const fill = usedPct > 90 ? "#ef4444" : usedPct > 75 ? "#fbbf24" : "#f59e0b";

  return (
    <div className="flex flex-wrap items-center gap-x-6 gap-y-3 border-b border-line bg-panel px-4 py-2.5">
      <div className="flex items-center gap-2">
        <StatusLed state={info.recorder_enabled ? "recording" : "offline"} />
        <span className="font-mono text-[10px] uppercase tracking-micro text-fg-secondary">
          Recorder {info.recorder_enabled ? "On" : "Off"}
        </span>
      </div>

      <Metric
        label="Recording"
        value={`${info.cameras_recording} / ${info.cameras_total}`}
        tone={info.cameras_recording > 0 ? "text-rec" : undefined}
      />
      <Metric label="Recorders" value={String(info.active_recorders)} />
      <Metric label="Segments" value={info.segments_total.toLocaleString()} />

      <div className="flex min-w-[160px] flex-col gap-1 leading-none">
        <span className="font-mono text-[9px] uppercase tracking-micro text-fg-muted">Storage</span>
        <div className="flex items-center gap-2">
          <div className="h-1.5 w-24 overflow-hidden rounded-full bg-line">
            <div className="h-full rounded-full" style={{ width: `${usedPct}%`, backgroundColor: fill }} />
          </div>
          <span className="whitespace-nowrap font-mono text-[12px] font-semibold text-fg">
            {formatBytes(usedBytes)} / {info.max_recordings_gb.toFixed(0)} GB
          </span>
        </div>
      </div>

      <Metric label="Uptime" value={formatUptime(info.uptime_seconds)} />

      <div className="ml-auto font-mono text-[10px] text-fg-muted">
        {info.name} v{info.version}
      </div>
    </div>
  );
}

export default SystemBar;
