// Heldar Core — Stage 3 zone + detection surface for a single camera.
//
// Renders the live AI-sampled frame as an editable canvas: click to draw a
// polygon zone (normalized 0..1), name/classify/save it (POST); list existing
// zones with an enable toggle + delete; draw every zone as a severity-coloured
// polygon and overlay live detection bboxes (label + track id) on the same
// frame. A zone-events feed (enter/exit/dwell) shows evidence thumbnails.

import { useEffect, useMemo, useRef, useState } from "react";
import type { FormEvent } from "react";
import { api, ApiError } from "../lib/api";
import { usePoll } from "../lib/usePoll";
import type {
  Detection,
  Severity,
  Zone,
  ZoneEvent,
  ZonePoint,
} from "../lib/types";
import { Button, Field, Input, Panel, Select, Spinner, cx } from "./ui";
import { formatClock, formatDuration, timeAgo } from "../lib/format";

/* ------------------------------- helpers ------------------------------- */

const SEVERITY_COLOR: Record<Severity, string> = {
  info: "#71717a",
  warning: "#fbbf24",
  critical: "#ef4444",
};

/** Boxes older than this (vs. the latest detection's timestamp) are not drawn. */
const OVERLAY_FRESH_MS = 20_000;

const KIND_OPTIONS = ["region", "line", "dwell"] as const;
const SEVERITY_OPTIONS: Severity[] = ["info", "warning", "critical"];

function clamp01(n: number): number {
  return Math.max(0, Math.min(1, n));
}

function confidencePct(c?: number | null): string {
  if (c == null || !Number.isFinite(c)) return "—";
  return `${Math.round(c * 100)}%`;
}

function isBbox(b: unknown): b is [number, number, number, number] {
  return Array.isArray(b) && b.length === 4 && b.every((n) => typeof n === "number");
}

/** Coerce the server's polygon JSON into a clean [x,y][] of finite numbers. */
function asPolygon(poly: unknown): ZonePoint[] {
  if (!Array.isArray(poly)) return [];
  const out: ZonePoint[] = [];
  for (const p of poly) {
    if (Array.isArray(p) && p.length >= 2 && typeof p[0] === "number" && typeof p[1] === "number") {
      out.push([p[0], p[1]]);
    }
  }
  return out;
}

/** "x1,y1 x2,y2 …" scaled to the 0..100 SVG viewBox. */
function svgPoints(poly: ZonePoint[]): string {
  return poly.map(([x, y]) => `${x * 100},${y * 100}`).join(" ");
}

function centroid(poly: ZonePoint[]): ZonePoint {
  if (poly.length === 0) return [0, 0];
  let sx = 0;
  let sy = 0;
  for (const [x, y] of poly) {
    sx += x;
    sy += y;
  }
  return [sx / poly.length, sy / poly.length];
}

/* ------------------------------- canvas -------------------------------- */

function ZoneCanvas({
  cameraId,
  zones,
  boxes,
  draft,
  drawing,
  onAddPoint,
}: {
  cameraId: string;
  zones: Zone[];
  boxes: Detection[];
  draft: ZonePoint[];
  drawing: boolean;
  onAddPoint: (p: ZonePoint) => void;
}) {
  const [tick, setTick] = useState(0);
  // null = loading, true = a frame loaded, false = 404 / no frame yet.
  const [frameOk, setFrameOk] = useState<boolean | null>(null);
  const wrapRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setFrameOk(null);
  }, [cameraId]);

  useEffect(() => {
    const t = setInterval(() => setTick((n) => n + 1), 1500);
    return () => clearInterval(t);
  }, []);

  const src = `${api.frameUrl(cameraId, "sub")}&_=${tick}`;

  function handleClick(e: React.MouseEvent<HTMLDivElement>) {
    if (!drawing) return;
    const rect = e.currentTarget.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return;
    const x = clamp01((e.clientX - rect.left) / rect.width);
    const y = clamp01((e.clientY - rect.top) / rect.height);
    onAddPoint([Number(x.toFixed(4)), Number(y.toFixed(4))]);
  }

  return (
    <div
      ref={wrapRef}
      onClick={handleClick}
      className={cx(
        "relative select-none overflow-hidden rounded-md border border-line bg-black",
        drawing && "cursor-crosshair ring-1 ring-accent/60",
      )}
    >
      <img
        key={cameraId}
        src={src}
        alt="Latest sampled frame"
        className={cx("block w-full", frameOk === false && "hidden")}
        onLoad={() => setFrameOk(true)}
        onError={() => setFrameOk(false)}
      />

      {/* Vector overlay: zone polygons, detection bboxes, live draft. */}
      {frameOk !== false && (
        <svg
          className="pointer-events-none absolute inset-0 h-full w-full"
          viewBox="0 0 100 100"
          preserveAspectRatio="none"
          aria-hidden="true"
        >
          {zones.map((z) => {
            const poly = asPolygon(z.polygon);
            if (poly.length < 3) return null;
            const color = SEVERITY_COLOR[z.severity] ?? SEVERITY_COLOR.info;
            return (
              <polygon
                key={z.id}
                points={svgPoints(poly)}
                fill={`${color}1f`}
                stroke={color}
                strokeWidth={1.5}
                strokeOpacity={z.enabled ? 1 : 0.35}
                strokeDasharray={z.enabled ? undefined : "4 3"}
                vectorEffect="non-scaling-stroke"
                strokeLinejoin="round"
              />
            );
          })}

          {boxes.map((d, i) => {
            const [x, y, w, h] = d.bbox as [number, number, number, number];
            return (
              <rect
                key={d.id ?? i}
                x={x * 100}
                y={y * 100}
                width={w * 100}
                height={h * 100}
                rx={0.6}
                fill="none"
                stroke="#f59e0b"
                strokeWidth={1.5}
                vectorEffect="non-scaling-stroke"
              />
            );
          })}

          {draft.length > 0 && (
            <polygon
              points={svgPoints(draft)}
              fill="rgba(245,158,11,0.12)"
              stroke="#f59e0b"
              strokeWidth={1.5}
              strokeDasharray="4 3"
              vectorEffect="non-scaling-stroke"
              strokeLinejoin="round"
            />
          )}
        </svg>
      )}

      {/* Zone name labels at polygon centroids (HTML to avoid SVG text distortion). */}
      {frameOk !== false &&
        zones.map((z) => {
          const poly = asPolygon(z.polygon);
          if (poly.length < 3) return null;
          const [cx2, cy2] = centroid(poly);
          const color = SEVERITY_COLOR[z.severity] ?? SEVERITY_COLOR.info;
          return (
            <span
              key={z.id}
              className={cx(
                "pointer-events-none absolute -translate-x-1/2 -translate-y-1/2 whitespace-nowrap rounded-sm px-1 py-px font-mono text-[9px] font-semibold uppercase tracking-micro",
                !z.enabled && "opacity-50",
              )}
              style={{
                left: `${cx2 * 100}%`,
                top: `${cy2 * 100}%`,
                color,
                backgroundColor: "rgba(0,0,0,0.6)",
                border: `1px solid ${color}80`,
              }}
            >
              {z.name}
            </span>
          );
        })}

      {/* Detection labels at bbox top-left. */}
      {frameOk !== false &&
        boxes.map((d, i) => {
          const [x, y] = d.bbox as [number, number, number, number];
          return (
            <span
              key={`lbl-${d.id ?? i}`}
              className="pointer-events-none absolute -translate-y-full whitespace-nowrap rounded-sm bg-accent px-1 py-px font-mono text-[9px] font-semibold uppercase tracking-micro text-accent-ink"
              style={{ left: `${x * 100}%`, top: `${y * 100}%` }}
            >
              {d.label ?? "obj"}
              {d.track_id != null && ` #${d.track_id}`}
              {d.confidence != null && ` ${confidencePct(d.confidence)}`}
            </span>
          );
        })}

      {/* Draft vertices (HTML dots — undistorted). */}
      {frameOk !== false &&
        draft.map(([x, y], i) => (
          <span
            key={`v-${i}`}
            className="pointer-events-none absolute h-2 w-2 -translate-x-1/2 -translate-y-1/2 rounded-full border border-accent-ink bg-accent shadow-[0_0_0_1px_rgba(0,0,0,0.6)]"
            style={{ left: `${x * 100}%`, top: `${y * 100}%` }}
          />
        ))}

      {/* Draw-mode hint */}
      {drawing && frameOk !== false && (
        <span className="pointer-events-none absolute left-2 top-2 inline-flex items-center gap-1.5 rounded bg-black/70 px-2 py-1 font-mono text-[9px] font-semibold uppercase tracking-micro text-accent-soft backdrop-blur">
          Click to add points · {draft.length} pt{draft.length === 1 ? "" : "s"}
        </span>
      )}

      {/* Sampled badge */}
      {!drawing && frameOk === true && (
        <span className="pointer-events-none absolute right-2 top-2 inline-flex items-center gap-1.5 rounded bg-black/60 px-1.5 py-1 backdrop-blur">
          <span className="inline-flex h-1.5 w-1.5 animate-led-ping rounded-full bg-rec" />
          <span className="font-mono text-[9px] font-semibold uppercase tracking-micro text-rec">
            Sampled
          </span>
        </span>
      )}

      {frameOk === null && (
        <div className="flex items-center justify-center gap-2 py-16 font-mono text-xs text-fg-muted">
          <Spinner size={14} /> Waiting for frame…
        </div>
      )}

      {frameOk === false && (
        <div className="flex flex-col items-center justify-center gap-2 px-6 py-16 text-center">
          <svg
            aria-hidden="true"
            viewBox="0 0 48 48"
            className="h-9 w-9 text-fg-muted"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
          >
            <rect x="7" y="13" width="34" height="24" rx="3" />
            <circle cx="24" cy="25" r="6" />
            <path d="M10 10 38 40" strokeLinecap="round" />
          </svg>
          <div className="font-mono text-xs font-semibold text-fg-secondary">No sampled frame</div>
          <p className="max-w-xs font-mono text-[11px] leading-relaxed text-fg-muted">
            Zones are drawn over the AI-sampled frame. Add &amp; enable a detection task on this
            camera (AI Perception above) to start sampling.
          </p>
        </div>
      )}
    </div>
  );
}

/* -------------------------------- panel -------------------------------- */

export function ZonePanel({ cameraId }: { cameraId: string }) {
  const zones = usePoll(() => api.listZones(cameraId), 10000, [cameraId]);
  const detections = usePoll(
    () => api.cameraDetections(cameraId, { limit: 50 }),
    5000,
    [cameraId],
  );
  const zoneEvents = usePoll(
    () => api.cameraZoneEvents(cameraId, { limit: 50 }),
    8000,
    [cameraId],
  );

  // ---- Draft polygon + new-zone form ----
  const [draft, setDraft] = useState<ZonePoint[]>([]);
  const [drawing, setDrawing] = useState(false);
  const [name, setName] = useState("");
  const [kind, setKind] = useState<string>("region");
  const [severity, setSeverity] = useState<Severity>("warning");
  const [dwell, setDwell] = useState("0");
  const [labels, setLabels] = useState("");
  const [busy, setBusy] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);

  function startDrawing() {
    setDraft([]);
    setDrawing(true);
    setFormError(null);
  }

  function cancelDrawing() {
    setDraft([]);
    setDrawing(false);
    setFormError(null);
  }

  function addPoint(p: ZonePoint) {
    setDraft((d) => [...d, p]);
  }

  async function saveZone(e: FormEvent) {
    e.preventDefault();
    setFormError(null);
    if (!name.trim()) {
      setFormError("Zone name is required.");
      return;
    }
    if (draft.length < 3) {
      setFormError("Draw at least 3 points to define a zone polygon.");
      return;
    }
    const parsedLabels = labels
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    setBusy(true);
    try {
      await api.createZone(cameraId, {
        name: name.trim(),
        polygon: draft,
        kind,
        severity,
        dwell_seconds: Number(dwell) || 0,
        labels: parsedLabels,
      });
      setName("");
      setDwell("0");
      setLabels("");
      setDraft([]);
      setDrawing(false);
      await zones.refresh();
    } catch (err) {
      setFormError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function toggleZone(z: Zone) {
    setBusy(true);
    try {
      await api.updateZone(z.id, { enabled: !z.enabled });
      await zones.refresh();
    } catch (err) {
      setFormError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function removeZone(z: Zone) {
    if (!window.confirm(`Delete zone "${z.name}"?`)) return;
    setBusy(true);
    try {
      await api.deleteZone(z.id);
      await zones.refresh();
    } catch (err) {
      setFormError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  const zoneList = zones.data ?? [];
  const detList = detections.data ?? [];
  const evList = zoneEvents.data ?? [];

  // Overlay: boxes from the most recent detection batch, if fresh enough.
  const overlayBoxes = useMemo(() => {
    if (detList.length === 0) return [];
    const latest = detList[0].timestamp;
    const age = Date.now() - new Date(latest).getTime();
    if (!Number.isFinite(age) || age > OVERLAY_FRESH_MS) return [];
    return detList.filter((d) => d.timestamp === latest && isBbox(d.bbox)).slice(0, 32);
  }, [detList]);

  // zone_id -> severity, for colouring zone-event rows.
  const severityByZone = useMemo(() => {
    const m = new Map<string, Severity>();
    for (const z of zoneList) m.set(z.id, z.severity);
    return m;
  }, [zoneList]);

  return (
    <>
      <Panel
        title="Zones"
        subtitle="Draw regions on the sampled frame · live detection overlay"
        padded={false}
        actions={
          drawing ? (
            <div className="flex items-center gap-1.5">
              <Button
                size="sm"
                disabled={draft.length === 0}
                onClick={() => setDraft((d) => d.slice(0, -1))}
              >
                Undo point
              </Button>
              <Button size="sm" variant="ghost" onClick={cancelDrawing}>
                Cancel
              </Button>
            </div>
          ) : (
            <Button size="sm" variant="primary" onClick={startDrawing}>
              New zone
            </Button>
          )
        }
      >
        <div className="p-3">
          <ZoneCanvas
            cameraId={cameraId}
            zones={zoneList}
            boxes={overlayBoxes}
            draft={draft}
            drawing={drawing}
            onAddPoint={addPoint}
          />

          {drawing && (
            <form onSubmit={saveZone} className="mt-3 space-y-3 border-t border-line pt-3">
              <div className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
                New zone · {draft.length} point{draft.length === 1 ? "" : "s"}
              </div>
              <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                <Field label="Name" htmlFor="zone-name">
                  <Input
                    id="zone-name"
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                    placeholder="Entrance"
                  />
                </Field>
                <Field label="Kind" htmlFor="zone-kind">
                  <Select
                    id="zone-kind"
                    value={kind}
                    onChange={(e) => setKind(e.target.value)}
                  >
                    {KIND_OPTIONS.map((k) => (
                      <option key={k} value={k}>
                        {k}
                      </option>
                    ))}
                  </Select>
                </Field>
                <Field label="Severity" htmlFor="zone-severity">
                  <Select
                    id="zone-severity"
                    value={severity}
                    onChange={(e) => setSeverity(e.target.value as Severity)}
                  >
                    {SEVERITY_OPTIONS.map((s) => (
                      <option key={s} value={s}>
                        {s}
                      </option>
                    ))}
                  </Select>
                </Field>
                <Field label="Dwell (s)" htmlFor="zone-dwell" hint="0 = enter/exit only">
                  <Input
                    id="zone-dwell"
                    type="number"
                    min={0}
                    step={0.5}
                    value={dwell}
                    onChange={(e) => setDwell(e.target.value)}
                  />
                </Field>
              </div>
              <Field
                label="Labels"
                htmlFor="zone-labels"
                hint="Comma-separated detection labels (blank = all)"
              >
                <Input
                  id="zone-labels"
                  value={labels}
                  onChange={(e) => setLabels(e.target.value)}
                  placeholder="person, car"
                />
              </Field>
              <div className="flex items-center gap-2">
                <Button
                  type="submit"
                  variant="primary"
                  disabled={busy || draft.length < 3}
                  className="flex-1"
                >
                  {busy ? "Saving…" : "Save zone"}
                </Button>
                <Button type="button" variant="ghost" onClick={cancelDrawing}>
                  Cancel
                </Button>
              </div>
              {formError && <p className="font-mono text-xs text-danger">{formError}</p>}
            </form>
          )}
        </div>
      </Panel>

      <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
        <Panel
          title="Defined Zones"
          subtitle="Toggle / delete"
          actions={
            <span className="font-mono text-[11px] tabular-nums text-fg-muted">
              {zoneList.length}
            </span>
          }
        >
          {zoneList.length === 0 ? (
            <p className="font-mono text-xs text-fg-muted">
              {zones.error ?? 'No zones yet. Use "New zone" to draw one on the frame.'}
            </p>
          ) : (
            <ul className="space-y-1.5">
              {zoneList.map((z) => (
                <ZoneRow
                  key={z.id}
                  zone={z}
                  busy={busy}
                  onToggle={() => void toggleZone(z)}
                  onDelete={() => void removeZone(z)}
                />
              ))}
            </ul>
          )}
          {!drawing && formError && (
            <p className="mt-3 font-mono text-xs text-danger">{formError}</p>
          )}
        </Panel>

        <Panel
          title="Zone Events"
          subtitle="Enter / exit / dwell"
          actions={
            <span className="font-mono text-[11px] tabular-nums text-fg-muted">
              {evList.length}
            </span>
          }
        >
          {evList.length === 0 ? (
            <p className="font-mono text-xs text-fg-muted">
              {zoneEvents.error ?? "No zone events recorded."}
            </p>
          ) : (
            <ul className="-mr-1 max-h-[420px] space-y-1.5 overflow-y-auto pr-1">
              {evList.map((ev) => (
                <ZoneEventRow
                  key={ev.id}
                  ev={ev}
                  severity={severityByZone.get(ev.zone_id) ?? "info"}
                />
              ))}
            </ul>
          )}
        </Panel>
      </div>
    </>
  );
}

/* ------------------------------- rows ---------------------------------- */

function ZoneRow({
  zone,
  busy,
  onToggle,
  onDelete,
}: {
  zone: Zone;
  busy: boolean;
  onToggle: () => void;
  onDelete: () => void;
}) {
  const color = SEVERITY_COLOR[zone.severity] ?? SEVERITY_COLOR.info;
  const pts = asPolygon(zone.polygon).length;
  const zoneLabels = Array.isArray(zone.labels) ? zone.labels : [];
  return (
    <li className="flex items-center justify-between gap-2 rounded-md border border-line bg-canvas px-2.5 py-2 transition-colors duration-150 hover:border-[#34373e]">
      <div className="min-w-0">
        <div className="flex items-center gap-2">
          <span
            className="inline-flex h-2.5 w-2.5 shrink-0 rounded-sm"
            style={{
              backgroundColor: `${color}33`,
              border: `1px solid ${color}`,
              opacity: zone.enabled ? 1 : 0.4,
            }}
          />
          <span className="truncate font-mono text-xs font-semibold text-fg">{zone.name}</span>
        </div>
        <div className="mt-0.5 flex flex-wrap items-center gap-x-2 font-mono text-[10px] text-fg-muted">
          <span className="uppercase" style={{ color }}>
            {zone.severity}
          </span>
          <span className="text-fg-muted/60">·</span>
          <span>{zone.kind}</span>
          <span className="text-fg-muted/60">·</span>
          <span className="tabular-nums">{pts} pts</span>
          {zone.dwell_seconds > 0 && (
            <>
              <span className="text-fg-muted/60">·</span>
              <span className="tabular-nums">{zone.dwell_seconds}s dwell</span>
            </>
          )}
          {zoneLabels.length > 0 && (
            <>
              <span className="text-fg-muted/60">·</span>
              <span className="truncate">{zoneLabels.join(", ")}</span>
            </>
          )}
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-1.5">
        <Button size="sm" disabled={busy} onClick={onToggle}>
          {zone.enabled ? "Disable" : "Enable"}
        </Button>
        <Button
          size="sm"
          variant="danger"
          disabled={busy}
          onClick={onDelete}
          aria-label="Delete zone"
        >
          ✕
        </Button>
      </div>
    </li>
  );
}

function ZoneEventRow({ ev, severity }: { ev: ZoneEvent; severity: Severity }) {
  const color = SEVERITY_COLOR[severity] ?? SEVERITY_COLOR.info;
  return (
    <li
      className="flex items-stretch gap-2.5 rounded-md border-l-2 border border-line bg-canvas py-2 pl-2.5 pr-2.5"
      style={{ borderLeftColor: color }}
    >
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span
            className="shrink-0 rounded border px-1.5 py-0.5 font-mono text-[9px] font-semibold uppercase tracking-micro"
            style={{ color, borderColor: `${color}55`, backgroundColor: `${color}1a` }}
          >
            {ev.event_type}
          </span>
          <span className="truncate text-xs font-semibold text-fg">{ev.zone_name}</span>
        </div>
        <div className="mt-0.5 flex flex-wrap items-center gap-x-2 font-mono text-[10px] text-fg-muted">
          <span title={formatClock(ev.timestamp)}>{timeAgo(ev.timestamp)}</span>
          {ev.label && (
            <>
              <span className="text-fg-muted/60">·</span>
              <span className="text-fg-secondary">{ev.label}</span>
            </>
          )}
          {ev.track_id != null && (
            <>
              <span className="text-fg-muted/60">·</span>
              <span>#{ev.track_id}</span>
            </>
          )}
          {ev.dwell_seconds != null && ev.dwell_seconds > 0 && (
            <>
              <span className="text-fg-muted/60">·</span>
              <span className="tabular-nums">{formatDuration(ev.dwell_seconds)}</span>
            </>
          )}
        </div>
      </div>
      {ev.evidence_path && (
        <a
          href={ev.evidence_path}
          target="_blank"
          rel="noreferrer"
          className="group shrink-0 self-center"
          title="Open evidence frame"
        >
          <img
            src={ev.evidence_path}
            alt="Zone event evidence"
            loading="lazy"
            className="h-11 w-16 rounded border border-line bg-black object-cover transition-colors duration-150 group-hover:border-accent"
          />
        </a>
      )}
    </li>
  );
}

export default ZonePanel;
