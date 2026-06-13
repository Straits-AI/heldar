import { useCallback, useEffect, useMemo, useState } from "react";
import type { FormEvent, ReactNode } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { api, ApiError } from "../lib/api";
import { usePoll } from "../lib/usePoll";
import type {
  CameraTestResult,
  ClipResult,
  LiveUrls,
  Severity,
  VisionEvent,
} from "../lib/types";
import { LiveView } from "../components/LiveView";
import { Timeline } from "../components/Timeline";
import { StatusBadge } from "../components/StatusBadge";
import {
  formatBytes,
  formatClock,
  formatDuration,
  formatTimeShort,
  isoToLocalInput,
  localInputToIso,
  timeAgo,
} from "../lib/format";

const RANGE_OPTIONS: { label: string; hours: number }[] = [
  { label: "1h", hours: 1 },
  { label: "6h", hours: 6 },
  { label: "24h", hours: 24 },
  { label: "3d", hours: 72 },
];

const SEVERITY_CLASS: Record<Severity, string> = {
  info: "text-slate-400 ring-slate-500/30",
  warning: "text-amber-300 ring-amber-500/30",
  critical: "text-red-300 ring-red-500/30",
};

function Panel({
  title,
  actions,
  children,
  className = "",
}: {
  title: string;
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section className={`panel ${className}`}>
      <div className="panel-head">
        <h2 className="panel-title">{title}</h2>
        {actions}
      </div>
      <div className="p-4">{children}</div>
    </section>
  );
}

function Field({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="flex flex-col">
      <span className="stat-k">{label}</span>
      <span className="stat-v break-words">{value}</span>
    </div>
  );
}

export function CameraDetail() {
  const { id = "" } = useParams();
  const navigate = useNavigate();

  const camera = usePoll(() => api.getCamera(id), 15000, [id]);
  const status = usePoll(() => api.cameraHealth(id), 4000, [id]);
  const segments = usePoll(() => api.listSegments(id, { limit: 50 }), 20000, [id]);
  const events = usePoll(() => api.listEvents({ camera_id: id, limit: 30 }), 15000, [id]);

  const [rangeHours, setRangeHours] = useState(24);
  const timeline = usePoll(
    () => {
      const to = new Date();
      const from = new Date(to.getTime() - rangeHours * 3600_000);
      return api.getTimeline(id, { from: from.toISOString(), to: to.toISOString() });
    },
    20000,
    [id, rangeHours],
  );

  // ---- Live view ----
  const [live, setLive] = useState<LiveUrls | null>(null);
  const [liveLoading, setLiveLoading] = useState(false);
  const [liveError, setLiveError] = useState<string | null>(null);

  const startLive = useCallback(async () => {
    setLiveLoading(true);
    setLiveError(null);
    try {
      setLive(await api.liveview(id));
    } catch (e) {
      setLiveError(e instanceof Error ? e.message : String(e));
    } finally {
      setLiveLoading(false);
    }
  }, [id]);

  useEffect(() => {
    setLive(null);
    void startLive();
  }, [startLive]);

  // ---- Selection / snapshot / clip ----
  const [selected, setSelected] = useState<string | null>(null);
  const [snapInput, setSnapInput] = useState("");
  const [snapSrc, setSnapSrc] = useState<string | null>(null);

  const [clipFrom, setClipFrom] = useState("");
  const [clipTo, setClipTo] = useState("");
  const [clipResult, setClipResult] = useState<ClipResult | null>(null);
  const [clipError, setClipError] = useState<string | null>(null);
  const [clipLoading, setClipLoading] = useState(false);

  const [playback, setPlayback] = useState<{ src: string; label: string } | null>(null);

  const handlePick = useCallback((iso: string) => {
    setSelected(iso);
    setSnapInput(isoToLocalInput(iso));
    const t = new Date(iso).getTime();
    setClipFrom(isoToLocalInput(new Date(t - 30_000).toISOString()));
    setClipTo(isoToLocalInput(new Date(t + 30_000).toISOString()));
  }, []);

  function captureSnapshot() {
    const iso = localInputToIso(snapInput);
    setSnapSrc(`${api.snapshotUrl(id, iso ?? undefined)}${iso ? "&" : "?"}_=${Date.now()}`);
  }

  async function submitClip(e: FormEvent) {
    e.preventDefault();
    setClipError(null);
    setClipResult(null);
    const from = localInputToIso(clipFrom);
    const to = localInputToIso(clipTo);
    if (!from || !to) {
      setClipError("Both start and end times are required.");
      return;
    }
    if (new Date(to) <= new Date(from)) {
      setClipError("End must be after start.");
      return;
    }
    setClipLoading(true);
    try {
      const result = await api.exportClip(id, from, to);
      setClipResult(result);
      setPlayback({ src: result.url, label: `Clip ${result.filename}` });
    } catch (err) {
      setClipError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setClipLoading(false);
    }
  }

  // ---- Camera actions ----
  const [testResult, setTestResult] = useState<CameraTestResult | null>(null);
  const [testing, setTesting] = useState(false);
  const [actionBusy, setActionBusy] = useState(false);

  async function runTest() {
    setTesting(true);
    setTestResult(null);
    try {
      setTestResult(await api.testCamera(id));
    } catch (e) {
      setTestResult({ reachable: false, url: "", error: e instanceof Error ? e.message : String(e) });
    } finally {
      setTesting(false);
    }
  }

  async function toggle(field: "enabled" | "record_enabled", value: boolean) {
    setActionBusy(true);
    try {
      await api.updateCamera(id, { [field]: value });
      await Promise.all([camera.refresh(), status.refresh()]);
    } finally {
      setActionBusy(false);
    }
  }

  async function remove() {
    const label = camera.data?.name ?? id;
    if (!window.confirm(`Delete camera "${label}" and all its recordings? This cannot be undone.`)) {
      return;
    }
    setActionBusy(true);
    try {
      await api.deleteCamera(id);
      navigate("/");
    } catch (e) {
      window.alert(e instanceof Error ? e.message : String(e));
      setActionBusy(false);
    }
  }

  const cam = camera.data;
  const st = status.data;
  const recentSegments = useMemo(
    () => [...(segments.data ?? [])].reverse(),
    [segments.data],
  );

  if (camera.error && !cam) {
    return (
      <div className="mx-auto max-w-3xl px-4 py-10 text-center">
        <p className="text-sm text-red-300">Failed to load camera: {camera.error}</p>
        <Link to="/" className="btn mt-4">
          ← Back to cameras
        </Link>
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-[1600px] px-4 py-5">
      {/* Header */}
      <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-3">
          <Link to="/" className="btn btn-sm" title="Back to cameras">
            ←
          </Link>
          <div>
            <div className="flex items-center gap-2">
              <h1 className="text-lg font-semibold tracking-tight text-slate-100">
                {cam?.name ?? id}
              </h1>
              <StatusBadge state={st?.state ?? (cam?.enabled ? "unknown" : "disabled")} />
            </div>
            <div className="mt-0.5 text-xs text-slate-500">
              <span className="font-mono">{id}</span>
              {cam ? ` · ${cam.vendor}` : ""}
              {cam?.model ? ` · ${cam.model}` : ""}
              {cam?.record_url_masked ? ` · ${cam.record_url_masked}` : ""}
            </div>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <button className="btn btn-sm" onClick={runTest} disabled={testing}>
            {testing ? "Testing…" : "Test stream"}
          </button>
          {cam && (
            <button
              className="btn btn-sm"
              disabled={actionBusy}
              onClick={() => toggle("record_enabled", !cam.record_enabled)}
            >
              {cam.record_enabled ? "Pause recording" : "Resume recording"}
            </button>
          )}
          {cam && (
            <button
              className="btn btn-sm"
              disabled={actionBusy}
              onClick={() => toggle("enabled", !cam.enabled)}
            >
              {cam.enabled ? "Disable" : "Enable"}
            </button>
          )}
          <button className="btn btn-danger btn-sm" disabled={actionBusy} onClick={remove}>
            Delete
          </button>
        </div>
      </div>

      {testResult && (
        <div
          className={`mb-4 rounded-md border px-3 py-2 text-sm ${
            testResult.reachable
              ? "border-emerald-500/40 bg-emerald-950/30 text-emerald-200"
              : "border-red-500/40 bg-red-950/30 text-red-200"
          }`}
        >
          {testResult.reachable ? (
            <span>
              Reachable · {testResult.codec ?? "?"} {testResult.width}×{testResult.height} ·{" "}
              <span className="font-mono">{testResult.url}</span>
            </span>
          ) : (
            <span>Unreachable — {testResult.error ?? "unknown error"}</span>
          )}
        </div>
      )}

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
        {/* Main column */}
        <div className="space-y-4 lg:col-span-2">
          <Panel
            title="Live view"
            actions={
              <button className="btn btn-sm" onClick={() => void startLive()} disabled={liveLoading}>
                {liveLoading ? "Connecting…" : live ? "Restart" : "Start"}
              </button>
            }
          >
            <LiveView hlsUrl={live?.hls_url} poster={api.snapshotUrl(id)} />
            {liveError && <p className="mt-2 text-xs text-red-300">{liveError}</p>}
            {live && (
              <div className="mt-2 grid grid-cols-1 gap-1 text-[11px] text-slate-500 sm:grid-cols-3">
                <span className="truncate">HLS: <span className="font-mono text-slate-400">{live.hls_url}</span></span>
                <span className="truncate">WebRTC: <span className="font-mono text-slate-400">{live.webrtc_url}</span></span>
                <span className="truncate">RTSP: <span className="font-mono text-slate-400">{live.rtsp_url}</span></span>
              </div>
            )}
          </Panel>

          {playback && (
            <Panel
              title="Recorded playback"
              actions={
                <div className="flex items-center gap-2">
                  <a className="btn btn-sm" href={playback.src} download>
                    Download
                  </a>
                  <button className="btn btn-sm" onClick={() => setPlayback(null)}>
                    Close
                  </button>
                </div>
              }
            >
              <video
                key={playback.src}
                className="aspect-video w-full rounded-md bg-black"
                src={playback.src}
                controls
                autoPlay
              />
              <p className="mt-2 truncate text-xs text-slate-500">{playback.label}</p>
            </Panel>
          )}

          <Panel
            title="Timeline"
            actions={
              <div className="flex gap-1">
                {RANGE_OPTIONS.map((opt) => (
                  <button
                    key={opt.hours}
                    className={`btn btn-sm ${rangeHours === opt.hours ? "btn-primary" : ""}`}
                    onClick={() => setRangeHours(opt.hours)}
                  >
                    {opt.label}
                  </button>
                ))}
              </div>
            }
          >
            {timeline.data ? (
              <Timeline
                timeline={timeline.data}
                from={timeline.data.from}
                to={timeline.data.to}
                selected={selected}
                onPick={handlePick}
              />
            ) : (
              <div className="py-6 text-center text-sm text-slate-500">
                {timeline.error ?? "Loading timeline…"}
              </div>
            )}
          </Panel>

          <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
            <Panel title="Snapshot">
              <div className="flex flex-wrap items-end gap-2">
                <div className="flex-1">
                  <label className="label" htmlFor="snap-at">
                    At time (blank = live)
                  </label>
                  <input
                    id="snap-at"
                    type="datetime-local"
                    step={1}
                    className="input"
                    value={snapInput}
                    onChange={(e) => setSnapInput(e.target.value)}
                  />
                </div>
                <button className="btn" onClick={captureSnapshot}>
                  Capture
                </button>
              </div>
              {snapSrc && (
                <div className="mt-3">
                  <img
                    src={snapSrc}
                    alt="Snapshot"
                    className="w-full rounded-md border border-line bg-black"
                  />
                  <a className="btn btn-sm mt-2" href={snapSrc} target="_blank" rel="noreferrer">
                    Open full size
                  </a>
                </div>
              )}
            </Panel>

            <Panel title="Export evidence clip">
              <form onSubmit={submitClip} className="space-y-2">
                <div>
                  <label className="label" htmlFor="clip-from">
                    From
                  </label>
                  <input
                    id="clip-from"
                    type="datetime-local"
                    step={1}
                    className="input"
                    value={clipFrom}
                    onChange={(e) => setClipFrom(e.target.value)}
                    required
                  />
                </div>
                <div>
                  <label className="label" htmlFor="clip-to">
                    To
                  </label>
                  <input
                    id="clip-to"
                    type="datetime-local"
                    step={1}
                    className="input"
                    value={clipTo}
                    onChange={(e) => setClipTo(e.target.value)}
                    required
                  />
                </div>
                <button type="submit" className="btn btn-primary w-full" disabled={clipLoading}>
                  {clipLoading ? "Exporting…" : "Export clip"}
                </button>
              </form>
              {clipError && <p className="mt-2 text-xs text-red-300">{clipError}</p>}
              {clipResult && (
                <div className="mt-3 rounded-md border border-line bg-ink p-2 text-xs text-slate-300">
                  <div className="mb-1 font-medium text-slate-100">{clipResult.filename}</div>
                  <div className="text-slate-500">
                    {formatDuration(clipResult.requested_seconds)} · {formatBytes(clipResult.size_bytes)} ·{" "}
                    {clipResult.segment_count} segments
                  </div>
                  <div className="mt-2 flex gap-2">
                    <a className="btn btn-sm" href={clipResult.url} download>
                      Download
                    </a>
                    <button
                      className="btn btn-sm"
                      type="button"
                      onClick={() => setPlayback({ src: clipResult.url, label: clipResult.filename })}
                    >
                      Play
                    </button>
                  </div>
                </div>
              )}
            </Panel>
          </div>
        </div>

        {/* Side column */}
        <div className="space-y-4">
          <Panel title="Health">
            {st ? (
              <div className="grid grid-cols-2 gap-3">
                <Field label="State" value={<StatusBadge state={st.state} />} />
                <Field label="FPS observed" value={st.fps_observed != null ? st.fps_observed.toFixed(1) : "—"} />
                <Field
                  label="Bitrate"
                  value={st.bitrate_kbps != null ? `${st.bitrate_kbps.toFixed(0)} kbps` : "—"}
                />
                <Field label="Reconnects" value={st.reconnect_count} />
                <Field label="Segments written" value={st.segments_written} />
                <Field label="Recorder PID" value={st.recorder_pid ?? "—"} />
                <Field label="Last segment" value={timeAgo(st.last_segment_at)} />
                <Field label="Last started" value={timeAgo(st.last_started_at)} />
                <div className="col-span-2">
                  <Field label="Updated" value={formatClock(st.updated_at)} />
                </div>
                {st.last_error && (
                  <div className="col-span-2 rounded-md border border-red-500/30 bg-red-950/20 px-2 py-1.5 text-xs text-red-300">
                    {st.last_error}
                  </div>
                )}
              </div>
            ) : (
              <p className="text-sm text-slate-500">{status.error ?? "No health data yet."}</p>
            )}
            {cam && (
              <div className="mt-3 grid grid-cols-2 gap-3 border-t border-line pt-3">
                <Field label="Record stream" value={cam.record_stream} />
                <Field label="Segment length" value={`${cam.segment_seconds}s`} />
                <Field label="Retention" value={`${cam.retention_hours}h`} />
                <Field label="Codec" value={cam.codec ?? "—"} />
              </div>
            )}
          </Panel>

          <Panel
            title="Recent segments"
            actions={<span className="text-[11px] text-slate-500">{recentSegments.length}</span>}
          >
            {recentSegments.length === 0 ? (
              <p className="text-sm text-slate-500">No recorded segments yet.</p>
            ) : (
              <ul className="max-h-96 space-y-1 overflow-y-auto pr-1">
                {recentSegments.map((seg) => (
                  <li
                    key={seg.id}
                    className="flex items-center justify-between gap-2 rounded-md border border-line bg-ink px-2 py-1.5 text-xs"
                  >
                    <div className="min-w-0">
                      <div className="font-mono text-slate-300">
                        {formatTimeShort(seg.start_time)} → {formatTimeShort(seg.end_time)}
                      </div>
                      <div className="text-slate-500">
                        {formatDuration(seg.duration_s)} · {formatBytes(seg.size_bytes)}
                        {seg.locked ? " · 🔒" : ""}
                      </div>
                    </div>
                    <button
                      className="btn btn-sm shrink-0"
                      onClick={() =>
                        setPlayback({
                          src: seg.url,
                          label: `Segment ${formatClock(seg.start_time)}`,
                        })
                      }
                    >
                      Play
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </Panel>

          <Panel title="Recent events">
            {(events.data ?? []).length === 0 ? (
              <p className="text-sm text-slate-500">No events.</p>
            ) : (
              <ul className="max-h-96 space-y-1.5 overflow-y-auto pr-1">
                {(events.data ?? []).map((ev) => (
                  <EventRow key={ev.id} ev={ev} />
                ))}
              </ul>
            )}
          </Panel>
        </div>
      </div>
    </div>
  );
}

function EventRow({ ev }: { ev: VisionEvent }) {
  const payloadKeys = Object.keys(ev.payload ?? {});
  return (
    <li className="rounded-md border border-line bg-ink px-2 py-1.5 text-xs">
      <div className="flex items-center justify-between gap-2">
        <span className="truncate font-medium text-slate-200">{ev.event_type}</span>
        <span
          className={`shrink-0 rounded-full px-1.5 py-0.5 text-[10px] font-semibold uppercase ring-1 ring-inset ${
            SEVERITY_CLASS[ev.severity] ?? SEVERITY_CLASS.info
          }`}
        >
          {ev.severity}
        </span>
      </div>
      <div className="mt-0.5 text-slate-500">{formatClock(ev.timestamp)}</div>
      {payloadKeys.length > 0 && (
        <div className="mt-1 truncate font-mono text-[11px] text-slate-600" title={JSON.stringify(ev.payload)}>
          {JSON.stringify(ev.payload)}
        </div>
      )}
    </li>
  );
}
