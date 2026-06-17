import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { api } from "../lib/api";
import { usePoll } from "../lib/usePoll";
import type { CameraStatus } from "../lib/types";
import { CameraCard } from "../components/CameraCard";
import { Button, EmptyState, SectionLabel, Spinner, Stat } from "../components/ui";

export function Dashboard() {
  const cameras = usePoll(() => api.listCameras(), 10000);
  const health = usePoll(() => api.listHealth(), 5000);

  const statusById = useMemo(() => {
    const map = new Map<string, CameraStatus>();
    for (const s of health.data ?? []) map.set(s.camera_id, s);
    return map;
  }, [health.data]);

  const list = cameras.data ?? [];
  const statuses = health.data ?? [];
  const recording = statuses.filter((s) => s.state === "recording").length;
  const offline = statuses.filter((s) => s.state === "offline").length;
  const errored = statuses.filter((s) => s.state === "error").length;

  const [refreshing, setRefreshing] = useState(false);
  const refresh = () => {
    setRefreshing(true);
    void Promise.all([cameras.refresh(), health.refresh()]).finally(() =>
      setRefreshing(false),
    );
  };

  const showEmpty = list.length === 0 && !cameras.loading;
  const showLoading = list.length === 0 && cameras.loading;

  return (
    <div className="mx-auto max-w-[1600px] px-4 py-6 sm:px-6">
      {/* ---- Wall header ---- */}
      <header className="animate-rise">
        <div className="flex flex-wrap items-end justify-between gap-4">
          <div className="min-w-0">
            <SectionLabel>Operations · Live</SectionLabel>
            <h1 className="mt-1 font-display text-2xl font-extrabold tracking-tight text-fg">
              Camera Wall
            </h1>
          </div>
          <div className="flex items-center gap-2">
            <Button onClick={refresh} disabled={refreshing} aria-label="Refresh wall">
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
            <Link to="/cameras/new" className="btn btn-primary">
              <svg
                viewBox="0 0 16 16"
                width="14"
                height="14"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.5"
                strokeLinecap="round"
                aria-hidden="true"
              >
                <path d="M8 3.5v9M3.5 8h9" />
              </svg>
              Add camera
            </Link>
          </div>
        </div>

        {/* Aggregate telemetry */}
        <div className="mt-4 grid grid-cols-2 gap-px overflow-hidden rounded-panel border border-line bg-line sm:grid-cols-4">
          <div className="bg-panel px-4 py-3">
            <Stat label="Cameras" value={list.length} />
          </div>
          <div className="bg-panel px-4 py-3">
            <Stat label="Recording" value={recording} tone="good" />
          </div>
          <div className="bg-panel px-4 py-3">
            <Stat label="Offline" value={offline} tone={offline > 0 ? "warn" : "default"} />
          </div>
          <div className="bg-panel px-4 py-3">
            <Stat label="Errors" value={errored} tone={errored > 0 ? "bad" : "default"} />
          </div>
        </div>
      </header>

      {/* ---- Body ---- */}
      <div className="mt-5">
        {cameras.error && (
          <div className="mb-4 flex items-center gap-2 rounded-md border border-danger/40 bg-danger/10 px-3 py-2 font-mono text-xs text-red-300 animate-rise">
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
              className="shrink-0"
            >
              <path d="M8 1.5l6.5 11.5H1.5z" />
              <path d="M8 6.5v3.5" />
              <path d="M8 11.6v.4" />
            </svg>
            Failed to load cameras: {cameras.error}
          </div>
        )}

        {showLoading ? (
          <div className="flex items-center justify-center gap-3 rounded-panel border border-line bg-panel py-16 text-fg-secondary animate-rise">
            <Spinner />
            <span className="font-mono text-xs uppercase tracking-micro">
              Loading camera wall…
            </span>
          </div>
        ) : showEmpty ? (
          <div className="animate-rise">
            <EmptyState
              title="No cameras registered"
              hint="Register an RTSP camera or scan your network to begin recording and build the wall."
              action={
                <div className="flex items-center gap-2">
                  <Link to="/discover" className="btn">
                    Discover network
                  </Link>
                  <Link to="/cameras/new" className="btn btn-primary">
                    + Add camera
                  </Link>
                </div>
              }
            />
          </div>
        ) : (
          <div className="stagger grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 2xl:grid-cols-5">
            {list.map((cam) => (
              <CameraCard key={cam.id} camera={cam} status={statusById.get(cam.id)} />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
