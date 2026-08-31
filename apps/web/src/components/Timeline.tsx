import { useMemo, useRef, useState } from "react";
import type { MouseEvent as ReactMouseEvent } from "react";
import type { Timeline as TimelineData } from "../lib/types";
import { formatClockIn, formatDuration, formatTimeShortIn, viewerZone } from "../lib/format";
import { StatusLed } from "./ui";

interface Props {
  timeline: TimelineData;
  /** Window bounds (RFC3339). Falls back to the timeline's own from/to. */
  from?: string | null;
  to?: string | null;
  /** Currently selected instant (RFC3339), shown as a marker. */
  selected?: string | null;
  onPick?: (iso: string) => void;
  /**
   * IANA zone to render every clock in (#148). Defaults to the viewer's, which is what this
   * component did unconditionally before — and which put a browser-zone timeline directly under a
   * site-zone schedule label on the camera page.
   *
   * The CALLER resolves it, because only the caller knows which camera this is and therefore which
   * site's zone applies. A component resolving its own would have to fetch, and could resolve it
   * differently from the label rendered beside it.
   */
  zone?: string;
}

interface TickLabel {
  pct: number;
  label: string;
}

function buildTicks(start: number, end: number, zone: string): TickLabel[] {
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
    // Ticks are the axis a reader scans; leaving them in the browser's zone while the endpoints
    // below moved would put two different clocks on one strip.
    const label = formatTimeShortIn(new Date(t).toISOString(), zone).replace(/:\d{2}(?=\s|$)/, "");
    ticks.push({ pct: ((t - start) / span) * 100, label });
  }
  return ticks;
}

export function Timeline({ timeline, from, to, selected, onPick, zone }: Props) {
  const tz = zone ?? viewerZone();
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

  const ticks = useMemo(() => (valid ? buildTicks(startMs, endMs, tz) : []), [startMs, endMs, valid, tz]);

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
      <div className="rounded-md border border-dashed border-line bg-canvas px-3 py-8 text-center font-mono text-[11px] uppercase tracking-micro text-fg-muted">
        No time window to display
      </div>
    );
  }

  return (
    <div className="select-none">
      {/* Header: window bounds + recorded summary */}
      <div className="mb-2 flex items-center justify-between gap-3 font-mono text-[10px]">
        <span className="uppercase tracking-micro text-fg-muted">
          {formatClockIn(new Date(startMs).toISOString(), tz)}
        </span>
        <span className="flex items-center gap-1.5 text-fg-secondary">
          <StatusLed state="recording" pulse={false} />
          <span className="tabular-nums">{formatDuration(timeline.recorded_seconds)} recorded</span>
          <span className="text-fg-muted">·</span>
          <span className="tabular-nums">{timeline.segment_count} seg</span>
        </span>
        <span className="uppercase tracking-micro text-fg-muted">
          {formatClockIn(new Date(endMs).toISOString(), tz)}
        </span>
      </div>

      <div
        ref={trackRef}
        className="relative h-14 cursor-crosshair overflow-hidden rounded-md border border-line bg-canvas"
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
        {/* faint inner gradient floor */}
        <div className="pointer-events-none absolute inset-0 bg-gradient-to-b from-transparent to-black/30" />

        {/* availability blocks (recorded footage) */}
        {blocks.map((b, i) => (
          <div
            key={i}
            className="absolute top-0 h-full bg-rec/25 transition-colors duration-150 hover:bg-rec/45"
            style={{
              left: `${b.left}%`,
              width: `${Math.max(b.width, 0.4)}%`,
              boxShadow: "inset 0 0 0 1px rgba(16,185,129,0.25)",
            }}
            title={`${formatClockIn(b.range.start, tz)} → ${formatClockIn(b.range.end, tz)} (${formatDuration(b.range.seconds)})`}
          >
            <span className="absolute inset-x-0 top-0 h-px bg-rec/70" />
          </div>
        ))}

        {/* hour ticks */}
        {ticks.map((t, i) => (
          <div key={i} className="absolute top-0 h-full" style={{ left: `${t.pct}%` }}>
            <div className="h-full w-px bg-line" />
            <div className="absolute bottom-0.5 left-1 font-mono text-[9px] tabular-nums text-fg-muted">
              {t.label}
            </div>
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
            style={{ left: `${selectedPct}%`, boxShadow: "0 0 8px 0 rgba(245,158,11,0.6)" }}
          >
            <div className="absolute -top-px left-1/2 h-2 w-2 -translate-x-1/2 rotate-45 bg-accent" />
          </div>
        )}
      </div>

      <div className="mt-1.5 flex h-4 items-center font-mono text-[11px]">
        {hoverIso ? (
          <span className="text-fg-secondary">
            Click to seek ·{" "}
            <span className="tabular-nums text-accent">{formatClockIn(hoverIso, tz)}</span>
          </span>
        ) : selected ? (
          <span className="text-fg-secondary">
            Selected ·{" "}
            <span className="tabular-nums text-fg">{formatClockIn(selected, tz)}</span>
          </span>
        ) : (
          <span className="uppercase tracking-micro text-fg-muted">
            Hover and click to pick a moment
          </span>
        )}
      </div>
    </div>
  );
}
