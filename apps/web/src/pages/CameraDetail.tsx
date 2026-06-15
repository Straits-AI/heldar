import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { FormEvent, ReactNode } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { api, ApiError } from "../lib/api";
import { usePoll } from "../lib/usePoll";
import type {
  CameraTestResult,
  ClipResult,
  LiveUrls,
  Principal,
  RecordTriggerResult,
  SegmentView,
  Severity,
  VisionEvent,
} from "../lib/types";
import { LiveView } from "../components/LiveView";
import { Timeline } from "../components/Timeline";
import { AiPanel } from "../components/AiPanel";
import { ZonePanel } from "../components/ZonePanel";
import {
  PlaybackSessionPanel,
  PtzPanel,
  RecordingGapsPanel,
  RecordingSchedulePanel,
  RecordingSettingsPanel,
  SnapshotSchedulesPanel,
} from "../components/RecordingPanels";
import {
  Button,
  EmptyState,
  Field,
  Input,
  Panel,
  Spinner,
  Stat,
  StatusPill,
} from "../components/ui";
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

const SEVERITY_COLOR: Record<Severity, string> = {
  info: "#71717a",
  warning: "#fbbf24",
  critical: "#ef4444",
};

// Anchor styled like a default <Button size="sm"> (anchors can't be Buttons).
const ANCHOR_BTN =
  "inline-flex items-center justify-center gap-1.5 rounded-md border border-line bg-raised px-2.5 py-1 text-xs font-medium text-fg transition-colors duration-150 hover:border-[#34373e] hover:bg-[#23262c] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-canvas";

/* ------------------------------ small bits ------------------------------ */

function Dot() {
  return <span className="text-fg-muted/60">·</span>;
}

function ArrowLeftIcon({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" className={className}>
      <path d="M9.5 3.5 5 8l4.5 4.5" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

function LockIcon({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" className={className}>
      <rect x="3.5" y="7" width="9" height="6.5" rx="1.2" />
      <path d="M5.5 7V5a2.5 2.5 0 0 1 5 0v2" strokeLinecap="round" />
    </svg>
  );
}

/** Compact mono key/value row for dense config / telemetry. */
function Meta({ label, value }: { label: ReactNode; value: ReactNode }) {
  return (
    <div className="flex items-baseline justify-between gap-3 py-1">
      <span className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">{label}</span>
      <span className="break-words text-right font-mono text-xs text-fg-secondary">{value}</span>
    </div>
  );
}

/* -------------------------------- page ---------------------------------- */

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

  // Guards for async setState: `mountedRef` blocks updates after unmount; `idRef` (always the latest
  // camera id) lets an in-flight response for a previous camera be discarded when the user navigates
  // away rapidly (A→B→C), so a slow response for A can't overwrite C's view.
  const mountedRef = useRef(true);
  const idRef = useRef(id);
  idRef.current = id;
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);
  const liveForThisCamera = useCallback(() => mountedRef.current && idRef.current === id, [id]);

  // ---- Principal / manager gating ----
  // Mutations on the new DVR surfaces (evidence lock, record-trigger, settings, schedules, PTZ,
  // gap retry) are manager+. When auth is disabled the server returns the `system` admin principal;
  // when unauthenticated the controls stay gated off. Read-only views are never blocked.
  const [principal, setPrincipal] = useState<Principal | null>(null);
  useEffect(() => {
    let alive = true;
    api
      .me()
      .then((p) => {
        if (alive) setPrincipal(p);
      })
      .catch(() => {
        /* unauthenticated / auth off — leave principal null (controls gated off) */
      });
    return () => {
      alive = false;
    };
  }, []);
  const canManage = principal?.role === "admin" || principal?.role === "manager";

  // ---- Live view ----
  const [live, setLive] = useState<LiveUrls | null>(null);
  const [liveLoading, setLiveLoading] = useState(false);
  const [liveError, setLiveError] = useState<string | null>(null);

  const startLive = useCallback(async () => {
    setLiveLoading(true);
    setLiveError(null);
    try {
      const urls = await api.liveview(id);
      if (liveForThisCamera()) setLive(urls);
    } catch (e) {
      if (liveForThisCamera()) setLiveError(e instanceof Error ? e.message : String(e));
    } finally {
      if (liveForThisCamera()) setLiveLoading(false);
    }
  }, [id, liveForThisCamera]);

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
      if (!liveForThisCamera()) return; // navigated away / unmounted — drop the stale result
      setClipResult(result);
      setPlayback({ src: result.url, label: `Clip ${result.filename}` });
    } catch (err) {
      if (liveForThisCamera()) setClipError(err instanceof ApiError ? err.message : String(err));
    } finally {
      if (liveForThisCamera()) setClipLoading(false);
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
      const r = await api.testCamera(id);
      if (liveForThisCamera()) setTestResult(r);
    } catch (e) {
      if (liveForThisCamera())
        setTestResult({ reachable: false, url: "", error: e instanceof Error ? e.message : String(e) });
    } finally {
      if (liveForThisCamera()) setTesting(false);
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

  // ---- Evidence lock / incident tag (per segment) + manual record trigger ----
  const [segBusy, setSegBusy] = useState<string | null>(null);
  const [trigBusy, setTrigBusy] = useState(false);
  const [trigResult, setTrigResult] = useState<RecordTriggerResult | null>(null);

  async function toggleEvidence(seg: SegmentView) {
    if (!seg.evidence_locked) {
      const inc = window.prompt(
        "Lock this segment as evidence (never pruned by retention). Optional incident id (blank = none):",
        seg.incident_id ?? "",
      );
      if (inc === null) return; // cancelled
      setSegBusy(seg.id);
      try {
        await api.lockSegmentEvidence(seg.id, inc.trim() ? inc.trim() : null);
        await segments.refresh();
      } catch (e) {
        window.alert(e instanceof ApiError ? e.message : String(e));
      } finally {
        setSegBusy(null);
      }
      return;
    }
    setSegBusy(seg.id);
    try {
      await api.unlockSegmentEvidence(seg.id);
      await segments.refresh();
    } catch (e) {
      window.alert(e instanceof ApiError ? e.message : String(e));
    } finally {
      setSegBusy(null);
    }
  }

  async function recordNow() {
    setTrigBusy(true);
    try {
      const r = await api.triggerRecord(id);
      if (liveForThisCamera()) setTrigResult(r);
      await segments.refresh();
    } catch (e) {
      window.alert(e instanceof ApiError ? e.message : String(e));
    } finally {
      if (liveForThisCamera()) setTrigBusy(false);
    }
  }

  const cam = camera.data;
  const st = status.data;
  const headerState = st?.state ?? (cam?.enabled ? "unknown" : "disabled");
  const isEventMode =
    cam?.record_mode === "event" || cam?.record_mode === "scheduled_event";
  const isScheduledMode =
    cam?.record_mode === "scheduled" || cam?.record_mode === "scheduled_event";
  const recentSegments = useMemo(
    () => [...(segments.data ?? [])].reverse(),
    [segments.data],
  );

  if (camera.error && !cam) {
    return (
      <div className="mx-auto max-w-2xl px-4 py-16">
        <EmptyState
          title="Camera unavailable"
          hint={`Failed to load camera: ${camera.error}`}
          action={
            <Link to="/" className={ANCHOR_BTN}>
              <ArrowLeftIcon className="h-3.5 w-3.5" />
              Back to wall
            </Link>
          }
        />
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-[1600px] animate-rise px-4 py-5 sm:px-6">
      {/* Header */}
      <div className="mb-5 flex flex-wrap items-start justify-between gap-4">
        <div className="flex items-start gap-3">
          <Link
            to="/"
            title="Back to wall"
            className="mt-0.5 inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-md border border-line bg-raised text-fg-secondary transition-colors duration-150 hover:border-[#34373e] hover:bg-[#23262c] hover:text-fg"
          >
            <ArrowLeftIcon className="h-4 w-4" />
          </Link>
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2.5">
              <h1 className="truncate font-display text-xl font-extrabold tracking-tight text-fg">
                {cam?.name ?? id}
              </h1>
              <StatusPill state={headerState} />
            </div>
            <div className="mt-1.5 flex flex-wrap items-center gap-x-2 gap-y-1 font-mono text-[11px] text-fg-muted">
              <span className="text-fg-secondary">{id}</span>
              {cam && (
                <>
                  <Dot />
                  <span className="uppercase">{cam.vendor}</span>
                </>
              )}
              {cam?.model && (
                <>
                  <Dot />
                  <span>{cam.model}</span>
                </>
              )}
              {cam?.record_url_masked && (
                <>
                  <Dot />
                  <span className="max-w-[320px] truncate">{cam.record_url_masked}</span>
                </>
              )}
            </div>
          </div>
        </div>
      </div>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
        {/* Main column */}
        <div className="stagger space-y-4 lg:col-span-2">
          {/* Hero: live player */}
          <Panel
            title="Live View"
            subtitle="Low-latency HLS"
            padded={false}
            actions={
              <Button size="sm" onClick={() => void startLive()} disabled={liveLoading}>
                {liveLoading ? "Connecting…" : live ? "Restart" : "Start"}
              </Button>
            }
          >
            <div className="p-3">
              <LiveView
                hlsUrl={live?.hls_url}
                poster={api.snapshotUrl(id)}
                name={cam?.name ?? id}
                state={headerState}
                loading={liveLoading}
                onRetry={() => void startLive()}
              />
              {liveError && (
                <p className="mt-2 font-mono text-xs text-danger">{liveError}</p>
              )}
              {live && (
                <div className="mt-3 grid grid-cols-1 gap-2 border-t border-line pt-3 sm:grid-cols-3">
                  {[
                    { k: "HLS", v: live.hls_url },
                    { k: "WebRTC", v: live.webrtc_url },
                    { k: "RTSP", v: live.rtsp_url },
                  ].map(({ k, v }) => (
                    <div key={k} className="min-w-0">
                      <div className="font-mono text-[9px] uppercase tracking-micro text-fg-muted">
                        {k}
                      </div>
                      <div className="truncate font-mono text-[11px] text-fg-secondary" title={v}>
                        {v}
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>
          </Panel>

          {playback && (
            <Panel
              title="Clip / Segment Player"
              padded={false}
              actions={
                <div className="flex items-center gap-2">
                  <a className={ANCHOR_BTN} href={playback.src} download>
                    Download
                  </a>
                  <Button size="sm" variant="ghost" onClick={() => setPlayback(null)}>
                    Close
                  </Button>
                </div>
              }
            >
              <div className="p-3">
                <video
                  key={playback.src}
                  className="aspect-video w-full rounded-md border border-line bg-black"
                  src={playback.src}
                  controls
                  autoPlay
                />
                <p className="mt-2 truncate font-mono text-xs text-fg-muted">{playback.label}</p>
              </div>
            </Panel>
          )}

          <PlaybackSessionPanel key={id} cameraId={id} />

          <Panel
            title="Timeline"
            subtitle="Recorded availability"
            actions={
              <div className="flex gap-1">
                {RANGE_OPTIONS.map((opt) => (
                  <Button
                    key={opt.hours}
                    size="sm"
                    variant={rangeHours === opt.hours ? "primary" : "default"}
                    onClick={() => setRangeHours(opt.hours)}
                  >
                    {opt.label}
                  </Button>
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
              <div className="flex items-center justify-center gap-2 py-8 font-mono text-xs text-fg-muted">
                {timeline.error ? (
                  <span className="text-danger">{timeline.error}</span>
                ) : (
                  <>
                    <Spinner size={14} /> Loading timeline…
                  </>
                )}
              </div>
            )}
          </Panel>

          <AiPanel cameraId={id} />

          <ZonePanel cameraId={id} />

          <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
            <Panel title="Snapshot" subtitle="Frame grab">
              <div className="flex flex-wrap items-end gap-2">
                <div className="min-w-0 flex-1">
                  <Field label="At time — blank = live" htmlFor="snap-at">
                    <Input
                      id="snap-at"
                      type="datetime-local"
                      step={1}
                      value={snapInput}
                      onChange={(e) => setSnapInput(e.target.value)}
                    />
                  </Field>
                </div>
                <Button onClick={captureSnapshot}>Capture</Button>
              </div>
              {snapSrc && (
                <div className="mt-3">
                  <img
                    src={snapSrc}
                    alt="Snapshot"
                    className="w-full rounded-md border border-line bg-black"
                  />
                  <a className={`${ANCHOR_BTN} mt-2`} href={snapSrc} target="_blank" rel="noreferrer">
                    Open full size
                  </a>
                </div>
              )}
            </Panel>

            <Panel title="Evidence Clip" subtitle="Export MP4">
              <form onSubmit={submitClip} className="space-y-3">
                <Field label="From" htmlFor="clip-from">
                  <Input
                    id="clip-from"
                    type="datetime-local"
                    step={1}
                    value={clipFrom}
                    onChange={(e) => setClipFrom(e.target.value)}
                    required
                  />
                </Field>
                <Field label="To" htmlFor="clip-to">
                  <Input
                    id="clip-to"
                    type="datetime-local"
                    step={1}
                    value={clipTo}
                    onChange={(e) => setClipTo(e.target.value)}
                    required
                  />
                </Field>
                <Button type="submit" variant="primary" className="w-full" disabled={clipLoading}>
                  {clipLoading ? "Exporting…" : "Export clip"}
                </Button>
              </form>
              {clipError && <p className="mt-2 font-mono text-xs text-danger">{clipError}</p>}
              {clipResult && (
                <div className="mt-3 rounded-md border border-line bg-canvas p-3">
                  <div className="truncate font-mono text-xs font-semibold text-fg">
                    {clipResult.filename}
                  </div>
                  <div className="mt-1 font-mono text-[11px] text-fg-muted">
                    {formatDuration(clipResult.requested_seconds)} · {formatBytes(clipResult.size_bytes)} ·{" "}
                    {clipResult.segment_count} segments
                  </div>
                  <div className="mt-2 flex gap-2">
                    <a className={ANCHOR_BTN} href={clipResult.url} download>
                      Download
                    </a>
                    <Button
                      size="sm"
                      onClick={() =>
                        setPlayback({ src: clipResult.url, label: clipResult.filename })
                      }
                    >
                      Play
                    </Button>
                  </div>
                </div>
              )}
            </Panel>
          </div>

          <SnapshotSchedulesPanel cameraId={id} canManage={canManage} />
        </div>

        {/* Side column */}
        <div className="stagger space-y-4">
          <Panel title="Health" subtitle="Recorder telemetry">
            {st ? (
              <>
                <div className="mb-4 flex items-center justify-between gap-2">
                  <StatusPill state={st.state} />
                  <span className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
                    upd {timeAgo(st.updated_at)}
                  </span>
                </div>
                <div className="grid grid-cols-2 gap-x-4 gap-y-4">
                  <Stat
                    label="FPS"
                    value={st.fps_observed != null ? st.fps_observed.toFixed(1) : "—"}
                    unit={st.fps_observed != null ? "fps" : undefined}
                  />
                  <Stat
                    label="Bitrate"
                    value={st.bitrate_kbps != null ? st.bitrate_kbps.toFixed(0) : "—"}
                    unit={st.bitrate_kbps != null ? "kbps" : undefined}
                  />
                  <Stat
                    label="Reconnects"
                    value={st.reconnect_count}
                    tone={st.reconnect_count > 0 ? "warn" : "default"}
                  />
                  <Stat label="Segments" value={st.segments_written} />
                </div>
                <div className="mt-4 border-t border-line pt-2">
                  <Meta label="Last segment" value={timeAgo(st.last_segment_at)} />
                  <Meta label="Last started" value={timeAgo(st.last_started_at)} />
                  <Meta label="Recorder PID" value={st.recorder_pid ?? "—"} />
                </div>
                {st.last_error && (
                  <div className="mt-3 rounded-md border border-danger/30 bg-danger/10 px-2.5 py-2 font-mono text-[11px] text-red-300">
                    {st.last_error}
                  </div>
                )}
              </>
            ) : (
              <p className="font-mono text-xs text-fg-muted">
                {status.error ?? "No health data yet."}
              </p>
            )}
          </Panel>

          <Panel title="Settings" subtitle="Recording & actions">
            <div className="grid grid-cols-2 gap-2">
              <Button
                disabled={actionBusy || !cam}
                onClick={() => cam && toggle("record_enabled", !cam.record_enabled)}
                className="w-full"
              >
                {cam?.record_enabled ? "Pause rec" : "Resume rec"}
              </Button>
              <Button
                disabled={actionBusy || !cam}
                onClick={() => cam && toggle("enabled", !cam.enabled)}
                className="w-full"
              >
                {cam?.enabled ? "Disable" : "Enable"}
              </Button>
              <Button onClick={runTest} disabled={testing} className="w-full">
                {testing ? "Testing…" : "Test stream"}
              </Button>
              <Button variant="danger" disabled={actionBusy} onClick={remove} className="w-full">
                Delete
              </Button>
            </div>

            {testResult && (
              <div
                className={`mt-3 rounded-md border px-2.5 py-2 font-mono text-[11px] ${
                  testResult.reachable
                    ? "border-rec/40 bg-rec/10 text-emerald-200"
                    : "border-danger/40 bg-danger/10 text-red-200"
                }`}
              >
                {testResult.reachable ? (
                  <span>
                    Reachable · {testResult.codec ?? "?"} {testResult.width}×{testResult.height}
                    <span className="mt-1 block truncate text-fg-muted">{testResult.url}</span>
                  </span>
                ) : (
                  <span>Unreachable — {testResult.error ?? "unknown error"}</span>
                )}
              </div>
            )}

            {cam && (
              <div className="mt-3 border-t border-line pt-2">
                <Meta label="Record stream" value={cam.record_stream} />
                <Meta label="Segment length" value={`${cam.segment_seconds}s`} />
                <Meta label="Retention" value={`${cam.retention_hours}h`} />
                <Meta label="Codec" value={cam.codec ?? "—"} />
              </div>
            )}
          </Panel>

          {cam && (
            <RecordingSettingsPanel
              key={cam.id}
              camera={cam}
              canManage={canManage}
              onSaved={() => camera.refresh()}
            />
          )}

          {isScheduledMode && (
            <RecordingSchedulePanel cameraId={id} canManage={canManage} />
          )}

          <PtzPanel cameraId={id} canManage={canManage} />

          <Panel
            title="Recent Segments"
            subtitle={isEventMode ? "Lock evidence · trigger recording" : "Lock evidence"}
            actions={
              <div className="flex items-center gap-2">
                {isEventMode && (
                  <Button
                    size="sm"
                    variant="primary"
                    disabled={!canManage || trigBusy}
                    onClick={() => void recordNow()}
                  >
                    {trigBusy ? "…" : "Record now"}
                  </Button>
                )}
                <span className="font-mono text-[11px] tabular-nums text-fg-muted">
                  {recentSegments.length}
                </span>
              </div>
            }
          >
            {trigResult && (
              <div className="mb-3 rounded-md border border-rec/40 bg-rec/10 px-2.5 py-2 font-mono text-[11px] text-emerald-200">
                Recording window extended to {formatClock(trigResult.window_end)} (post-roll{" "}
                {trigResult.post_roll_seconds}s).
              </div>
            )}
            {recentSegments.length === 0 ? (
              <p className="font-mono text-xs text-fg-muted">No recorded segments yet.</p>
            ) : (
              <ul className="-mr-1 max-h-96 space-y-1.5 overflow-y-auto pr-1">
                {recentSegments.map((seg) => (
                  <SegmentRow
                    key={seg.id}
                    seg={seg}
                    canManage={canManage}
                    busy={segBusy === seg.id}
                    onPlay={() =>
                      setPlayback({
                        src: seg.url,
                        label: `Segment ${formatClock(seg.start_time)}`,
                      })
                    }
                    onToggleEvidence={() => void toggleEvidence(seg)}
                  />
                ))}
              </ul>
            )}
          </Panel>

          <RecordingGapsPanel cameraId={id} canManage={canManage} />

          <Panel
            title="Recent Events"
            actions={
              <span className="font-mono text-[11px] tabular-nums text-fg-muted">
                {(events.data ?? []).length}
              </span>
            }
          >
            {(events.data ?? []).length === 0 ? (
              <p className="font-mono text-xs text-fg-muted">No events.</p>
            ) : (
              <ul className="-mr-1 max-h-96 space-y-1.5 overflow-y-auto pr-1">
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

function SegmentRow({
  seg,
  canManage,
  busy,
  onPlay,
  onToggleEvidence,
}: {
  seg: SegmentView;
  canManage: boolean;
  busy: boolean;
  onPlay: () => void;
  onToggleEvidence: () => void;
}) {
  return (
    <li className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-line bg-canvas px-2.5 py-2 transition-colors duration-150 hover:border-[#34373e]">
      <div className="min-w-0">
        <div className="flex items-center gap-1.5 font-mono text-xs text-fg-secondary">
          {seg.evidence_locked && <LockIcon className="h-3 w-3 text-accent" />}
          <span className="tabular-nums">
            {formatTimeShort(seg.start_time)} → {formatTimeShort(seg.end_time)}
          </span>
        </div>
        <div className="mt-0.5 flex flex-wrap items-center gap-x-1.5 font-mono text-[10px] text-fg-muted">
          <span>{formatDuration(seg.duration_s)}</span>
          <span className="text-fg-muted/60">·</span>
          <span>{formatBytes(seg.size_bytes)}</span>
          {seg.incident_id && (
            <>
              <span className="text-fg-muted/60">·</span>
              <span className="truncate text-accent-soft">#{seg.incident_id}</span>
            </>
          )}
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-1.5">
        {busy && <Spinner size={13} />}
        <Button size="sm" onClick={onPlay}>
          Play
        </Button>
        <Button
          size="sm"
          variant={seg.evidence_locked ? "danger" : "default"}
          disabled={!canManage || busy}
          onClick={onToggleEvidence}
        >
          {seg.evidence_locked ? "Unlock" : "Lock"}
        </Button>
      </div>
    </li>
  );
}

function EventRow({ ev }: { ev: VisionEvent }) {
  const payloadKeys = Object.keys(ev.payload ?? {});
  const color = SEVERITY_COLOR[ev.severity] ?? SEVERITY_COLOR.info;
  return (
    <li className="rounded-md border border-line bg-canvas px-2.5 py-2">
      <div className="flex items-center justify-between gap-2">
        <span className="truncate text-xs font-semibold text-fg">{ev.event_type}</span>
        <span
          className="shrink-0 rounded border px-1.5 py-0.5 font-mono text-[9px] font-semibold uppercase tracking-micro"
          style={{ color, borderColor: `${color}55`, backgroundColor: `${color}1a` }}
        >
          {ev.severity}
        </span>
      </div>
      <div className="mt-0.5 font-mono text-[10px] text-fg-muted">{formatClock(ev.timestamp)}</div>
      {payloadKeys.length > 0 && (
        <div
          className="mt-1 truncate font-mono text-[10px] text-fg-muted/80"
          title={JSON.stringify(ev.payload)}
        >
          {JSON.stringify(ev.payload)}
        </div>
      )}
    </li>
  );
}
