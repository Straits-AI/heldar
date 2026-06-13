import { useMemo } from "react";
import { Link } from "react-router-dom";
import { api } from "../lib/api";
import { usePoll } from "../lib/usePoll";
import type { CameraStatus } from "../lib/types";
import { CameraCard } from "../components/CameraCard";

export function Dashboard() {
  const cameras = usePoll(() => api.listCameras(), 10000);
  const health = usePoll(() => api.listHealth(), 5000);

  const statusById = useMemo(() => {
    const map = new Map<string, CameraStatus>();
    for (const s of health.data ?? []) map.set(s.camera_id, s);
    return map;
  }, [health.data]);

  const list = cameras.data ?? [];
  const recording = (health.data ?? []).filter((s) => s.state === "recording").length;

  return (
    <div className="mx-auto max-w-[1600px] px-4 py-5">
      <div className="mb-4 flex items-center justify-between">
        <div>
          <h1 className="text-lg font-semibold tracking-tight text-slate-100">Cameras</h1>
          <p className="text-xs text-slate-500">
            {list.length} registered · {recording} recording
          </p>
        </div>
        <div className="flex items-center gap-2">
          <button
            type="button"
            className="btn btn-sm"
            onClick={() => {
              void cameras.refresh();
              void health.refresh();
            }}
          >
            Refresh
          </button>
          <Link to="/cameras/new" className="btn btn-primary">
            + Add camera
          </Link>
        </div>
      </div>

      {cameras.error && (
        <div className="mb-4 rounded-md border border-red-500/40 bg-red-950/30 px-3 py-2 text-sm text-red-300">
          Failed to load cameras: {cameras.error}
        </div>
      )}

      {list.length === 0 && !cameras.loading ? (
        <div className="panel flex flex-col items-center justify-center px-6 py-16 text-center">
          <div className="text-base font-medium text-slate-200">No cameras yet</div>
          <p className="mt-1 max-w-sm text-sm text-slate-500">
            Register your first RTSP camera to start recording and build a timeline.
          </p>
          <Link to="/cameras/new" className="btn btn-primary mt-4">
            + Add camera
          </Link>
        </div>
      ) : (
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
          {list.map((cam) => (
            <CameraCard key={cam.id} camera={cam} status={statusById.get(cam.id)} />
          ))}
        </div>
      )}
    </div>
  );
}
