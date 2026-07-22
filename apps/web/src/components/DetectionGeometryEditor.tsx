// On-camera detection geometry editors (issue #46 follow-up): draw line-crossing lines and
// intrusion regions ON the camera frame and write them to the device, plus the motion
// arm/sensitivity controls. Mirrors the ZonePanel drawing idiom (frame <img> + normalized-viewBox
// SVG overlay, click to place points) — but these shapes live on the CAMERA, not in Heldar.

import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../lib/api";
import type {
  IntrusionConfig,
  LineCrossingConfig,
  MotionConfig,
  SmartLine,
  SmartRegion,
} from "../lib/types";
import { Button, Field } from "./ui";

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

const SELECT_CLS =
  "w-full rounded-md border border-line bg-raised px-2.5 py-1.5 font-mono text-xs text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent";
const SLOT_COLORS = ["#f59e0b", "#34d399", "#60a5fa", "#f472b6"];

/** Camera frame + normalized SVG overlay; reports clicks in 0..1 coordinates. */
function FrameCanvas({
  cameraId,
  onPick,
  children,
}: {
  cameraId: string;
  onPick?: (p: [number, number]) => void;
  children: React.ReactNode;
}) {
  // A stable cache-buster per mount: the frame refreshes when the editor opens, not per render.
  const tickRef = useRef(Date.now());
  const boxRef = useRef<HTMLDivElement>(null);

  function handleClick(e: React.MouseEvent) {
    if (!onPick || !boxRef.current) return;
    const r = boxRef.current.getBoundingClientRect();
    const x = (e.clientX - r.left) / r.width;
    const y = (e.clientY - r.top) / r.height;
    onPick([Math.min(1, Math.max(0, x)), Math.min(1, Math.max(0, y))]);
  }

  return (
    <div
      ref={boxRef}
      className={`relative overflow-hidden rounded-md border border-line bg-black ${onPick ? "cursor-crosshair" : ""}`}
      onClick={handleClick}
    >
      <img
        src={`${api.frameUrl(cameraId, "sub")}&_=${tickRef.current}`}
        alt="Camera frame"
        className="block w-full select-none"
        draggable={false}
      />
      <svg
        viewBox="0 0 1 1"
        preserveAspectRatio="none"
        className="pointer-events-none absolute inset-0 h-full w-full"
      >
        {children}
      </svg>
    </div>
  );
}

/* ------------------------------ line crossing ------------------------------ */

export function LineCrossingEditor({
  cameraId,
  canManage,
  onClose,
}: {
  cameraId: string;
  canManage: boolean;
  onClose: () => void;
}) {
  const [cfg, setCfg] = useState<LineCrossingConfig | null>(null);
  const [slot, setSlot] = useState(0);
  const [draft, setDraft] = useState<[number, number][]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setCfg(await api.getCameraLineCrossing(cameraId));
      setError(null);
    } catch (e) {
      setError(errMsg(e));
    }
  }, [cameraId]);
  useEffect(() => {
    void load();
  }, [load]);

  function updateLine(idx: number, patch: Partial<SmartLine>) {
    setCfg((c) =>
      c ? { ...c, lines: c.lines.map((l, i) => (i === idx ? { ...l, ...patch } : l)) } : c,
    );
  }

  function pick(p: [number, number]) {
    if (!canManage) return;
    const next = [...draft, p].slice(-2) as [number, number][];
    setDraft(next);
    if (next.length === 2) {
      updateLine(slot, { points: next, enabled: true });
      setDraft([]);
    }
  }

  async function save() {
    if (!cfg) return;
    setBusy(true);
    setError(null);
    try {
      setCfg(await api.putCameraLineCrossing(cameraId, cfg));
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setBusy(false);
    }
  }

  if (!cfg) {
    return error ? (
      <p className="mt-2 font-mono text-[11px] text-danger">{error}</p>
    ) : (
      <p className="mt-2 font-mono text-[11px] text-fg-muted">Loading…</p>
    );
  }
  const sel = cfg.lines[slot];

  return (
    <div className="mt-3 space-y-2 rounded-md border border-line bg-canvas p-2.5">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
          Line crossing — click two points to place line {sel?.id}
        </span>
        <div className="flex gap-1">
          {cfg.lines.map((l, i) => (
            <Button
              key={l.id}
              size="sm"
              variant={i === slot ? "primary" : "default"}
              onClick={() => {
                setSlot(i);
                setDraft([]);
              }}
            >
              <span style={{ color: i === slot ? undefined : SLOT_COLORS[i % 4] }}>L{l.id}</span>
            </Button>
          ))}
        </div>
      </div>

      <FrameCanvas cameraId={cameraId} onPick={canManage ? pick : undefined}>
        {cfg.lines.map((l, i) =>
          l.points.length === 2 && !(l.points[0][0] === l.points[1][0] && l.points[0][1] === l.points[1][1]) ? (
            <line
              key={l.id}
              x1={l.points[0][0]}
              y1={l.points[0][1]}
              x2={l.points[1][0]}
              y2={l.points[1][1]}
              stroke={SLOT_COLORS[i % 4]}
              strokeWidth={i === slot ? 0.008 : 0.004}
              strokeDasharray={l.enabled ? undefined : "0.02 0.012"}
              vectorEffect="non-scaling-stroke"
            />
          ) : null,
        )}
        {draft.map((p, i) => (
          <circle key={i} cx={p[0]} cy={p[1]} r={0.008} fill={SLOT_COLORS[slot % 4]} />
        ))}
      </FrameCanvas>

      {sel && (
        <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={sel.enabled}
              disabled={!canManage || busy}
              onChange={(e) => updateLine(slot, { enabled: e.target.checked })}
            />
            <span className="font-mono text-xs text-fg-secondary">Armed</span>
          </label>
          <Field label="Direction" htmlFor="line-dir">
            <select
              id="line-dir"
              className={SELECT_CLS}
              value={sel.direction}
              disabled={!canManage || busy}
              onChange={(e) => updateLine(slot, { direction: e.target.value })}
            >
              <option value="any">Both ways</option>
              <option value="left-right">Left → right</option>
              <option value="right-left">Right → left</option>
            </select>
          </Field>
          <Field label={`Sensitivity ${sel.sensitivity}`} htmlFor="line-sens">
            <input
              id="line-sens"
              type="range"
              min={1}
              max={100}
              value={sel.sensitivity}
              disabled={!canManage || busy}
              onChange={(e) => updateLine(slot, { sensitivity: Number(e.target.value) })}
              className="mt-2 w-full accent-[var(--accent,#6ee7b7)]"
            />
          </Field>
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={cfg.enabled}
              disabled={!canManage || busy}
              onChange={(e) => setCfg((c) => (c ? { ...c, enabled: e.target.checked } : c))}
            />
            <span className="font-mono text-xs text-fg-secondary">Master on</span>
          </label>
        </div>
      )}

      {error && <p className="font-mono text-[11px] text-danger">{error}</p>}
      <div className="flex gap-2">
        <Button size="sm" variant="primary" disabled={!canManage || busy} onClick={() => void save()}>
          {busy ? "Saving…" : "Save to camera"}
        </Button>
        <Button size="sm" variant="ghost" disabled={busy} onClick={() => void load()}>
          Revert
        </Button>
        <Button size="sm" variant="ghost" onClick={onClose}>
          Close
        </Button>
      </div>
    </div>
  );
}

/* ------------------------------ intrusion regions ------------------------------ */

export function IntrusionEditor({
  cameraId,
  canManage,
  onClose,
}: {
  cameraId: string;
  canManage: boolean;
  onClose: () => void;
}) {
  const [cfg, setCfg] = useState<IntrusionConfig | null>(null);
  const [slot, setSlot] = useState(0);
  const [draft, setDraft] = useState<[number, number][]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setCfg(await api.getCameraIntrusion(cameraId));
      setError(null);
    } catch (e) {
      setError(errMsg(e));
    }
  }, [cameraId]);
  useEffect(() => {
    void load();
  }, [load]);

  function updateRegion(idx: number, patch: Partial<SmartRegion>) {
    setCfg((c) =>
      c ? { ...c, regions: c.regions.map((r, i) => (i === idx ? { ...r, ...patch } : r)) } : c,
    );
  }

  function finishDraft() {
    if (draft.length >= 3) {
      updateRegion(slot, { points: draft, enabled: true });
    }
    setDraft([]);
  }

  async function save() {
    if (!cfg) return;
    setBusy(true);
    setError(null);
    try {
      setCfg(await api.putCameraIntrusion(cameraId, cfg));
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setBusy(false);
    }
  }

  if (!cfg) {
    return error ? (
      <p className="mt-2 font-mono text-[11px] text-danger">{error}</p>
    ) : (
      <p className="mt-2 font-mono text-[11px] text-fg-muted">Loading…</p>
    );
  }
  const sel = cfg.regions[slot];
  const toSvg = (pts: [number, number][]) => pts.map((p) => `${p[0]},${p[1]}`).join(" ");

  return (
    <div className="mt-3 space-y-2 rounded-md border border-line bg-canvas p-2.5">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
          Intrusion — click to add points for region {sel?.id} ({draft.length} pt
          {draft.length === 1 ? "" : "s"}), then Finish — most models require exactly 4
        </span>
        <div className="flex gap-1">
          {cfg.regions.map((r, i) => (
            <Button
              key={r.id}
              size="sm"
              variant={i === slot ? "primary" : "default"}
              onClick={() => {
                setSlot(i);
                setDraft([]);
              }}
            >
              <span style={{ color: i === slot ? undefined : SLOT_COLORS[i % 4] }}>R{r.id}</span>
            </Button>
          ))}
        </div>
      </div>

      <FrameCanvas
        cameraId={cameraId}
        onPick={canManage ? (p) => setDraft((d) => [...d, p].slice(0, 10)) : undefined}
      >
        {cfg.regions.map((r, i) =>
          r.points.length >= 3 ? (
            <polygon
              key={r.id}
              points={toSvg(r.points)}
              fill={`${SLOT_COLORS[i % 4]}22`}
              stroke={SLOT_COLORS[i % 4]}
              strokeWidth={i === slot ? 0.006 : 0.003}
              strokeDasharray={r.enabled ? undefined : "0.02 0.012"}
              vectorEffect="non-scaling-stroke"
            />
          ) : null,
        )}
        {draft.length > 0 && (
          <polygon
            points={toSvg(draft)}
            fill="none"
            stroke={SLOT_COLORS[slot % 4]}
            strokeWidth={0.006}
            strokeDasharray="0.015 0.01"
            vectorEffect="non-scaling-stroke"
          />
        )}
        {draft.map((p, i) => (
          <circle key={i} cx={p[0]} cy={p[1]} r={0.007} fill={SLOT_COLORS[slot % 4]} />
        ))}
      </FrameCanvas>

      {sel && (
        <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={sel.enabled}
              disabled={!canManage || busy}
              onChange={(e) => updateRegion(slot, { enabled: e.target.checked })}
            />
            <span className="font-mono text-xs text-fg-secondary">Armed</span>
          </label>
          <Field label={`Sensitivity ${sel.sensitivity}`} htmlFor="reg-sens">
            <input
              id="reg-sens"
              type="range"
              min={1}
              max={100}
              value={sel.sensitivity}
              disabled={!canManage || busy}
              onChange={(e) => updateRegion(slot, { sensitivity: Number(e.target.value) })}
              className="mt-2 w-full accent-[var(--accent,#6ee7b7)]"
            />
          </Field>
          <Field label="Dwell (s) before alarm" htmlFor="reg-thresh">
            <input
              id="reg-thresh"
              type="number"
              min={0}
              max={100}
              value={sel.time_threshold}
              disabled={!canManage || busy}
              onChange={(e) => updateRegion(slot, { time_threshold: Number(e.target.value) })}
              className={SELECT_CLS}
            />
          </Field>
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={cfg.enabled}
              disabled={!canManage || busy}
              onChange={(e) => setCfg((c) => (c ? { ...c, enabled: e.target.checked } : c))}
            />
            <span className="font-mono text-xs text-fg-secondary">Master on</span>
          </label>
        </div>
      )}

      {error && <p className="font-mono text-[11px] text-danger">{error}</p>}
      <div className="flex flex-wrap gap-2">
        <Button size="sm" disabled={!canManage || draft.length < 3} onClick={finishDraft}>
          Finish region ({draft.length})
        </Button>
        <Button size="sm" variant="ghost" disabled={draft.length === 0} onClick={() => setDraft([])}>
          Discard draft
        </Button>
        <Button
          size="sm"
          variant="danger"
          disabled={!canManage || busy || !sel || sel.points.length === 0}
          onClick={() => updateRegion(slot, { points: [], enabled: false })}
        >
          Clear region
        </Button>
        <Button size="sm" variant="primary" disabled={!canManage || busy} onClick={() => void save()}>
          {busy ? "Saving…" : "Save to camera"}
        </Button>
        <Button size="sm" variant="ghost" disabled={busy} onClick={() => void load()}>
          Revert
        </Button>
        <Button size="sm" variant="ghost" onClick={onClose}>
          Close
        </Button>
      </div>
    </div>
  );
}

/* ------------------------------ motion ------------------------------ */

export function MotionEditor({
  cameraId,
  canManage,
  onClose,
}: {
  cameraId: string;
  canManage: boolean;
  onClose: () => void;
}) {
  const [cfg, setCfg] = useState<MotionConfig | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .getCameraMotion(cameraId)
      .then(setCfg)
      .catch((e) => setError(errMsg(e)));
  }, [cameraId]);

  async function save() {
    if (!cfg) return;
    setBusy(true);
    setError(null);
    try {
      setCfg(await api.putCameraMotion(cameraId, cfg));
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setBusy(false);
    }
  }

  if (!cfg) {
    return error ? (
      <p className="mt-2 font-mono text-[11px] text-danger">{error}</p>
    ) : (
      <p className="mt-2 font-mono text-[11px] text-fg-muted">Loading…</p>
    );
  }

  return (
    <div className="mt-3 space-y-2 rounded-md border border-line bg-canvas p-2.5">
      <div className="grid grid-cols-2 gap-3">
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={cfg.enabled}
            disabled={!canManage || busy}
            onChange={(e) => setCfg((c) => (c ? { ...c, enabled: e.target.checked } : c))}
          />
          <span className="font-mono text-xs text-fg-secondary">Motion detection armed</span>
        </label>
        {cfg.sensitivity != null && (
          <Field label={`Sensitivity ${cfg.sensitivity}`} htmlFor="mot-sens">
            <input
              id="mot-sens"
              type="range"
              min={0}
              max={100}
              value={cfg.sensitivity}
              disabled={!canManage || busy}
              onChange={(e) =>
                setCfg((c) => (c ? { ...c, sensitivity: Number(e.target.value) } : c))
              }
              className="mt-2 w-full accent-[var(--accent,#6ee7b7)]"
            />
          </Field>
        )}
      </div>
      <p className="font-mono text-[10px] text-fg-muted">
        The motion grid (which cells are watched) stays as configured on the device — full-frame by
        default.
      </p>
      {error && <p className="font-mono text-[11px] text-danger">{error}</p>}
      <div className="flex gap-2">
        <Button size="sm" variant="primary" disabled={!canManage || busy} onClick={() => void save()}>
          {busy ? "Saving…" : "Save to camera"}
        </Button>
        <Button size="sm" variant="ghost" onClick={onClose}>
          Close
        </Button>
      </div>
    </div>
  );
}
