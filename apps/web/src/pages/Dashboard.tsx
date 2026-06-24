import { useMemo, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { api } from "../lib/api";
import { usePoll } from "../lib/usePoll";
import type { CameraStatus } from "../lib/types";
import { CameraCard } from "../components/CameraCard";
import { Button, EmptyState, SectionLabel, Spinner, Stat } from "../components/ui";

// DVR-style multi-view layouts. `auto` keeps the responsive grid; the fixed layouts page through the
// cameras N-per-page (N = cells) like an NVR. `cols` maps to a static Tailwind class so the grid is a
// true fixed grid (not viewport-driven).
type LayoutKey = "auto" | "1" | "4" | "9" | "16";
const LAYOUTS: { key: LayoutKey; label: string; cells: number | null; colsClass: string }[] = [
  { key: "auto", label: "Auto", cells: null, colsClass: "" },
  { key: "1", label: "1", cells: 1, colsClass: "grid-cols-1" },
  { key: "4", label: "2×2", cells: 4, colsClass: "grid-cols-2" },
  { key: "9", label: "3×3", cells: 9, colsClass: "grid-cols-3" },
  { key: "16", label: "4×4", cells: 16, colsClass: "grid-cols-4" },
];
const LAYOUT_STORAGE_KEY = "heldar.wall.layout";

function isLayoutKey(v: string | null): v is LayoutKey {
  return v != null && LAYOUTS.some((l) => l.key === v);
}

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

  // Layout + page live in the URL (shareable, e2e-driveable); the chosen layout also persists in
  // localStorage so a fresh visit (no URL param) restores the operator's last view.
  const [params, setParams] = useSearchParams();
  const urlLayout = params.get("layout");
  const layout: LayoutKey = isLayoutKey(urlLayout)
    ? urlLayout
    : ((typeof localStorage !== "undefined" &&
        isLayoutKey(localStorage.getItem(LAYOUT_STORAGE_KEY)) &&
        (localStorage.getItem(LAYOUT_STORAGE_KEY) as LayoutKey)) ||
      "auto");
  const cfg = LAYOUTS.find((l) => l.key === layout) ?? LAYOUTS[0];

  const cells = cfg.cells; // null = auto (all on one page)
  const pageCount = cells ? Math.max(1, Math.ceil(list.length / cells)) : 1;
  const page = Math.min(Math.max(1, Number(params.get("page") ?? "1") || 1), pageCount);
  const visible = cells ? list.slice((page - 1) * cells, page * cells) : list;

  const setLayout = (key: LayoutKey) => {
    if (typeof localStorage !== "undefined") localStorage.setItem(LAYOUT_STORAGE_KEY, key);
    setParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        if (key === "auto") next.delete("layout");
        else next.set("layout", key);
        next.delete("page"); // reset to page 1 on layout change
        return next;
      },
      { replace: true },
    );
  };

  const goToPage = (p: number) => {
    setParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        if (p <= 1) next.delete("page");
        else next.set("page", String(p));
        return next;
      },
      { replace: true },
    );
  };

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
            {/* Layout picker (NVR multi-view) */}
            <div
              className="flex items-center gap-px overflow-hidden rounded-md border border-line bg-line"
              role="group"
              aria-label="Wall layout"
              data-testid="wall-layout-picker"
            >
              {LAYOUTS.map((l) => (
                <button
                  key={l.key}
                  type="button"
                  onClick={() => setLayout(l.key)}
                  aria-pressed={layout === l.key}
                  data-testid={`wall-layout-${l.key}`}
                  className={`px-2.5 py-1.5 font-mono text-xs tracking-micro transition-colors ${
                    layout === l.key
                      ? "bg-accent text-accent-ink"
                      : "bg-panel text-fg-secondary hover:text-fg"
                  }`}
                >
                  {l.label}
                </button>
              ))}
            </div>
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
          <>
            {/* Pager (only meaningful for fixed layouts that overflow one page) */}
            {pageCount > 1 && (
              <div
                className="mb-3 flex items-center justify-end gap-3"
                data-testid="wall-pager"
              >
                <button
                  type="button"
                  className="btn"
                  onClick={() => goToPage(page - 1)}
                  disabled={page <= 1}
                  aria-label="Previous page"
                  data-testid="wall-prev"
                >
                  ‹ Prev
                </button>
                <span
                  className="font-mono text-xs uppercase tracking-micro text-fg-secondary"
                  data-testid="wall-page-indicator"
                >
                  Page {page} / {pageCount}
                </span>
                <button
                  type="button"
                  className="btn"
                  onClick={() => goToPage(page + 1)}
                  disabled={page >= pageCount}
                  aria-label="Next page"
                  data-testid="wall-next"
                >
                  Next ›
                </button>
              </div>
            )}

            <div
              className={
                layout === "auto"
                  ? "stagger grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 2xl:grid-cols-5"
                  : `stagger grid gap-3 ${cfg.colsClass}`
              }
              data-testid="camera-grid"
              data-layout={layout}
              data-page={page}
            >
              {visible.map((cam) => (
                <CameraCard key={cam.id} camera={cam} status={statusById.get(cam.id)} />
              ))}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
