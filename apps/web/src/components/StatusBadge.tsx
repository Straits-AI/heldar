import type { CameraStatusState } from "../lib/types";

interface StatusStyle {
  label: string;
  dot: string;
  text: string;
  ring: string;
  pulse: boolean;
}

const STYLES: Record<CameraStatusState, StatusStyle> = {
  recording: {
    label: "Recording",
    dot: "bg-emerald-400",
    text: "text-emerald-300",
    ring: "bg-emerald-500/10 ring-emerald-500/30",
    pulse: true,
  },
  connecting: {
    label: "Connecting",
    dot: "bg-amber-400",
    text: "text-amber-300",
    ring: "bg-amber-500/10 ring-amber-500/30",
    pulse: true,
  },
  offline: {
    label: "Offline",
    dot: "bg-slate-400",
    text: "text-slate-300",
    ring: "bg-slate-500/10 ring-slate-500/30",
    pulse: false,
  },
  error: {
    label: "Error",
    dot: "bg-red-400",
    text: "text-red-300",
    ring: "bg-red-500/10 ring-red-500/30",
    pulse: false,
  },
  disabled: {
    label: "Disabled",
    dot: "bg-zinc-500",
    text: "text-zinc-400",
    ring: "bg-zinc-500/10 ring-zinc-500/30",
    pulse: false,
  },
  unknown: {
    label: "Unknown",
    dot: "bg-zinc-500",
    text: "text-zinc-400",
    ring: "bg-zinc-500/10 ring-zinc-500/30",
    pulse: false,
  },
};

interface Props {
  state: CameraStatusState | string | undefined;
  className?: string;
}

export function StatusBadge({ state, className = "" }: Props) {
  const style = STYLES[(state ?? "unknown") as CameraStatusState] ?? STYLES.unknown;
  return (
    <span
      className={`inline-flex items-center gap-1.5 rounded-full px-2 py-0.5 text-[11px] font-semibold uppercase tracking-wide ring-1 ring-inset ${style.ring} ${style.text} ${className}`}
    >
      <span className="relative flex h-1.5 w-1.5">
        {style.pulse && (
          <span
            className={`absolute inline-flex h-full w-full animate-ping rounded-full opacity-75 ${style.dot}`}
          />
        )}
        <span className={`relative inline-flex h-1.5 w-1.5 rounded-full ${style.dot}`} />
      </span>
      {style.label}
    </span>
  );
}
