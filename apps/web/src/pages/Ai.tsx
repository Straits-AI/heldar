// Heldar Core — AI perception console.
// One screen to see the sampler fleet (state + effective fps) and every enabled AI task,
// plus a short explainer of the global fps budget that drives backpressure.

import { useMemo, useState } from "react";
import type { ReactNode } from "react";
import { Link } from "react-router-dom";
import { api } from "../lib/api";
import { usePoll } from "../lib/usePoll";
import type { SamplerInfo, WorkerTask } from "../lib/types";
import {
  Button,
  EmptyState,
  Panel,
  SectionLabel,
  Spinner,
  Stat,
  StatusLed,
  StatusPill,
  cx,
} from "../components/ui";

// Sampler states are distinct from camera/recorder states; map onto the LED palette.
const SAMPLER_TO_CAM: Record<string, string> = {
  sampling: "recording",
  connecting: "connecting",
  offline: "offline",
  error: "error",
  stopped: "disabled",
};

function camState(s: string): string {
  return SAMPLER_TO_CAM[s] ?? "unknown";
}

/* ------------------------------ table bits ----------------------------- */

function Th({ children, className }: { children?: ReactNode; className?: string }) {
  return (
    <th
      className={cx(
        "whitespace-nowrap px-3 py-2 text-left font-mono text-[10px] font-medium uppercase tracking-micro text-fg-muted",
        className,
      )}
    >
      {children}
    </th>
  );
}

function CameraCell({ cameraId, name }: { cameraId: string; name: string }) {
  return (
    <Link
      to={`/cameras/${encodeURIComponent(cameraId)}`}
      className="group flex min-w-0 items-center gap-2.5"
    >
      <span className="min-w-0">
        <span className="block truncate text-sm font-medium text-fg group-hover:text-accent-soft">
          {name}
        </span>
        <span className="block truncate font-mono text-[10px] text-fg-muted">{cameraId}</span>
      </span>
    </Link>
  );
}

/* ------------------------------- panels -------------------------------- */

function PanelStatus({
  loading,
  error,
  label,
}: {
  loading: boolean;
  error: string | null;
  label: string;
}) {
  if (error) {
    return (
      <div className="flex items-center gap-2 rounded-md border border-danger/40 bg-danger/10 px-3 py-2 font-mono text-xs text-red-300">
        Failed to load {label}: {error}
      </div>
    );
  }
  if (loading) {
    return (
      <div className="flex items-center gap-2 font-mono text-xs text-fg-muted">
        <Spinner size={14} /> Loading {label}…
      </div>
    );
  }
  return <p className="font-mono text-xs text-fg-muted">No {label} available.</p>;
}

function SamplersPanel({
  samplers,
  nameById,
  loading,
  error,
}: {
  samplers: SamplerInfo[] | null;
  nameById: Map<string, string>;
  loading: boolean;
  error: string | null;
}) {
  const rows = samplers ?? [];
  return (
    <Panel
      title="Samplers"
      subtitle="Per-camera frame sampling"
      padded={false}
      actions={
        rows.length > 0 ? (
          <span className="font-mono text-[11px] tabular-nums text-fg-muted">{rows.length}</span>
        ) : undefined
      }
    >
      {rows.length === 0 ? (
        <div className="p-4">
          {loading && !samplers ? (
            <PanelStatus loading={loading} error={error} label="samplers" />
          ) : (
            <EmptyState
              title="No active samplers"
              hint="A sampler runs only while a camera has at least one enabled AI task. Enable a task on a camera to start sampling."
            />
          )}
        </div>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full border-collapse">
            <thead>
              <tr>
                <Th>Camera</Th>
                <Th>State</Th>
                <Th className="text-right">Effective fps</Th>
              </tr>
            </thead>
            <tbody>
              {rows.map((s) => (
                <tr
                  key={s.camera_id}
                  className="border-t border-line transition-colors duration-150 hover:bg-raised/40"
                >
                  <td className="px-3 py-2.5">
                    <div className="flex items-center gap-2.5">
                      <StatusLed state={camState(s.state)} />
                      <CameraCell
                        cameraId={s.camera_id}
                        name={nameById.get(s.camera_id) ?? s.camera_id}
                      />
                    </div>
                  </td>
                  <td className="px-3 py-2.5">
                    <StatusPill state={camState(s.state)} label={s.state} />
                  </td>
                  <td className="px-3 py-2.5 text-right font-mono text-xs tabular-nums text-fg-secondary">
                    {s.fps.toFixed(1)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Panel>
  );
}

function TasksPanel({
  tasks,
  nameById,
  loading,
  error,
}: {
  tasks: WorkerTask[] | null;
  nameById: Map<string, string>;
  loading: boolean;
  error: string | null;
}) {
  const rows = tasks ?? [];
  return (
    <Panel
      title="Enabled Tasks"
      subtitle="Worker discovery view"
      padded={false}
      actions={
        rows.length > 0 ? (
          <span className="font-mono text-[11px] tabular-nums text-fg-muted">{rows.length}</span>
        ) : undefined
      }
    >
      {rows.length === 0 ? (
        <div className="p-4">
          {loading && !tasks ? (
            <PanelStatus loading={loading} error={error} label="tasks" />
          ) : (
            <EmptyState
              title="No enabled AI tasks"
              hint="Open a camera and add an AI task in the AI Perception panel. Enabled tasks on enabled cameras show up here for workers to pick up."
            />
          )}
        </div>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full border-collapse">
            <thead>
              <tr>
                <Th>Camera</Th>
                <Th>Task type</Th>
                <Th>Stream</Th>
                <Th className="text-right">fps</Th>
                <Th className="text-right">Width</Th>
                <Th>Frame</Th>
              </tr>
            </thead>
            <tbody>
              {rows.map((t) => (
                <tr
                  key={t.id}
                  className="border-t border-line transition-colors duration-150 hover:bg-raised/40"
                >
                  <td className="px-3 py-2.5">
                    <CameraCell cameraId={t.camera_id} name={nameById.get(t.camera_id) ?? t.camera_id} />
                  </td>
                  <td className="px-3 py-2.5">
                    <span className="font-mono text-xs font-semibold text-fg">{t.task_type}</span>
                  </td>
                  <td className="px-3 py-2.5 font-mono text-[11px] uppercase tracking-micro text-fg-secondary">
                    {t.stream_profile}
                  </td>
                  <td className="px-3 py-2.5 text-right font-mono text-xs tabular-nums text-fg-secondary">
                    {t.fps.toFixed(1)}
                  </td>
                  <td className="px-3 py-2.5 text-right font-mono text-xs tabular-nums text-fg-secondary">
                    {t.width}
                  </td>
                  <td className="px-3 py-2.5">
                    <a
                      href={t.frame_url}
                      target="_blank"
                      rel="noreferrer"
                      className="font-mono text-[11px] text-accent-soft hover:text-accent"
                    >
                      frame ↗
                    </a>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Panel>
  );
}

/* -------------------------------- page --------------------------------- */

export function Ai() {
  const samplers = usePoll(() => api.samplers(), 4000);
  const tasks = usePoll(() => api.aiTasks(), 8000);
  const cameras = usePoll(() => api.listCameras(), 30000);

  const [refreshing, setRefreshing] = useState(false);
  const refresh = () => {
    setRefreshing(true);
    void Promise.all([samplers.refresh(), tasks.refresh(), cameras.refresh()]).finally(() =>
      setRefreshing(false),
    );
  };

  const nameById = useMemo(() => {
    const map = new Map<string, string>();
    for (const c of cameras.data ?? []) map.set(c.id, c.name);
    return map;
  }, [cameras.data]);

  const samplerList = samplers.data ?? [];
  const activeSamplers = samplerList.filter((s) => s.state === "sampling").length;
  const totalFps = samplerList
    .filter((s) => s.state === "sampling")
    .reduce((sum, s) => sum + s.fps, 0);
  const enabledTasks = (tasks.data ?? []).length;

  return (
    <div className="mx-auto max-w-[1600px] px-4 py-6 sm:px-6">
      {/* ---- Header ---- */}
      <header className="animate-rise">
        <div className="flex flex-wrap items-end justify-between gap-4">
          <div className="min-w-0">
            <SectionLabel>Operations · AI</SectionLabel>
            <h1 className="mt-1 font-display text-2xl font-extrabold tracking-tight text-fg">
              AI Perception
            </h1>
          </div>
          <Button onClick={refresh} disabled={refreshing} aria-label="Refresh AI">
            {refreshing ? (
              <Spinner size={14} />
            ) : (
              <svg
                viewBox="0 0 16 16"
                width="14"
                height="14"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.5"
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden="true"
              >
                <path d="M13.5 8a5.5 5.5 0 1 1-1.6-3.9" />
                <path d="M13.5 2.5V5H11" />
              </svg>
            )}
            <span>Refresh</span>
          </Button>
        </div>

        {/* Aggregate telemetry */}
        <div className="mt-4 grid grid-cols-2 gap-px overflow-hidden rounded-panel border border-line bg-line sm:grid-cols-4">
          <div className="bg-panel px-4 py-3">
            <Stat label="Active samplers" value={activeSamplers} tone={activeSamplers > 0 ? "good" : "default"} />
          </div>
          <div className="bg-panel px-4 py-3">
            <Stat label="Sampling fps" value={totalFps.toFixed(1)} unit="fps" />
          </div>
          <div className="bg-panel px-4 py-3">
            <Stat label="Enabled tasks" value={enabledTasks} />
          </div>
          <div className="bg-panel px-4 py-3">
            <Stat label="Samplers" value={samplerList.length} />
          </div>
        </div>
      </header>

      {/* ---- fps budget explainer ---- */}
      <div className="mt-5 flex items-start gap-3 rounded-panel border border-line bg-panel px-4 py-3 animate-rise">
        <svg
          viewBox="0 0 20 20"
          className="mt-0.5 h-4 w-4 shrink-0 text-accent"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.6"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          <circle cx="10" cy="10" r="7.5" />
          <path d="M10 9v4" />
          <path d="M10 6.6v.4" />
        </svg>
        <p className="font-mono text-[11px] leading-relaxed text-fg-secondary">
          The sampler decodes each AI-enabled camera&apos;s stream at a budgeted frame rate and writes
          the latest JPEG to disk. A global budget{" "}
          <code className="text-accent-soft">HELDAR_AI_MAX_TOTAL_FPS</code> (default 40 fps) is split
          across active cameras — when many cameras run AI, each camera&apos;s effective fps is reduced
          (backpressure) so the host is never overloaded. Workers pull the latest frame over HTTP and{" "}
          <span className="text-fg">never touch RTSP</span>.
        </p>
      </div>

      {/* ---- Body ---- */}
      <div className="mt-5 grid grid-cols-1 gap-4 lg:grid-cols-2">
        <div className="stagger space-y-4">
          <SamplersPanel
            samplers={samplers.data}
            nameById={nameById}
            loading={samplers.loading}
            error={samplers.error}
          />
        </div>
        <div className="stagger space-y-4">
          <TasksPanel
            tasks={tasks.data}
            nameById={nameById}
            loading={tasks.loading}
            error={tasks.error}
          />
        </div>
      </div>
    </div>
  );
}

export default Ai;
