import { useMemo, useRef, useState } from "react";
import type { MouseEvent as ReactMouseEvent } from "react";
import type { Timeline as TimelineData } from "../lib/types";
import { formatClock, formatDuration } from "../lib/format";

interface Props {
  timeline: TimelineData;
  /** Window bounds (RFC3339). Falls back to the timeline's own from/to. */
  from?: string | null;
  to?: string | null;
  /** Currently selected instant (RFC3339), shown as a marker. */
  selected?: string | null;
  onPick?: (iso: string) => void;
}

interface TickLabel {
  pct: number;
  label: string;
}

function buildTicks(start: number, end: number): TickLabel[] {
  const span = end - start;
  if (span <= 0) return [];
  const target = 6;
  // Snap the tick interval to a sensible unit.
  const stepsMs = [
    60_000, 5 * 60_000, 15 * 60_000, 30 * 60_000, 60 * 60_000, 3 * 3600_000, 6 * 3600_000,
    12 * 3600_000, 24 * 3600_000,
  ];
  const raw = span / target;
  const step = stepsMs.find((s) => s >= raw) ?? stepsMs[stepsMs.length - 1];
  const ticks: TickLabel[] = [];
  const first = Math.ceil(start / step) * step;
  for (let t = first; t <= end; t += step) {
    const label = new Date(t).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    ticks.push({ pct: ((t - start) / span) * 100, label });
  }
  return ticks;
}

export function Timeline({ timeline, from, to, selected, onPick }: Props) {
  const trackRef = useRef<HTMLDivElement>(null);
  const [hoverIso, setHoverIso] = useState<string | null>(null);
  const [hoverPct, setHoverPct] = useState<number | null>(null);

  const startMs = new Date(from ?? timeline.from ?? "").getTime();
  const endMs = new Date(to ?? timeline.to ?? "").getTime();
  const valid = Number.isFinite(startMs) && Number.isFinite(endMs) && endMs > startMs;
  const span = valid ? endMs - startMs : 0;

  const blocks = useMemo(() => {
    if (!valid) return [];
    return timeline.ranges
      .map((r) => {
        const s = new Date(r.start).getTime();
        const e = new Date(r.end).getTime();
        const left = ((Math.max(s, startMs) - startMs) / span) * 100;
        const width = ((Math.min(e, endMs) - Math.max(s, startMs)) / span) * 100;
        return { left, width, range: r };
      })
      .filter((b) => b.width > 0);
  }, [timeline.ranges, startMs, endMs, span, valid]);

  const ticks = useMemo(() => (valid ? buildTicks(startMs, endMs) : []), [startMs, endMs, valid]);

  const selectedPct =
    selected && valid ? ((new Date(selected).getTime() - startMs) / span) * 100 : null;

  function pctToIso(pct: number): string {
    const t = startMs + (pct / 100) * span;
    return new Date(t).toISOString();
  }

  function handleMove(e: ReactMouseEvent<HTMLDivElement>) {
    const el = trackRef.current;
    if (!el || !valid) return;
    const rect = el.getBoundingClientRect();
    const pct = Math.min(100, Math.max(0, ((e.clientX - rect.left) / rect.width) * 100));
    setHoverPct(pct);
    setHoverIso(pctToIso(pct));
  }

  function handleClick(e: ReactMouseEvent<HTMLDivElement>) {
    const el = trackRef.current;
    if (!el || !valid || !onPick) return;
    const rect = el.getBoundingClientRect();
    const pct = Math.min(100, Math.max(0, ((e.clientX - rect.left) / rect.width) * 100));
    onPick(pctToIso(pct));
  }

  if (!valid) {
    return (
      <div className="rounded-md border border-line bg-ink px-3 py-6 text-center text-sm text-slate-500">
        No time window to display.
      </div>
    );
  }

  return (
    <div className="select-none">
      <div className="mb-1 flex items-center justify-between text-[11px] text-slate-500">
        <span>{formatClock(new Date(startMs).toISOString())}</span>
        <span className="text-slate-400">
          {formatDuration(timeline.recorded_seconds)} recorded · {timeline.segment_count} segments
        </span>
        <span>{formatClock(new Date(endMs).toISOString())}</span>
      </div>

      <div
        ref={trackRef}
        className="relative h-12 cursor-crosshair overflow-hidden rounded-md border border-line bg-ink"
        onMouseMove={handleMove}
        onMouseLeave={() => {
          setHoverIso(null);
          setHoverPct(null);
        }}
        onClick={handleClick}
        role="slider"
        aria-label="Recording timeline"
        aria-valuemin={startMs}
        aria-valuemax={endMs}
        aria-valuenow={selected ? new Date(selected).getTime() : endMs}
        tabIndex={0}
      >
        {/* availability blocks */}
        {blocks.map((b, i) => (
          <div
            key={i}
            className="absolute top-0 h-full bg-emerald-500/30 hover:bg-emerald-500/45"
            style={{ left: `${b.left}%`, width: `${Math.max(b.width, 0.4)}%` }}
            title={`${formatClock(b.range.start)} → ${formatClock(b.range.end)} (${formatDuration(b.range.seconds)})`}
          />
        ))}

        {/* hour ticks */}
        {ticks.map((t, i) => (
          <div key={i} className="absolute top-0 h-full" style={{ left: `${t.pct}%` }}>
            <div className="h-full w-px bg-line" />
            <div className="absolute bottom-0.5 left-1 text-[9px] text-slate-600">{t.label}</div>
          </div>
        ))}

        {/* hover indicator */}
        {hoverPct != null && (
          <div
            className="pointer-events-none absolute top-0 h-full w-px bg-accent/70"
            style={{ left: `${hoverPct}%` }}
          />
        )}

        {/* selected marker */}
        {selectedPct != null && selectedPct >= 0 && selectedPct <= 100 && (
          <div
            className="pointer-events-none absolute top-0 h-full w-0.5 bg-accent"
            style={{ left: `${selectedPct}%` }}
          >
            <div className="absolute -top-0.5 left-1/2 h-2 w-2 -translate-x-1/2 rotate-45 bg-accent" />
          </div>
        )}
      </div>

      <div className="mt-1 h-4 text-[11px] text-slate-400">
        {hoverIso ? (
          <span>
            Click to seek · <span className="font-mono text-slate-300">{formatClock(hoverIso)}</span>
          </span>
        ) : selected ? (
          <span>
            Selected · <span className="font-mono text-slate-300">{formatClock(selected)}</span>
          </span>
        ) : (
          <span className="text-slate-600">Hover and click to pick a moment</span>
        )}
      </div>
    </div>
  );
}
