// Heldar Core — Stage 2 AI surface for a single camera.
// Lists this camera's AI tasks (toggle / delete), an "Add AI task" form, the live
// sampled-frame preview with detection-bbox overlay, and a recent-detections feed.

import { useEffect, useMemo, useState } from "react";
import type { FormEvent } from "react";
import { api, ApiError } from "../lib/api";
import { usePoll } from "../lib/usePoll";
import type { AiTask, Detection, StreamProfile, ZonePoint } from "../lib/types";
import { Button, Field, Input, Panel, Select, Spinner, cx } from "./ui";
import { formatClock, timeAgo } from "../lib/format";

/* ------------------------------- helpers ------------------------------- */

/** Boxes older than this (vs. the latest detection's timestamp) are not drawn. */
const OVERLAY_FRESH_MS = 20_000;

/** Detection model choices written to the task's `config.weights`. */
const MODEL_OPTIONS = [
  { value: "yolo26n.pt", label: "YOLO26n (recommended)" },
  { value: "yolov8n.pt", label: "YOLOv8n (faster)" },
] as const;

/** A full-frame polygon (normalized 0..1) — the one-click presence zone created by the presets. */
const FULL_FRAME_POLYGON: ZonePoint[] = [
  [0, 0],
  [1, 0],
  [1, 1],
  [0, 1],
];

/** Object labels each one-click preset watches for. */
const PRESET_LABELS = {
  people: ["person"],
  vehicles: ["car", "truck", "bus", "motorcycle"],
} as const;

function confidencePct(c?: number | null): string {
  if (c == null || !Number.isFinite(c)) return "—";
  return `${Math.round(c * 100)}%`;
}

function isBbox(b: unknown): b is [number, number, number, number] {
  return Array.isArray(b) && b.length === 4 && b.every((n) => typeof n === "number");
}

/* -------------------------------- frame -------------------------------- */

function SampledFrame({
  cameraId,
  boxes,
}: {
  cameraId: string;
  boxes: Detection[];
}) {
  const [tick, setTick] = useState(0);
  // null = loading, true = a frame loaded, false = 404 / no frame yet.
  const [frameOk, setFrameOk] = useState<boolean | null>(null);

  useEffect(() => {
    setFrameOk(null);
  }, [cameraId]);

  useEffect(() => {
    const t = setInterval(() => setTick((n) => n + 1), 1000);
    return () => clearInterval(t);
  }, []);

  const src = `${api.frameUrl(cameraId)}?_=${tick}`;

  return (
    <div className="relative overflow-hidden rounded-md border border-line bg-black">
      <img
        key={cameraId}
        src={src}
        alt="Latest sampled frame"
        className={cx("block w-full", frameOk === false && "hidden")}
        onLoad={() => setFrameOk(true)}
        onError={() => setFrameOk(false)}
      />

      {frameOk !== false &&
        boxes.map((d, i) => {
          const [x, y, w, h] = d.bbox as [number, number, number, number];
          return (
            <div
              key={d.id ?? i}
              className="pointer-events-none absolute rounded-sm border border-accent shadow-[0_0_0_1px_rgba(0,0,0,0.5)]"
              style={{
                left: `${x * 100}%`,
                top: `${y * 100}%`,
                width: `${w * 100}%`,
                height: `${h * 100}%`,
              }}
            >
              <span className="absolute left-0 top-0 -translate-y-full whitespace-nowrap rounded-sm bg-accent px-1 py-px font-mono text-[9px] font-semibold uppercase tracking-micro text-accent-ink">
                {d.label ?? "obj"}
                {d.confidence != null && ` ${confidencePct(d.confidence)}`}
              </span>
            </div>
          );
        })}

      {/* Live badge */}
      {frameOk === true && (
        <span className="absolute right-2 top-2 inline-flex items-center gap-1.5 rounded bg-black/60 px-1.5 py-1 backdrop-blur">
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
            The sampler writes a frame only while an AI task is enabled. Add &amp; enable a task to
            start sampling this camera.
          </p>
        </div>
      )}
    </div>
  );
}

/* ------------------------------- tasks --------------------------------- */

function TaskRow({
  task,
  busy,
  onToggle,
  onDelete,
}: {
  task: AiTask;
  busy: boolean;
  onToggle: () => void;
  onDelete: () => void;
}) {
  return (
    <li className="flex items-center justify-between gap-2 rounded-md border border-line bg-canvas px-2.5 py-2 transition-colors duration-150 hover:border-[#34373e]">
      <div className="min-w-0">
        <div className="flex items-center gap-2">
          <span
            className={cx(
              "inline-flex h-1.5 w-1.5 shrink-0 rounded-full",
              task.enabled ? "bg-rec shadow-glow-rec" : "bg-disabled",
            )}
          />
          <span className="truncate font-mono text-xs font-semibold text-fg">{task.task_type}</span>
        </div>
        <div className="mt-0.5 flex flex-wrap items-center gap-x-2 font-mono text-[10px] text-fg-muted">
          <span className="tabular-nums">{task.fps.toFixed(1)} fps</span>
          <span className="text-fg-muted/60">·</span>
          <span className="tabular-nums">{task.width}px</span>
          <span className="text-fg-muted/60">·</span>
          <span className="uppercase">{task.stream_profile}</span>
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-1.5">
        <Button size="sm" disabled={busy} onClick={onToggle}>
          {task.enabled ? "Disable" : "Enable"}
        </Button>
        <Button size="sm" variant="danger" disabled={busy} onClick={onDelete} aria-label="Delete task">
          ✕
        </Button>
      </div>
    </li>
  );
}

/* -------------------------------- panel -------------------------------- */

export function AiPanel({ cameraId }: { cameraId: string }) {
  const tasks = usePoll(() => api.listAiTasks(cameraId), 8000, [cameraId]);
  const detections = usePoll(
    () => api.cameraDetections(cameraId, { limit: 50 }),
    5000,
    [cameraId],
  );
  // Engine status: the per-camera sampler tells us whether frames are actually flowing to detection.
  const samplers = usePoll(() => api.samplers(), 8000, []);

  // ---- Add form ----
  const [taskType, setTaskType] = useState("detection");
  const [model, setModel] = useState<string>(MODEL_OPTIONS[0].value);
  const [fps, setFps] = useState("5");
  const [width, setWidth] = useState("1280");
  const [profile, setProfile] = useState<StreamProfile>("sub");
  const [addError, setAddError] = useState<string | null>(null);
  const [presetNote, setPresetNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function addTask(e: FormEvent) {
    e.preventDefault();
    setAddError(null);
    setPresetNote(null);
    const type = taskType.trim();
    if (!type) {
      setAddError("Task type is required.");
      return;
    }
    setBusy(true);
    try {
      await api.createAiTask(cameraId, {
        task_type: type,
        fps: Number(fps) || undefined,
        width: Number(width) || undefined,
        stream_profile: profile,
        // The detection model lives in the task config; only detection tasks consume it.
        config: type === "detection" ? { weights: model } : undefined,
      });
      setTaskType("detection");
      await tasks.refresh();
    } catch (err) {
      setAddError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  // One-click preset: create a detection task (chosen model) AND a full-frame presence zone that
  // raises a warning when the chosen object class is present. Mirrors ZonePanel's createZone call.
  async function createPreset(kind: "people" | "vehicles") {
    setAddError(null);
    setPresetNote(null);
    setBusy(true);
    const name = kind === "people" ? "People (auto)" : "Vehicles (auto)";
    try {
      await api.createAiTask(cameraId, {
        task_type: "detection",
        fps: Number(fps) || undefined,
        width: Number(width) || undefined,
        stream_profile: profile,
        config: { weights: model },
      });
      await api.createZone(cameraId, {
        name,
        polygon: FULL_FRAME_POLYGON,
        kind: "region",
        severity: "warning",
        dwell_seconds: 5,
        labels: [...PRESET_LABELS[kind]],
      });
      setPresetNote(`Created a detection task and the "${name}" zone for this camera.`);
      await tasks.refresh();
    } catch (err) {
      setAddError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function toggle(task: AiTask) {
    setBusy(true);
    try {
      await api.updateAiTask(task.id, { enabled: !task.enabled });
      await tasks.refresh();
    } catch (err) {
      setAddError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function remove(task: AiTask) {
    if (!window.confirm(`Delete AI task "${task.task_type}"?`)) return;
    setBusy(true);
    try {
      await api.deleteAiTask(task.id);
      await tasks.refresh();
    } catch (err) {
      setAddError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  const taskList = tasks.data ?? [];
  const detList = detections.data ?? [];

  // A sampler that is sampling/connecting means the detection engine (AI worker) is attached and
  // frames are flowing for this camera. Absent / stopped / error => detection cannot run yet.
  const samplerActive = (samplers.data ?? []).some(
    (s) => s.camera_id === cameraId && (s.state === "sampling" || s.state === "connecting"),
  );

  // Overlay: draw boxes from the most recent detection batch, if fresh enough.
  const overlayBoxes = useMemo(() => {
    if (detList.length === 0) return [];
    const latest = detList[0].timestamp;
    const age = Date.now() - new Date(latest).getTime();
    if (!Number.isFinite(age) || age > OVERLAY_FRESH_MS) return [];
    return detList.filter((d) => d.timestamp === latest && isBbox(d.bbox)).slice(0, 32);
  }, [detList]);

  return (
    <>
      <Panel
        title="AI Perception"
        subtitle="Live sampled frame · detection overlay"
        padded={false}
        actions={
          <span className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
            {overlayBoxes.length > 0 ? `${overlayBoxes.length} boxes` : "auto-refresh 1s"}
          </span>
        }
      >
        <div className="p-3">
          <SampledFrame cameraId={cameraId} boxes={overlayBoxes} />
        </div>
      </Panel>

      <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
        <Panel
          title="AI Tasks"
          subtitle="Perception workloads"
          actions={
            <span className="font-mono text-[11px] tabular-nums text-fg-muted">
              {taskList.length}
            </span>
          }
        >
          {taskList.length === 0 ? (
            <p className="font-mono text-xs text-fg-muted">
              {tasks.error ?? "No AI tasks. Add one below to start sampling."}
            </p>
          ) : (
            <ul className="space-y-1.5">
              {taskList.map((t) => (
                <TaskRow
                  key={t.id}
                  task={t}
                  busy={busy}
                  onToggle={() => void toggle(t)}
                  onDelete={() => void remove(t)}
                />
              ))}
            </ul>
          )}

          {/* Engine-status hint: explains why detection may be idle (does not block creation). */}
          {!samplerActive && (
            <div className="mt-4 flex items-start gap-2 rounded-md border border-connecting/40 bg-connecting/10 px-3 py-2 font-mono text-[11px] leading-relaxed text-amber-200">
              <span
                className="mt-1 inline-flex h-1.5 w-1.5 shrink-0 rounded-full bg-connecting"
                aria-hidden="true"
              />
              <span>Detection engine not connected — start the AI worker to run detection.</span>
            </div>
          )}

          {/* One-click presets: a detection task + a full-frame presence zone, in a single click. */}
          <div className="mt-4 space-y-2 border-t border-line pt-4">
            <div className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
              Quick setup
            </div>
            <div className="flex flex-wrap gap-2">
              <Button
                variant="primary"
                className="flex-1"
                disabled={busy}
                onClick={() => void createPreset("people")}
              >
                Detect people
              </Button>
              <Button
                variant="primary"
                className="flex-1"
                disabled={busy}
                onClick={() => void createPreset("vehicles")}
              >
                Detect vehicles
              </Button>
            </div>
            <p className="text-[11px] leading-snug text-fg-muted">
              Adds a detection task and a full-frame zone that alerts when the chosen objects appear.
            </p>
            {presetNote && <p className="font-mono text-[11px] text-rec">{presetNote}</p>}
          </div>

          <form onSubmit={addTask} className="mt-4 space-y-3 border-t border-line pt-4">
            <div className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
              Add AI task
            </div>
            <Field label="Task type" htmlFor="ai-task-type" hint="Free-form: detection, anpr, tracking…">
              <Input
                id="ai-task-type"
                value={taskType}
                onChange={(e) => setTaskType(e.target.value)}
                placeholder="detection"
              />
            </Field>
            <Field
              label="Detection model"
              htmlFor="ai-model"
              hint="Applied to detection tasks (written to the task's weights)."
            >
              <Select id="ai-model" value={model} onChange={(e) => setModel(e.target.value)}>
                {MODEL_OPTIONS.map((m) => (
                  <option key={m.value} value={m.value}>
                    {m.label}
                  </option>
                ))}
              </Select>
            </Field>
            <div className="grid grid-cols-2 gap-3">
              <Field label="FPS" htmlFor="ai-fps">
                <Input
                  id="ai-fps"
                  type="number"
                  min={0.1}
                  max={30}
                  step={0.1}
                  value={fps}
                  onChange={(e) => setFps(e.target.value)}
                />
              </Field>
              <Field label="Width (px)" htmlFor="ai-width">
                <Input
                  id="ai-width"
                  type="number"
                  min={160}
                  max={3840}
                  step={16}
                  value={width}
                  onChange={(e) => setWidth(e.target.value)}
                />
              </Field>
            </div>
            <Field label="Stream profile" htmlFor="ai-profile">
              <Select
                id="ai-profile"
                value={profile}
                onChange={(e) => setProfile(e.target.value as StreamProfile)}
              >
                <option value="sub">sub (low-res, cheap)</option>
                <option value="main">main (full-res)</option>
              </Select>
            </Field>
            <Button type="submit" variant="primary" className="w-full" disabled={busy}>
              {busy ? "Working…" : "Add AI task"}
            </Button>
            {addError && <p className="font-mono text-xs text-danger">{addError}</p>}
          </form>
        </Panel>

        <Panel
          title="Recent Detections"
          subtitle="Worker-posted results"
          actions={
            <span className="font-mono text-[11px] tabular-nums text-fg-muted">
              {detList.length}
            </span>
          }
        >
          {detList.length === 0 ? (
            <p className="font-mono text-xs text-fg-muted">
              {detections.error ?? "No detections yet."}
            </p>
          ) : (
            <ul className="-mr-1 max-h-[420px] space-y-1.5 overflow-y-auto pr-1">
              {detList.map((d) => (
                <DetectionRow key={d.id} det={d} />
              ))}
            </ul>
          )}
        </Panel>
      </div>
    </>
  );
}

function DetectionRow({ det }: { det: Detection }) {
  return (
    <li className="flex items-center justify-between gap-2 rounded-md border border-line bg-canvas px-2.5 py-2">
      <div className="min-w-0">
        <div className="flex items-center gap-2">
          <span className="truncate text-xs font-semibold text-fg">{det.label ?? "object"}</span>
          <span className="shrink-0 font-mono text-[10px] uppercase tracking-micro text-fg-muted">
            {det.task_type}
          </span>
        </div>
        <div className="mt-0.5 font-mono text-[10px] text-fg-muted" title={formatClock(det.timestamp)}>
          {timeAgo(det.timestamp)}
          {det.track_id && (
            <>
              <span className="text-fg-muted/60"> · </span>
              <span>#{det.track_id}</span>
            </>
          )}
        </div>
      </div>
      {det.confidence != null && (
        <span className="shrink-0 rounded border border-accent/40 bg-accent/10 px-1.5 py-0.5 font-mono text-[10px] font-semibold tabular-nums text-accent-soft">
          {confidencePct(det.confidence)}
        </span>
      )}
    </li>
  );
}

export default AiPanel;
