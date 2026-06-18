// Heldar Core — DVR control surfaces for a single camera (CameraDetail).
//
// A cohesive set of cards that drive the recording/DVR backend (batches 1-9):
//   - RecordingSettingsPanel  : PATCH camera record config (quota, audio, mode, roll, mirror, ANR)
//   - RecordingSchedulePanel  : per-camera time-of-day recording windows (camera_schedules)
//   - SnapshotSchedulesPanel  : interval snapshot schedules + a gallery of persisted snapshots
//   - PtzPanel                : ONVIF PTZ d-pad / zoom / presets (only when the device has PTZ)
//   - RecordingGapsPanel      : persisted ANR recording gaps + retry
//   - PlaybackSessionPanel    : segment-spanning HLS VOD player over a recorded time range
//
// Reads are open to any principal; mutations are manager+ (the API enforces this — the controls
// mirror it by gating on `canManage`). Shares the design system primitives from ui.tsx.

import Hls from "hls.js";
import { useEffect, useRef, useState } from "react";
import type { FormEvent, ReactNode } from "react";
import { api, ApiError } from "../lib/api";
import { usePoll } from "../lib/usePoll";
import type {
  CameraUpdate,
  CameraView,
  GapFillState,
  PlaybackSession,
  RecordMode,
  RecordSchedule,
  RecordingGap,
  SnapshotSchedule,
  SnapshotView,
} from "../lib/types";
import { Button, Field, Input, Panel, Select, cx } from "./ui";
import {
  formatBytes,
  formatClock,
  formatDuration,
  isoToLocalInput,
  localInputToIso,
  timeAgo,
} from "../lib/format";

/* ------------------------------ shared bits ------------------------------ */

/** 1 GiB in bytes — quota inputs are entered in GB and stored as bytes. */
const GIB = 1024 ** 3;

/** weekday int (0=Mon..6=Sun) -> short label. */
const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] as const;

const RECORD_MODE_OPTIONS: { value: RecordMode; label: string }[] = [
  { value: "continuous", label: "Continuous" },
  { value: "scheduled", label: "Scheduled" },
  { value: "event", label: "Event" },
  { value: "scheduled_event", label: "Scheduled + Event" },
];

function errMsg(e: unknown): string {
  return e instanceof ApiError || e instanceof Error ? e.message : String(e);
}

/** A small on/off switch matching the dark/accent design system. */
function Switch({
  checked,
  onChange,
  disabled,
  id,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  id?: string;
}) {
  return (
    <button
      id={id}
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      onClick={() => !disabled && onChange(!checked)}
      className={cx(
        "relative inline-flex h-5 w-9 shrink-0 items-center rounded-full border transition-colors duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-canvas disabled:cursor-not-allowed disabled:opacity-50",
        checked ? "border-transparent bg-accent" : "border-line bg-raised",
      )}
    >
      <span
        className={cx(
          "inline-block h-3.5 w-3.5 rounded-full bg-fg shadow transition-transform duration-150",
          checked ? "translate-x-4" : "translate-x-0.5",
        )}
      />
    </button>
  );
}

/** Labelled switch row used inside the settings editors. */
function ToggleField({
  label,
  hint,
  checked,
  onChange,
  disabled,
}: {
  label: ReactNode;
  hint?: ReactNode;
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <div className="flex items-center justify-between gap-3">
      <div className="min-w-0">
        <div className="font-mono text-[10px] font-medium uppercase tracking-micro text-fg-secondary">
          {label}
        </div>
        {hint != null && <div className="mt-0.5 text-[11px] leading-snug text-fg-muted">{hint}</div>}
      </div>
      <Switch checked={checked} onChange={onChange} disabled={disabled} />
    </div>
  );
}

function ErrorNote({ children }: { children: ReactNode }) {
  return <p className="font-mono text-xs text-danger">{children}</p>;
}

/** Read-only notice shown to non-managers in lieu of mutation controls. */
function ReadOnlyNote() {
  return (
    <p className="font-mono text-[11px] text-fg-muted">
      Manager role required to change recording configuration.
    </p>
  );
}

/* ------------------------------- HLS video ------------------------------- */

/** Attaches an hls.js instance (or native HLS) to a <video> for VOD playback. */
function HlsVideo({ src, className }: { src: string; className?: string }) {
  const videoRef = useRef<HTMLVideoElement>(null);

  useEffect(() => {
    const video = videoRef.current;
    if (!video || !src) return;
    let hls: Hls | null = null;

    if (Hls.isSupported()) {
      hls = new Hls();
      hls.loadSource(src);
      hls.attachMedia(video);
      hls.on(Hls.Events.MANIFEST_PARSED, () => {
        video.play().catch(() => {
          /* autoplay may be blocked until a user gesture */
        });
      });
      return () => {
        hls?.destroy();
        video.removeAttribute("src");
        video.load();
      };
    }

    if (video.canPlayType("application/vnd.apple.mpegurl")) {
      video.src = src;
      const onMeta = () =>
        video.play().catch(() => {
          /* autoplay may be blocked */
        });
      video.addEventListener("loadedmetadata", onMeta);
      return () => {
        video.removeEventListener("loadedmetadata", onMeta);
        video.removeAttribute("src");
        video.load();
      };
    }

    return undefined;
  }, [src]);

  return <video ref={videoRef} className={className} controls autoPlay playsInline />;
}

/* ========================= Recording settings editor ====================== */

export function RecordingSettingsPanel({
  camera,
  canManage,
  onSaved,
}: {
  camera: CameraView;
  canManage: boolean;
  onSaved: () => void | Promise<void>;
}) {
  // Initialised once per mount; CameraDetail remounts this with key={camera.id}, so navigating to a
  // different camera resets the form while polling the same camera preserves in-flight edits.
  const [quotaGb, setQuotaGb] = useState(
    camera.storage_quota_bytes != null ? (camera.storage_quota_bytes / GIB).toFixed(2) : "",
  );
  const [recordAudio, setRecordAudio] = useState(camera.record_audio);
  const [recordMode, setRecordMode] = useState<RecordMode>(camera.record_mode);
  const [preRoll, setPreRoll] = useState(String(camera.pre_roll_seconds));
  const [postRoll, setPostRoll] = useState(String(camera.post_roll_seconds));
  const [mirrorEnabled, setMirrorEnabled] = useState(camera.mirror_enabled);
  const [anrEnabled, setAnrEnabled] = useState(camera.anr_enabled);
  const [anrTemplate, setAnrTemplate] = useState(camera.anr_replay_url_template ?? "");

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  const eventMode = recordMode === "event" || recordMode === "scheduled_event";

  async function save(e: FormEvent) {
    e.preventDefault();
    setError(null);
    setSaved(false);
    let quotaBytes: number | null = null;
    if (quotaGb.trim()) {
      const gb = Number(quotaGb);
      if (!Number.isFinite(gb) || gb < 0) {
        setError("Storage quota must be a non-negative number of GB (blank = uncapped).");
        return;
      }
      quotaBytes = Math.round(gb * GIB);
    }
    const body: CameraUpdate = {
      storage_quota_bytes: quotaBytes,
      record_audio: recordAudio,
      record_mode: recordMode,
      pre_roll_seconds: Number(preRoll) || 0,
      post_roll_seconds: Number(postRoll) || 0,
      mirror_enabled: mirrorEnabled,
      anr_enabled: anrEnabled,
      anr_replay_url_template: anrTemplate.trim() ? anrTemplate.trim() : null,
    };
    setBusy(true);
    try {
      await api.updateCamera(camera.id, body);
      setSaved(true);
      await onSaved();
    } catch (err) {
      setError(errMsg(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Panel title="Recording Settings" subtitle="Capture configuration">
      <form onSubmit={save} className="space-y-4">
        <Field
          label="Storage quota (GB)"
          htmlFor="rs-quota"
          hint={
            camera.storage_quota_bytes != null
              ? `Currently ${formatBytes(camera.storage_quota_bytes)} · blank = uncapped`
              : "Blank = uncapped (global retention only)"
          }
        >
          <Input
            id="rs-quota"
            type="number"
            min={0}
            step={0.5}
            value={quotaGb}
            onChange={(e) => setQuotaGb(e.target.value)}
            placeholder="uncapped"
            disabled={!canManage}
          />
        </Field>

        <Field label="Record mode" htmlFor="rs-mode">
          <Select
            id="rs-mode"
            value={recordMode}
            onChange={(e) => setRecordMode(e.target.value as RecordMode)}
            disabled={!canManage}
          >
            {RECORD_MODE_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </Select>
        </Field>

        {eventMode && (
          <div className="grid grid-cols-2 gap-3">
            <Field label="Pre-roll (s)" htmlFor="rs-preroll" hint="0–300">
              <Input
                id="rs-preroll"
                type="number"
                min={0}
                max={300}
                step={1}
                value={preRoll}
                onChange={(e) => setPreRoll(e.target.value)}
                disabled={!canManage}
              />
            </Field>
            <Field label="Post-roll (s)" htmlFor="rs-postroll" hint="0–3600">
              <Input
                id="rs-postroll"
                type="number"
                min={0}
                max={3600}
                step={1}
                value={postRoll}
                onChange={(e) => setPostRoll(e.target.value)}
                disabled={!canManage}
              />
            </Field>
          </div>
        )}

        <div className="space-y-3 border-t border-line pt-3">
          <ToggleField
            label="Record audio"
            hint="Pass through the camera's audio stream"
            checked={recordAudio}
            onChange={setRecordAudio}
            disabled={!canManage}
          />
          <ToggleField
            label="Mirror recordings"
            hint="Second pipeline to the configured mirror dir"
            checked={mirrorEnabled}
            onChange={setMirrorEnabled}
            disabled={!canManage}
          />
          <ToggleField
            label="ANR backfill"
            hint="Refetch missed footage from the camera's onboard storage"
            checked={anrEnabled}
            onChange={setAnrEnabled}
            disabled={!canManage}
          />
        </div>

        {anrEnabled && (
          <Field
            label="ANR replay URL template"
            htmlFor="rs-anr-tpl"
            hint="{start}/{end} placeholders · blank = default Hikvision RTSP playback"
          >
            <Input
              id="rs-anr-tpl"
              value={anrTemplate}
              onChange={(e) => setAnrTemplate(e.target.value)}
              placeholder="rtsp://…/playback?starttime={start}&endtime={end}"
              disabled={!canManage}
            />
          </Field>
        )}

        {canManage ? (
          <Button type="submit" variant="primary" className="w-full" disabled={busy}>
            {busy ? "Saving…" : "Save settings"}
          </Button>
        ) : (
          <ReadOnlyNote />
        )}
        {error && <ErrorNote>{error}</ErrorNote>}
        {saved && !error && (
          <p className="font-mono text-[11px] text-rec">Settings saved.</p>
        )}
      </form>
    </Panel>
  );
}

/* ========================= Recording schedule editor ====================== */

export function RecordingSchedulePanel({
  cameraId,
  canManage,
}: {
  cameraId: string;
  canManage: boolean;
}) {
  const schedules = usePoll(() => api.listSchedules(cameraId), 20000, [cameraId]);

  const [days, setDays] = useState<number[]>([0, 1, 2, 3, 4]);
  const [start, setStart] = useState("08:00");
  const [end, setEnd] = useState("18:00");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  function toggleDay(d: number) {
    setDays((cur) => (cur.includes(d) ? cur.filter((x) => x !== d) : [...cur, d].sort((a, b) => a - b)));
  }

  async function add(e: FormEvent) {
    e.preventDefault();
    setError(null);
    if (days.length === 0) {
      setError("Select at least one weekday.");
      return;
    }
    if (!start || !end) {
      setError("Both start and end times are required.");
      return;
    }
    setBusy(true);
    try {
      await api.createSchedule(cameraId, { days, time_start: start, time_end: end, enabled: true });
      setDays([0, 1, 2, 3, 4]);
      setStart("08:00");
      setEnd("18:00");
      await schedules.refresh();
    } catch (err) {
      setError(errMsg(err));
    } finally {
      setBusy(false);
    }
  }

  async function toggle(s: RecordSchedule) {
    setBusy(true);
    try {
      await api.updateSchedule(s.id, { enabled: !s.enabled });
      await schedules.refresh();
    } catch (err) {
      setError(errMsg(err));
    } finally {
      setBusy(false);
    }
  }

  async function remove(s: RecordSchedule) {
    if (!window.confirm("Delete this recording window?")) return;
    setBusy(true);
    try {
      await api.deleteSchedule(s.id);
      await schedules.refresh();
    } catch (err) {
      setError(errMsg(err));
    } finally {
      setBusy(false);
    }
  }

  const list = schedules.data ?? [];

  return (
    <Panel
      title="Recording Schedule"
      subtitle="Time-of-day windows"
      actions={
        <span className="font-mono text-[11px] tabular-nums text-fg-muted">{list.length}</span>
      }
    >
      {list.length === 0 ? (
        <p className="font-mono text-xs text-fg-muted">
          {schedules.error ?? "No windows. Add one below — the recorder runs only inside these windows."}
        </p>
      ) : (
        <ul className="space-y-1.5">
          {list.map((s) => (
            <li
              key={s.id}
              className="flex items-center justify-between gap-2 rounded-md border border-line bg-canvas px-2.5 py-2 transition-colors duration-150 hover:border-[#34373e]"
            >
              <div className="min-w-0">
                <div className="flex items-center gap-2">
                  <span
                    className={cx(
                      "inline-flex h-1.5 w-1.5 shrink-0 rounded-full",
                      s.enabled ? "bg-rec shadow-glow-rec" : "bg-disabled",
                    )}
                  />
                  <span className="font-mono text-xs font-semibold tabular-nums text-fg">
                    {s.time_start}–{s.time_end}
                  </span>
                </div>
                <div className="mt-0.5 font-mono text-[10px] text-fg-muted">
                  {s.days.length === 7
                    ? "Every day"
                    : s.days
                        .slice()
                        .sort((a, b) => a - b)
                        .map((d) => WEEKDAYS[d] ?? d)
                        .join(", ")}
                </div>
              </div>
              <div className="flex shrink-0 items-center gap-1.5">
                <Button size="sm" disabled={!canManage || busy} onClick={() => void toggle(s)}>
                  {s.enabled ? "Disable" : "Enable"}
                </Button>
                <Button
                  size="sm"
                  variant="danger"
                  disabled={!canManage || busy}
                  onClick={() => void remove(s)}
                  aria-label="Delete window"
                >
                  ✕
                </Button>
              </div>
            </li>
          ))}
        </ul>
      )}

      {canManage && (
        <form onSubmit={add} className="mt-4 space-y-3 border-t border-line pt-4">
          <div className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
            Add window
          </div>
          <div className="flex flex-wrap gap-1.5">
            {WEEKDAYS.map((label, d) => {
              const on = days.includes(d);
              return (
                <button
                  key={d}
                  type="button"
                  onClick={() => toggleDay(d)}
                  aria-pressed={on}
                  className={cx(
                    "rounded-md border px-2 py-1 font-mono text-[11px] font-medium transition-colors duration-150",
                    on
                      ? "border-transparent bg-accent text-accent-ink"
                      : "border-line bg-raised text-fg-secondary hover:border-[#34373e] hover:bg-[#23262c]",
                  )}
                >
                  {label}
                </button>
              );
            })}
          </div>
          <div className="grid grid-cols-2 gap-3">
            <Field label="Start" htmlFor="sch-start">
              <Input id="sch-start" type="time" value={start} onChange={(e) => setStart(e.target.value)} />
            </Field>
            <Field label="End" htmlFor="sch-end" hint="Start > end = overnight">
              <Input id="sch-end" type="time" value={end} onChange={(e) => setEnd(e.target.value)} />
            </Field>
          </div>
          <Button type="submit" variant="primary" className="w-full" disabled={busy}>
            {busy ? "Working…" : "Add window"}
          </Button>
          {error && <ErrorNote>{error}</ErrorNote>}
        </form>
      )}
      {!canManage && error && <ErrorNote>{error}</ErrorNote>}
    </Panel>
  );
}

/* ======================= Snapshot schedules + gallery ===================== */

export function SnapshotSchedulesPanel({
  cameraId,
  canManage,
}: {
  cameraId: string;
  canManage: boolean;
}) {
  const schedules = usePoll(() => api.listSnapshotSchedules(cameraId), 20000, [cameraId]);
  const snapshots = usePoll(() => api.listSnapshots(cameraId, { limit: 24 }), 30000, [cameraId]);

  const [interval, setIntervalSecs] = useState("300");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function add(e: FormEvent) {
    e.preventDefault();
    setError(null);
    const secs = Number(interval);
    if (!Number.isFinite(secs) || secs <= 0) {
      setError("Interval must be a positive number of seconds.");
      return;
    }
    setBusy(true);
    try {
      await api.createSnapshotSchedule(cameraId, { interval_seconds: Math.round(secs), enabled: true });
      await schedules.refresh();
    } catch (err) {
      setError(errMsg(err));
    } finally {
      setBusy(false);
    }
  }

  async function toggle(s: SnapshotSchedule) {
    setBusy(true);
    try {
      await api.updateSnapshotSchedule(s.id, { enabled: !s.enabled });
      await schedules.refresh();
    } catch (err) {
      setError(errMsg(err));
    } finally {
      setBusy(false);
    }
  }

  async function remove(s: SnapshotSchedule) {
    if (!window.confirm("Delete this snapshot schedule?")) return;
    setBusy(true);
    try {
      await api.deleteSnapshotSchedule(s.id);
      await schedules.refresh();
    } catch (err) {
      setError(errMsg(err));
    } finally {
      setBusy(false);
    }
  }

  const list = schedules.data ?? [];
  const snaps = snapshots.data ?? [];

  return (
    <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
      <Panel
        title="Snapshot Schedules"
        subtitle="Interval JPEG capture"
        actions={
          <span className="font-mono text-[11px] tabular-nums text-fg-muted">{list.length}</span>
        }
      >
        {list.length === 0 ? (
          <p className="font-mono text-xs text-fg-muted">
            {schedules.error ?? "No snapshot schedules. Add one to capture frames on an interval."}
          </p>
        ) : (
          <ul className="space-y-1.5">
            {list.map((s) => (
              <li
                key={s.id}
                className="flex items-center justify-between gap-2 rounded-md border border-line bg-canvas px-2.5 py-2 transition-colors duration-150 hover:border-[#34373e]"
              >
                <div className="min-w-0">
                  <div className="flex items-center gap-2">
                    <span
                      className={cx(
                        "inline-flex h-1.5 w-1.5 shrink-0 rounded-full",
                        s.enabled ? "bg-rec shadow-glow-rec" : "bg-disabled",
                      )}
                    />
                    <span className="font-mono text-xs font-semibold tabular-nums text-fg">
                      every {formatDuration(s.interval_seconds)}
                    </span>
                  </div>
                  <div className="mt-0.5 font-mono text-[10px] text-fg-muted">
                    last fired {timeAgo(s.last_fired_at)}
                  </div>
                </div>
                <div className="flex shrink-0 items-center gap-1.5">
                  <Button size="sm" disabled={!canManage || busy} onClick={() => void toggle(s)}>
                    {s.enabled ? "Disable" : "Enable"}
                  </Button>
                  <Button
                    size="sm"
                    variant="danger"
                    disabled={!canManage || busy}
                    onClick={() => void remove(s)}
                    aria-label="Delete snapshot schedule"
                  >
                    ✕
                  </Button>
                </div>
              </li>
            ))}
          </ul>
        )}

        {canManage && (
          <form onSubmit={add} className="mt-4 space-y-3 border-t border-line pt-4">
            <div className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
              Add schedule
            </div>
            <Field label="Interval (s)" htmlFor="snap-interval">
              <Input
                id="snap-interval"
                type="number"
                min={1}
                step={1}
                value={interval}
                onChange={(e) => setIntervalSecs(e.target.value)}
              />
            </Field>
            <Button type="submit" variant="primary" className="w-full" disabled={busy}>
              {busy ? "Working…" : "Add schedule"}
            </Button>
            {error && <ErrorNote>{error}</ErrorNote>}
          </form>
        )}
        {!canManage && error && <ErrorNote>{error}</ErrorNote>}
      </Panel>

      <Panel
        title="Recent Snapshots"
        subtitle="Persisted captures"
        actions={
          <span className="font-mono text-[11px] tabular-nums text-fg-muted">{snaps.length}</span>
        }
      >
        {snaps.length === 0 ? (
          <p className="font-mono text-xs text-fg-muted">
            {snapshots.error ?? "No captured snapshots yet."}
          </p>
        ) : (
          <div className="grid grid-cols-3 gap-2 sm:grid-cols-4">
            {snaps.map((snap) => (
              <SnapshotThumb key={snap.id} snap={snap} />
            ))}
          </div>
        )}
      </Panel>
    </div>
  );
}

function SnapshotThumb({ snap }: { snap: SnapshotView }) {
  return (
    <a
      href={snap.url}
      target="_blank"
      rel="noreferrer"
      title={`${formatClock(snap.taken_at)} · ${formatBytes(snap.size_bytes)}`}
      className="group relative block overflow-hidden rounded-md border border-line bg-black transition-colors duration-150 hover:border-accent"
    >
      <img
        src={snap.url}
        alt={`Snapshot ${formatClock(snap.taken_at)}`}
        loading="lazy"
        className="aspect-video w-full object-cover"
      />
      <span className="pointer-events-none absolute inset-x-0 bottom-0 truncate bg-gradient-to-t from-black/80 to-transparent px-1.5 py-1 font-mono text-[9px] tabular-nums text-fg-secondary">
        {timeAgo(snap.taken_at)}
      </span>
    </a>
  );
}

/* ================================ PTZ control ============================= */

const PTZ_SPEED = 0.6;

interface PtzDir {
  pan: number;
  tilt: number;
  glyph: string;
  label: string;
}

const PTZ_DIRS: (PtzDir | null)[] = [
  { pan: -PTZ_SPEED, tilt: PTZ_SPEED, glyph: "↖", label: "up-left" },
  { pan: 0, tilt: PTZ_SPEED, glyph: "↑", label: "up" },
  { pan: PTZ_SPEED, tilt: PTZ_SPEED, glyph: "↗", label: "up-right" },
  { pan: -PTZ_SPEED, tilt: 0, glyph: "←", label: "left" },
  null, // center → stop
  { pan: PTZ_SPEED, tilt: 0, glyph: "→", label: "right" },
  { pan: -PTZ_SPEED, tilt: -PTZ_SPEED, glyph: "↙", label: "down-left" },
  { pan: 0, tilt: -PTZ_SPEED, glyph: "↓", label: "down" },
  { pan: PTZ_SPEED, tilt: -PTZ_SPEED, glyph: "↘", label: "down-right" },
];

export function PtzPanel({ cameraId, canManage }: { cameraId: string; canManage: boolean }) {
  const onvif = usePoll(() => api.getCameraOnvif(cameraId), 0, [cameraId]);
  const presets = usePoll(() => api.listPtzPresets(cameraId), 0, [cameraId]);

  const [preset, setPreset] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Tracks whether a continuous move is in flight, so pointer-up/leave only issues a stop after a
  // real press (not on a stray hover-out).
  const movingRef = useRef(false);

  // Only meaningful for devices that actually expose PTZ on the bound profile.
  if (!onvif.data?.ptz_enabled) return null;

  const presetList = presets.data ?? [];

  function move(dir: PtzDir) {
    if (!canManage) return;
    movingRef.current = true;
    api.ptzContinuous(cameraId, { pan: dir.pan, tilt: dir.tilt }).catch((e) => setError(errMsg(e)));
  }

  function zoom(z: number) {
    if (!canManage) return;
    movingRef.current = true;
    api.ptzContinuous(cameraId, { zoom: z }).catch((e) => setError(errMsg(e)));
  }

  // Issued on pointer-up/leave — only after a real press (movingRef), so a stray hover-out is a no-op.
  function stop() {
    if (!canManage || !movingRef.current) return;
    movingRef.current = false;
    api.ptzStop(cameraId).catch((e) => setError(errMsg(e)));
  }

  // Explicit Stop control — always halts any in-progress move.
  function stopNow() {
    if (!canManage) return;
    movingRef.current = false;
    api.ptzStop(cameraId).catch((e) => setError(errMsg(e)));
  }

  async function goto() {
    if (!preset) return;
    setBusy(true);
    setError(null);
    try {
      await api.ptzGotoPreset(cameraId, preset);
    } catch (err) {
      setError(errMsg(err));
    } finally {
      setBusy(false);
    }
  }

  async function refresh() {
    setBusy(true);
    setError(null);
    try {
      await api.refreshPtzPresets(cameraId);
      await presets.refresh();
    } catch (err) {
      setError(errMsg(err));
    } finally {
      setBusy(false);
    }
  }

  const ptzBtn =
    "flex aspect-square items-center justify-center rounded-md border border-line bg-raised text-base text-fg transition-colors duration-150 hover:border-[#34373e] hover:bg-[#23262c] active:bg-accent active:text-accent-ink disabled:cursor-not-allowed disabled:opacity-40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-canvas";

  return (
    <Panel title="PTZ Control" subtitle={onvif.data.model ?? "ONVIF Profile S"}>
      {!canManage && (
        <p className="mb-3 font-mono text-[11px] text-fg-muted">
          Manager role required to move the camera.
        </p>
      )}

      <div className="flex flex-wrap items-start justify-center gap-4">
        {/* Directional pad */}
        <div className="grid w-40 grid-cols-3 gap-1.5">
          {PTZ_DIRS.map((dir, i) =>
            dir ? (
              <button
                key={i}
                type="button"
                className={ptzBtn}
                disabled={!canManage}
                aria-label={`Pan/tilt ${dir.label}`}
                onPointerDown={() => move(dir)}
                onPointerUp={stop}
                onPointerLeave={stop}
                onPointerCancel={stop}
              >
                {dir.glyph}
              </button>
            ) : (
              <button
                key={i}
                type="button"
                className={cx(ptzBtn, "text-[10px] font-semibold uppercase tracking-micro")}
                disabled={!canManage}
                aria-label="Stop"
                onClick={stopNow}
              >
                Stop
              </button>
            ),
          )}
        </div>

        {/* Zoom */}
        <div className="flex flex-col items-center gap-1.5">
          <span className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">Zoom</span>
          <button
            type="button"
            className={cx(ptzBtn, "w-12 text-lg")}
            disabled={!canManage}
            aria-label="Zoom in"
            onPointerDown={() => zoom(PTZ_SPEED)}
            onPointerUp={stop}
            onPointerLeave={stop}
            onPointerCancel={stop}
          >
            +
          </button>
          <button
            type="button"
            className={cx(ptzBtn, "w-12 text-lg")}
            disabled={!canManage}
            aria-label="Zoom out"
            onPointerDown={() => zoom(-PTZ_SPEED)}
            onPointerUp={stop}
            onPointerLeave={stop}
            onPointerCancel={stop}
          >
            −
          </button>
        </div>
      </div>

      {/* Presets */}
      <div className="mt-4 space-y-2 border-t border-line pt-3">
        <div className="flex items-center justify-between">
          <span className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
            Presets
          </span>
          <Button size="sm" disabled={!canManage || busy} onClick={() => void refresh()}>
            Refresh
          </Button>
        </div>
        {presetList.length === 0 ? (
          <p className="font-mono text-[11px] text-fg-muted">
            {presets.error ?? "No presets stored. Refresh to fetch from the device."}
          </p>
        ) : (
          <div className="flex items-end gap-2">
            <div className="min-w-0 flex-1">
              <Select
                aria-label="PTZ preset"
                value={preset}
                onChange={(e) => setPreset(e.target.value)}
                disabled={!canManage}
              >
                <option value="">Select preset…</option>
                {presetList.map((p) => (
                  <option key={p.id} value={p.token}>
                    {p.name ?? p.token}
                  </option>
                ))}
              </Select>
            </div>
            <Button disabled={!canManage || busy || !preset} onClick={() => void goto()}>
              Go
            </Button>
          </div>
        )}
      </div>

      {error && <ErrorNote>{error}</ErrorNote>}
    </Panel>
  );
}

/* ============================== Recording gaps ============================ */

const GAP_TONE: Record<GapFillState, { color: string; label: string }> = {
  pending: { color: "#fbbf24", label: "pending" },
  filled: { color: "#10b981", label: "filled" },
  failed: { color: "#ef4444", label: "failed" },
};

export function RecordingGapsPanel({
  cameraId,
  canManage,
}: {
  cameraId: string;
  canManage: boolean;
}) {
  const gaps = usePoll(() => api.listRecordingGaps(cameraId, { limit: 30 }), 20000, [cameraId]);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function retry(g: RecordingGap) {
    setBusy(g.id);
    setError(null);
    try {
      await api.retryRecordingGap(cameraId, g.id);
      await gaps.refresh();
    } catch (err) {
      setError(errMsg(err));
    } finally {
      setBusy(null);
    }
  }

  const list = gaps.data ?? [];

  return (
    <Panel
      title="Recording Gaps"
      subtitle="ANR backfill queue"
      actions={
        <span className="font-mono text-[11px] tabular-nums text-fg-muted">{list.length}</span>
      }
    >
      {list.length === 0 ? (
        <p className="font-mono text-xs text-fg-muted">
          {gaps.error ?? "No recording gaps detected."}
        </p>
      ) : (
        <ul className="-mr-1 max-h-96 space-y-1.5 overflow-y-auto pr-1">
          {list.map((g) => {
            const tone = GAP_TONE[g.fill_state] ?? GAP_TONE.pending;
            return (
              <li
                key={g.id}
                className="flex items-center justify-between gap-2 rounded-md border border-line bg-canvas px-2.5 py-2"
              >
                <div className="min-w-0">
                  <div className="flex items-center gap-2">
                    <span
                      className="shrink-0 rounded border px-1.5 py-0.5 font-mono text-[9px] font-semibold uppercase tracking-micro"
                      style={{
                        color: tone.color,
                        borderColor: `${tone.color}55`,
                        backgroundColor: `${tone.color}1a`,
                      }}
                    >
                      {tone.label}
                    </span>
                    <span className="truncate font-mono text-xs tabular-nums text-fg-secondary">
                      {formatDuration(g.gap_seconds)}
                    </span>
                  </div>
                  <div className="mt-0.5 font-mono text-[10px] text-fg-muted">
                    {formatClock(g.gap_start)} · {g.fill_attempts} attempt
                    {g.fill_attempts === 1 ? "" : "s"}
                  </div>
                </div>
                <Button
                  size="sm"
                  className="shrink-0"
                  disabled={!canManage || busy === g.id || g.fill_state === "filled"}
                  onClick={() => void retry(g)}
                >
                  {busy === g.id ? "…" : "Retry"}
                </Button>
              </li>
            );
          })}
        </ul>
      )}
      {error && <p className="mt-2 font-mono text-xs text-danger">{error}</p>}
    </Panel>
  );
}

/* =========================== Playback session (VOD) ======================= */

export function PlaybackSessionPanel({ cameraId }: { cameraId: string }) {
  const [from, setFrom] = useState(() =>
    isoToLocalInput(new Date(Date.now() - 3600_000).toISOString()),
  );
  const [to, setTo] = useState(() => isoToLocalInput(new Date().toISOString()));
  const [session, setSession] = useState<PlaybackSession | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Best-effort teardown of the server-side session on unmount (CameraDetail remounts this with
  // key={cameraId}, so navigating cameras tears down the previous session); TTL is the backstop.
  const sessionRef = useRef<PlaybackSession | null>(null);
  sessionRef.current = session;
  useEffect(() => {
    return () => {
      const s = sessionRef.current;
      if (s) api.deletePlaybackSession(s.id).catch(() => {});
    };
  }, []);

  async function open(e: FormEvent) {
    e.preventDefault();
    setError(null);
    const f = localInputToIso(from);
    const t = localInputToIso(to);
    if (!f || !t) {
      setError("Both start and end times are required.");
      return;
    }
    if (new Date(t) <= new Date(f)) {
      setError("End must be after start.");
      return;
    }
    setLoading(true);
    try {
      // Tear down any prior session first to avoid leaking it.
      if (session) await api.deletePlaybackSession(session.id).catch(() => {});
      const s = await api.createPlaybackSession(cameraId, f, t);
      setSession(s);
    } catch (err) {
      setError(errMsg(err));
    } finally {
      setLoading(false);
    }
  }

  async function close() {
    const s = session;
    setSession(null);
    if (s) {
      try {
        await api.deletePlaybackSession(s.id);
      } catch {
        /* already gone / expired */
      }
    }
  }

  return (
    <Panel
      title="Recorded Playback"
      subtitle="Segment-spanning HLS VOD"
      padded={false}
      actions={
        session ? (
          <Button size="sm" variant="ghost" onClick={() => void close()}>
            Close
          </Button>
        ) : undefined
      }
    >
      <div className="p-3">
        {session ? (
          <>
            <HlsVideo
              key={session.id}
              src={session.playlist_url}
              className="aspect-video w-full rounded-md border border-line bg-black"
            />
            <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 font-mono text-[11px] text-fg-muted">
              <span className="tabular-nums">{formatDuration(session.duration_s)}</span>
              <span className="text-fg-muted/60">·</span>
              <span className="tabular-nums">{session.segment_count} segments</span>
              <span className="text-fg-muted/60">·</span>
              <span className="tabular-nums">
                {formatClock(session.from)} → {formatClock(session.to)}
              </span>
            </div>
          </>
        ) : (
          <form onSubmit={open} className="flex flex-wrap items-end gap-2">
            <div className="min-w-0 flex-1">
              <Field label="From" htmlFor="pb-from">
                <Input
                  id="pb-from"
                  type="datetime-local"
                  step={1}
                  value={from}
                  onChange={(e) => setFrom(e.target.value)}
                  required
                />
              </Field>
            </div>
            <div className="min-w-0 flex-1">
              <Field label="To" htmlFor="pb-to">
                <Input
                  id="pb-to"
                  type="datetime-local"
                  step={1}
                  value={to}
                  onChange={(e) => setTo(e.target.value)}
                  required
                />
              </Field>
            </div>
            <Button type="submit" variant="primary" disabled={loading}>
              {loading ? "Opening…" : "Open session"}
            </Button>
          </form>
        )}
        {error && <p className="mt-2 font-mono text-xs text-danger">{error}</p>}
      </div>
    </Panel>
  );
}
