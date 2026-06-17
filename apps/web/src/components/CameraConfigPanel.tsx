// Heldar Core — HikVision ISAPI camera-configuration surfaces.
//
// Two cohesive control clusters over the kernel's camera-config endpoints:
//   - CameraConfigPanel : single-camera sections — device identity, per-channel video encoding,
//                         clock/NTP, ONVIF enablement + user provisioning, and OSD overlays.
//   - BulkConfigPanel   : apply one configuration action across the whole fleet (or a selection),
//                         rendering a per-camera ok/error results table.
//
// Reads are open to any principal; mutations are manager+ (the API enforces this — the controls
// mirror it by gating on `canManage`). Shares the design system primitives from ui.tsx and follows
// the RecordingPanels.tsx patterns (usePoll reads, errMsg, ToggleField, ReadOnlyNote, ErrorNote).

import { useState } from "react";
import type { FormEvent, ReactNode } from "react";
import { api, ApiError } from "../lib/api";
import { usePoll } from "../lib/usePoll";
import type {
  BulkAction,
  BulkConfigRequest,
  BulkConfigResponse,
  EnableOnvifResult,
  OnvifSettings,
  OsdConfig,
  VideoConfig,
  VideoConfigPatch,
} from "../lib/types";
import { Button, Field, Input, Panel, Select, cx } from "./ui";
import { formatClock } from "../lib/format";

/* ------------------------------ shared bits ------------------------------ */

/** Encoder codecs the HikVision ISAPI layer accepts; the current value is folded in if exotic. */
const CODEC_OPTIONS = ["H.264", "H.265", "H.265+"] as const;
/** Rate-control modes for the video encoder. */
const QC_OPTIONS = ["CBR", "VBR"] as const;
/** Default username the kernel provisions for the dedicated ONVIF account (server-side default). */
const DEFAULT_ONVIF_USER = "heldar_onvif";

function errMsg(e: unknown): string {
  return e instanceof ApiError || e instanceof Error ? e.message : String(e);
}

/** Fold the live value into a fixed option list so a save never silently drops an exotic setting. */
function withCurrent(options: readonly string[], current: string): string[] {
  return current && !options.includes(current) ? [current, ...options] : [...options];
}

function ErrorNote({ children }: { children: ReactNode }) {
  return <p className="font-mono text-xs text-danger">{children}</p>;
}

function SavedNote({ children }: { children: ReactNode }) {
  return <p className="font-mono text-[11px] text-rec">{children}</p>;
}

/** Read-only notice shown to non-managers in lieu of mutation controls. */
function ReadOnlyNote() {
  return (
    <p className="font-mono text-[11px] text-fg-muted">
      Manager role required to change camera configuration.
    </p>
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

/** An ONVIF on/off badge for the device-info header. */
function OnvifBadge({ enabled }: { enabled: boolean | undefined }) {
  const color = enabled ? "#10b981" : "#52525b";
  return (
    <span
      className="inline-flex items-center gap-1.5 rounded border px-1.5 py-0.5 font-mono text-[9px] font-semibold uppercase tracking-micro"
      style={{ color, borderColor: `${color}55`, backgroundColor: `${color}1a` }}
    >
      <span className="inline-flex h-1.5 w-1.5 rounded-full" style={{ backgroundColor: color }} />
      ONVIF {enabled == null ? "?" : enabled ? "on" : "off"}
    </span>
  );
}

/** IPv4 literal → `ipaddress`, anything else → `hostname` (HikVision NTP addressingFormatType). */
function ntpAddressingFormat(host: string): string {
  return /^(\d{1,3}\.){3}\d{1,3}$/.test(host.trim()) ? "ipaddress" : "hostname";
}

/* =============================== Device info ============================== */

function DeviceInfoSection({ cameraId, canManage }: { cameraId: string; canManage: boolean }) {
  const info = usePoll(() => api.getCameraDeviceInfo(cameraId), 0, [cameraId]);
  const onvif = usePoll(() => api.getCameraOnvifSettings(cameraId), 0, [cameraId]);
  const [busy, setBusy] = useState(false);
  const [rebooting, setRebooting] = useState(false);

  async function refresh() {
    setBusy(true);
    try {
      await Promise.all([info.refresh(), onvif.refresh()]);
    } finally {
      setBusy(false);
    }
  }

  async function reboot() {
    if (!window.confirm("Reboot this camera? It will drop offline for ~1 minute.")) return;
    setRebooting(true);
    try {
      await api.rebootCamera(cameraId);
      window.alert("Reboot command sent.");
    } catch (e) {
      window.alert(e instanceof Error ? e.message : String(e));
    } finally {
      setRebooting(false);
    }
  }

  const d = info.data;

  return (
    <Panel
      title="Device Info"
      subtitle="ISAPI identity"
      actions={
        <div className="flex items-center gap-2">
          <OnvifBadge enabled={onvif.data?.onvif_enabled} />
          {canManage && (
            <Button variant="danger" size="sm" disabled={rebooting} onClick={() => void reboot()}>
              {rebooting ? "…" : "Reboot"}
            </Button>
          )}
          <Button size="sm" disabled={busy} onClick={() => void refresh()}>
            {busy ? "…" : "Refresh"}
          </Button>
        </div>
      }
    >
      {d ? (
        <div className="border-t border-line pt-1">
          <Meta label="Model" value={d.model ?? "—"} />
          <Meta label="Firmware" value={d.firmware_version ?? "—"} />
          <Meta label="Serial" value={d.serial_number ?? "—"} />
          <Meta label="Device name" value={d.device_name ?? "—"} />
        </div>
      ) : (
        <p className="font-mono text-xs text-fg-muted">
          {info.error ?? "Reading device identity over ISAPI…"}
        </p>
      )}
    </Panel>
  );
}

/* ============================== Video config ============================= */

function VideoChannelEditor({
  cameraId,
  cfg,
  canManage,
  onSaved,
}: {
  cameraId: string;
  cfg: VideoConfig;
  canManage: boolean;
  onSaved: () => void | Promise<void>;
}) {
  const [codec, setCodec] = useState(cfg.codec);
  const [width, setWidth] = useState(String(cfg.width));
  const [height, setHeight] = useState(String(cfg.height));
  // The device reports fps in centi-fps (2000 = 20fps); the editor works in whole fps.
  const [fps, setFps] = useState((cfg.fps / 100).toString());
  const [qc, setQc] = useState(cfg.quality_control);
  const [bitrate, setBitrate] = useState(String(cfg.bitrate));
  const [vbrCap, setVbrCap] = useState(String(cfg.vbr_upper_cap));
  const [gop, setGop] = useState(String(cfg.gop));

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  const isVbr = qc.toUpperCase() === "VBR";
  const label = cfg.channel_name?.trim() ? cfg.channel_name : `Channel ${cfg.channel_id}`;

  async function save(e: FormEvent) {
    e.preventDefault();
    setError(null);
    setSaved(false);
    const patch: VideoConfigPatch = {
      codec,
      width: Number(width) || cfg.width,
      height: Number(height) || cfg.height,
      fps: Math.round((Number(fps) || cfg.fps / 100) * 100),
      quality_control: qc,
      bitrate: Number(bitrate) || cfg.bitrate,
      vbr_upper_cap: Number(vbrCap) || cfg.vbr_upper_cap,
      gop: Number(gop) || cfg.gop,
    };
    setBusy(true);
    try {
      await api.putCameraVideoConfig(cameraId, cfg.channel_id, patch);
      setSaved(true);
      await onSaved();
    } catch (err) {
      setError(errMsg(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={save} className="space-y-3 rounded-md border border-line bg-canvas p-3">
      <div className="flex items-center justify-between gap-2">
        <span className="font-mono text-xs font-semibold text-fg">{label}</span>
        <span className="font-mono text-[10px] tabular-nums text-fg-muted">#{cfg.channel_id}</span>
      </div>

      <div className="grid grid-cols-2 gap-3">
        <Field label="Codec" htmlFor={`vc-codec-${cfg.channel_id}`}>
          <Select
            id={`vc-codec-${cfg.channel_id}`}
            value={codec}
            onChange={(e) => setCodec(e.target.value)}
            disabled={!canManage}
          >
            {withCurrent(CODEC_OPTIONS, codec).map((c) => (
              <option key={c} value={c}>
                {c}
              </option>
            ))}
          </Select>
        </Field>
        <Field label="Rate control" htmlFor={`vc-qc-${cfg.channel_id}`}>
          <Select
            id={`vc-qc-${cfg.channel_id}`}
            value={qc}
            onChange={(e) => setQc(e.target.value)}
            disabled={!canManage}
          >
            {withCurrent(QC_OPTIONS, qc).map((q) => (
              <option key={q} value={q}>
                {q}
              </option>
            ))}
          </Select>
        </Field>
      </div>

      <div className="grid grid-cols-3 gap-3">
        <Field label="Width" htmlFor={`vc-w-${cfg.channel_id}`}>
          <Input
            id={`vc-w-${cfg.channel_id}`}
            type="number"
            min={0}
            step={1}
            value={width}
            onChange={(e) => setWidth(e.target.value)}
            disabled={!canManage}
          />
        </Field>
        <Field label="Height" htmlFor={`vc-h-${cfg.channel_id}`}>
          <Input
            id={`vc-h-${cfg.channel_id}`}
            type="number"
            min={0}
            step={1}
            value={height}
            onChange={(e) => setHeight(e.target.value)}
            disabled={!canManage}
          />
        </Field>
        <Field label="FPS" htmlFor={`vc-fps-${cfg.channel_id}`}>
          <Input
            id={`vc-fps-${cfg.channel_id}`}
            type="number"
            min={0}
            step={1}
            value={fps}
            onChange={(e) => setFps(e.target.value)}
            disabled={!canManage}
          />
        </Field>
      </div>

      <div className="grid grid-cols-2 gap-3">
        <Field
          label={isVbr ? "Max bitrate (kbps)" : "Bitrate (kbps)"}
          htmlFor={`vc-br-${cfg.channel_id}`}
        >
          <Input
            id={`vc-br-${cfg.channel_id}`}
            type="number"
            min={0}
            step={1}
            value={isVbr ? vbrCap : bitrate}
            onChange={(e) => (isVbr ? setVbrCap(e.target.value) : setBitrate(e.target.value))}
            disabled={!canManage}
          />
        </Field>
        <Field label="GOP" htmlFor={`vc-gop-${cfg.channel_id}`} hint="Keyframe interval">
          <Input
            id={`vc-gop-${cfg.channel_id}`}
            type="number"
            min={0}
            step={1}
            value={gop}
            onChange={(e) => setGop(e.target.value)}
            disabled={!canManage}
          />
        </Field>
      </div>

      {canManage && (
        <Button type="submit" variant="primary" className="w-full" disabled={busy}>
          {busy ? "Saving…" : "Save channel"}
        </Button>
      )}
      {error && <ErrorNote>{error}</ErrorNote>}
      {saved && !error && <SavedNote>Channel saved.</SavedNote>}
    </form>
  );
}

function VideoConfigSection({ cameraId, canManage }: { cameraId: string; canManage: boolean }) {
  const cfgs = usePoll(() => api.listCameraVideoConfigs(cameraId), 0, [cameraId]);
  const list = cfgs.data ?? [];

  return (
    <Panel
      title="Video Encoding"
      subtitle="Per-channel encoder"
      actions={
        list.length > 0 ? (
          <span className="font-mono text-[11px] tabular-nums text-fg-muted">{list.length}</span>
        ) : undefined
      }
    >
      {list.length === 0 ? (
        <p className="font-mono text-xs text-fg-muted">
          {cfgs.error ?? "Reading streaming channels over ISAPI…"}
        </p>
      ) : (
        <div className="space-y-3">
          {!canManage && <ReadOnlyNote />}
          {list.map((cfg) => (
            <VideoChannelEditor
              key={cfg.channel_id}
              cameraId={cameraId}
              cfg={cfg}
              canManage={canManage}
              onSaved={() => cfgs.refresh()}
            />
          ))}
        </div>
      )}
    </Panel>
  );
}

/* ============================== Time / NTP =============================== */

function TimeNtpSection({ cameraId, canManage }: { cameraId: string; canManage: boolean }) {
  const time = usePoll(() => api.getCameraTimeConfig(cameraId), 0, [cameraId]);
  const ntp = usePoll(() => api.getCameraNtpConfig(cameraId), 0, [cameraId]);

  const [host, setHost] = useState("");
  const [port, setPort] = useState("123");
  const [hydrated, setHydrated] = useState(false);
  const [busy, setBusy] = useState<"sync" | "ntp" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);

  // One-shot hydrate of the NTP form from the first successful read (then leave edits alone).
  if (!hydrated && ntp.data) {
    setHost(ntp.data.host_name);
    setPort(String(ntp.data.port));
    setHydrated(true);
  }

  async function saveNtp(e: FormEvent) {
    e.preventDefault();
    setError(null);
    setSaved(null);
    const h = host.trim();
    if (!h) {
      setError("An NTP server hostname or IP is required.");
      return;
    }
    const p = Number(port);
    if (!Number.isFinite(p) || p <= 0) {
      setError("NTP port must be a positive number.");
      return;
    }
    setBusy("ntp");
    try {
      await api.putCameraNtpConfig(cameraId, {
        addressing_format: ntpAddressingFormat(h),
        host_name: h,
        port: Math.round(p),
      });
      await ntp.refresh();
      setSaved("NTP server saved.");
    } catch (err) {
      setError(errMsg(err));
    } finally {
      setBusy(null);
    }
  }

  async function syncNow() {
    setError(null);
    setSaved(null);
    setBusy("sync");
    try {
      await api.syncCameraTimeNow(cameraId);
      await time.refresh();
      setSaved("Clock switched to NTP and synced.");
    } catch (err) {
      setError(errMsg(err));
    } finally {
      setBusy(null);
    }
  }

  const t = time.data;

  return (
    <Panel
      title="Time & NTP"
      subtitle="Device clock"
      actions={
        canManage ? (
          <Button size="sm" disabled={busy != null} onClick={() => void syncNow()}>
            {busy === "sync" ? "Syncing…" : "Sync to NTP now"}
          </Button>
        ) : undefined
      }
    >
      {t ? (
        <div className="border-t border-line pt-1">
          <Meta label="Mode" value={t.time_mode} />
          <Meta label="Timezone" value={t.time_zone} />
          <Meta label="Local time" value={formatClock(t.local_time)} />
        </div>
      ) : (
        <p className="font-mono text-xs text-fg-muted">
          {time.error ?? "Reading device clock over ISAPI…"}
        </p>
      )}

      <form onSubmit={saveNtp} className="mt-4 space-y-3 border-t border-line pt-4">
        <div className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
          NTP server
        </div>
        <div className="flex items-end gap-2">
          <div className="min-w-0 flex-1">
            <Field label="Host / IP" htmlFor="ntp-host">
              <Input
                id="ntp-host"
                value={host}
                onChange={(e) => setHost(e.target.value)}
                placeholder="pool.ntp.org"
                disabled={!canManage}
              />
            </Field>
          </div>
          <div className="w-24 shrink-0">
            <Field label="Port" htmlFor="ntp-port">
              <Input
                id="ntp-port"
                type="number"
                min={1}
                step={1}
                value={port}
                onChange={(e) => setPort(e.target.value)}
                disabled={!canManage}
              />
            </Field>
          </div>
        </div>
        {canManage ? (
          <Button type="submit" variant="primary" className="w-full" disabled={busy != null}>
            {busy === "ntp" ? "Saving…" : "Save NTP server"}
          </Button>
        ) : (
          <ReadOnlyNote />
        )}
      </form>

      {error && <ErrorNote>{error}</ErrorNote>}
      {saved && !error && <SavedNote>{saved}</SavedNote>}
    </Panel>
  );
}

/* ============================== Enable ONVIF ============================= */

function OnvifEnableSection({ cameraId, canManage }: { cameraId: string; canManage: boolean }) {
  const onvif = usePoll(() => api.getCameraOnvifSettings(cameraId), 0, [cameraId]);
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<EnableOnvifResult | null>(null);
  const [disabled, setDisabled] = useState(false);

  const enabled = onvif.data?.onvif_enabled ?? false;

  async function enable() {
    setError(null);
    setResult(null);
    setDisabled(false);
    if (!password.trim()) {
      setError(`A password for the dedicated ${DEFAULT_ONVIF_USER} account is required.`);
      return;
    }
    setBusy(true);
    try {
      const cur: OnvifSettings = onvif.data ?? { onvif_enabled: false, isapi_enabled: true };
      await api.putCameraOnvifSettings(cameraId, { ...cur, onvif_enabled: true });
      const r = await api.ensureCameraOnvifUser(cameraId, { password: password.trim() });
      setResult(r);
      await onvif.refresh();
    } catch (err) {
      setError(errMsg(err));
    } finally {
      setBusy(false);
    }
  }

  async function disable() {
    setError(null);
    setResult(null);
    setBusy(true);
    try {
      const cur: OnvifSettings = onvif.data ?? { onvif_enabled: true, isapi_enabled: true };
      await api.putCameraOnvifSettings(cameraId, { ...cur, onvif_enabled: false });
      setDisabled(true);
      await onvif.refresh();
    } catch (err) {
      setError(errMsg(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Panel
      title="ONVIF Integration"
      subtitle="Profile S enablement"
      actions={<OnvifBadge enabled={onvif.data?.onvif_enabled} />}
    >
      {!canManage ? (
        <ReadOnlyNote />
      ) : (
        <div className="space-y-3">
          <Field
            label="ONVIF user password"
            htmlFor="onvif-pw"
            hint={`Provisions the dedicated ${DEFAULT_ONVIF_USER} operator account on enable.`}
          >
            <Input
              id="onvif-pw"
              type="password"
              autoComplete="new-password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder="••••••••"
              disabled={busy || enabled}
            />
          </Field>
          {enabled ? (
            <Button variant="danger" className="w-full" disabled={busy} onClick={() => void disable()}>
              {busy ? "Working…" : "Disable ONVIF"}
            </Button>
          ) : (
            <Button
              variant="primary"
              className="w-full"
              disabled={busy}
              onClick={() => void enable()}
            >
              {busy ? "Enabling…" : "Enable ONVIF + create user"}
            </Button>
          )}
        </div>
      )}

      {error && <ErrorNote>{error}</ErrorNote>}
      {result && !error && (
        <SavedNote>
          ONVIF enabled · {DEFAULT_ONVIF_USER}{" "}
          {result.created ? "account created." : "account already present."}
        </SavedNote>
      )}
      {disabled && !error && <SavedNote>ONVIF integration disabled.</SavedNote>}
    </Panel>
  );
}

/* ================================== OSD ================================== */

function OsdSection({ cameraId, canManage }: { cameraId: string; canManage: boolean }) {
  const osd = usePoll(() => api.getCameraOsdConfig(cameraId), 0, [cameraId]);
  const [datetime, setDatetime] = useState(false);
  const [channelName, setChannelName] = useState(false);
  const [hydrated, setHydrated] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  // One-shot hydrate from the first successful read.
  if (!hydrated && osd.data) {
    setDatetime(osd.data.datetime_enabled);
    setChannelName(osd.data.channel_name_enabled);
    setHydrated(true);
  }

  async function save() {
    setError(null);
    setSaved(false);
    setBusy(true);
    try {
      const cur: OsdConfig = osd.data ?? { datetime_enabled: false, channel_name_enabled: false };
      await api.putCameraOsdConfig(cameraId, {
        ...cur,
        datetime_enabled: datetime,
        channel_name_enabled: channelName,
      });
      await osd.refresh();
      setSaved(true);
    } catch (err) {
      setError(errMsg(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Panel title="On-Screen Display" subtitle="Burnt-in overlays">
      {osd.data == null && osd.error ? (
        <p className="font-mono text-xs text-fg-muted">{osd.error}</p>
      ) : (
        <div className="space-y-3">
          <ToggleField
            label="Date / time overlay"
            hint="Burn the device clock into the video"
            checked={datetime}
            onChange={setDatetime}
            disabled={!canManage}
          />
          <ToggleField
            label="Channel-name overlay"
            hint="Burn the channel name into the video"
            checked={channelName}
            onChange={setChannelName}
            disabled={!canManage}
          />
          {canManage ? (
            <Button
              variant="primary"
              className="w-full"
              disabled={busy}
              onClick={() => void save()}
            >
              {busy ? "Saving…" : "Save overlays"}
            </Button>
          ) : (
            <ReadOnlyNote />
          )}
        </div>
      )}
      {error && <ErrorNote>{error}</ErrorNote>}
      {saved && !error && <SavedNote>Overlays saved.</SavedNote>}
    </Panel>
  );
}

/* ========================= Single-camera config =========================== */

export function CameraConfigPanel({
  cameraId,
  canManage,
}: {
  cameraId: string;
  canManage: boolean;
}) {
  const camera = usePoll(() => api.getCamera(cameraId), 0, [cameraId]);
  const cam = camera.data;

  // Render nothing until the camera loads (graceful — no flashing placeholder).
  if (!cam) return null;

  const supported = cam.vendor.toLowerCase() === "hikvision" && !!cam.address;
  if (!supported) {
    return (
      <Panel title="Camera Configuration" subtitle="HikVision ISAPI">
        <p className="font-mono text-[11px] text-fg-muted">
          ISAPI config supported on HikVision cameras only.
        </p>
      </Panel>
    );
  }

  return (
    <div className="space-y-4">
      <DeviceInfoSection cameraId={cameraId} canManage={canManage} />
      <VideoConfigSection cameraId={cameraId} canManage={canManage} />
      <TimeNtpSection cameraId={cameraId} canManage={canManage} />
      <OnvifEnableSection cameraId={cameraId} canManage={canManage} />
      <OsdSection cameraId={cameraId} canManage={canManage} />
    </div>
  );
}

/* ============================== Bulk config =============================== */

type BulkActionType = BulkAction["type"];

const BULK_ACTIONS: { value: BulkActionType; label: string }[] = [
  { value: "enable_onvif", label: "Enable ONVIF on all" },
  { value: "sync_time", label: "Sync time on all" },
  { value: "set_ntp", label: "Set NTP" },
  { value: "set_video", label: "Set video" },
];

export function BulkConfigPanel({ canManage }: { canManage: boolean }) {
  const cameras = usePoll(() => api.listCameras(), 0, []);
  const list = cameras.data ?? [];

  const [actionType, setActionType] = useState<BulkActionType>("enable_onvif");
  const [selected, setSelected] = useState<Set<string>>(new Set());

  // enable_onvif
  const [onvifUser, setOnvifUser] = useState("");
  const [onvifPass, setOnvifPass] = useState("");
  // sync_time / set_ntp
  const [ntpServer, setNtpServer] = useState("");
  // set_video
  const [vChannel, setVChannel] = useState("");
  const [vCodec, setVCodec] = useState("");
  const [vWidth, setVWidth] = useState("");
  const [vHeight, setVHeight] = useState("");
  const [vFps, setVFps] = useState("");
  const [vQc, setVQc] = useState("");
  const [vBitrate, setVBitrate] = useState("");
  const [vGop, setVGop] = useState("");

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<BulkConfigResponse | null>(null);

  const nameById = new Map(list.map((c) => [c.id, c.name]));

  function toggleCamera(id: string) {
    setSelected((cur) => {
      const next = new Set(cur);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  function buildAction(): BulkAction | string {
    switch (actionType) {
      case "enable_onvif": {
        if (!onvifPass.trim()) return "An ONVIF user password is required.";
        return {
          type: "enable_onvif",
          onvif_username: onvifUser.trim() || undefined,
          onvif_password: onvifPass.trim(),
        };
      }
      case "sync_time":
        return { type: "sync_time", ntp_server: ntpServer.trim() ? ntpServer.trim() : null };
      case "set_ntp": {
        if (!ntpServer.trim()) return "An NTP server is required.";
        return { type: "set_ntp", ntp_server: ntpServer.trim() };
      }
      case "set_video": {
        const patch: VideoConfigPatch = {};
        if (vCodec.trim()) patch.codec = vCodec.trim();
        if (vWidth.trim()) patch.width = Number(vWidth);
        if (vHeight.trim()) patch.height = Number(vHeight);
        if (vFps.trim()) patch.fps = Math.round(Number(vFps) * 100);
        if (vQc.trim()) patch.quality_control = vQc.trim();
        if (vBitrate.trim()) patch.bitrate = Number(vBitrate);
        if (vGop.trim()) patch.gop = Number(vGop);
        if (Object.keys(patch).length === 0) return "Set at least one video field to apply.";
        return { type: "set_video", channel: vChannel.trim() ? Number(vChannel) : null, patch };
      }
    }
  }

  async function run() {
    setError(null);
    setResult(null);
    const action = buildAction();
    if (typeof action === "string") {
      setError(action);
      return;
    }
    const body: BulkConfigRequest = {
      camera_ids: selected.size > 0 ? [...selected] : null,
      action,
    };
    setBusy(true);
    try {
      const res = await api.bulkCameraConfig(body);
      setResult(res);
    } catch (err) {
      setError(errMsg(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Panel
      title="Bulk Camera Configuration"
      subtitle="Apply one action across the fleet"
      actions={
        canManage ? (
          <Button size="sm" variant="primary" disabled={busy} onClick={() => void run()}>
            {busy ? "Applying…" : "Apply"}
          </Button>
        ) : undefined
      }
    >
      {!canManage ? (
        <ReadOnlyNote />
      ) : (
        <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
          {/* Action + parameters */}
          <div className="space-y-3">
            <Field label="Action" htmlFor="bulk-action">
              <Select
                id="bulk-action"
                value={actionType}
                onChange={(e) => {
                  setActionType(e.target.value as BulkActionType);
                  setResult(null);
                  setError(null);
                }}
              >
                {BULK_ACTIONS.map((a) => (
                  <option key={a.value} value={a.value}>
                    {a.label}
                  </option>
                ))}
              </Select>
            </Field>

            {actionType === "enable_onvif" && (
              <div className="space-y-3">
                <Field label="ONVIF username" htmlFor="bulk-onvif-user" hint={`Blank = ${DEFAULT_ONVIF_USER}`}>
                  <Input
                    id="bulk-onvif-user"
                    value={onvifUser}
                    onChange={(e) => setOnvifUser(e.target.value)}
                    placeholder={DEFAULT_ONVIF_USER}
                  />
                </Field>
                <Field label="ONVIF password" htmlFor="bulk-onvif-pw">
                  <Input
                    id="bulk-onvif-pw"
                    type="password"
                    autoComplete="new-password"
                    value={onvifPass}
                    onChange={(e) => setOnvifPass(e.target.value)}
                    placeholder="••••••••"
                  />
                </Field>
              </div>
            )}

            {actionType === "sync_time" && (
              <Field
                label="NTP server"
                htmlFor="bulk-ntp-sync"
                hint="Optional — blank keeps each camera's current NTP server"
              >
                <Input
                  id="bulk-ntp-sync"
                  value={ntpServer}
                  onChange={(e) => setNtpServer(e.target.value)}
                  placeholder="pool.ntp.org"
                />
              </Field>
            )}

            {actionType === "set_ntp" && (
              <Field label="NTP server" htmlFor="bulk-ntp-set">
                <Input
                  id="bulk-ntp-set"
                  value={ntpServer}
                  onChange={(e) => setNtpServer(e.target.value)}
                  placeholder="pool.ntp.org"
                />
              </Field>
            )}

            {actionType === "set_video" && (
              <div className="space-y-3">
                <Field label="Channel" htmlFor="bulk-v-ch" hint="Blank = main channel">
                  <Input
                    id="bulk-v-ch"
                    type="number"
                    min={0}
                    step={1}
                    value={vChannel}
                    onChange={(e) => setVChannel(e.target.value)}
                    placeholder="main"
                  />
                </Field>
                <div className="grid grid-cols-2 gap-3">
                  <Field label="Codec" htmlFor="bulk-v-codec">
                    <Select id="bulk-v-codec" value={vCodec} onChange={(e) => setVCodec(e.target.value)}>
                      <option value="">— unchanged —</option>
                      {CODEC_OPTIONS.map((c) => (
                        <option key={c} value={c}>
                          {c}
                        </option>
                      ))}
                    </Select>
                  </Field>
                  <Field label="Rate control" htmlFor="bulk-v-qc">
                    <Select id="bulk-v-qc" value={vQc} onChange={(e) => setVQc(e.target.value)}>
                      <option value="">— unchanged —</option>
                      {QC_OPTIONS.map((q) => (
                        <option key={q} value={q}>
                          {q}
                        </option>
                      ))}
                    </Select>
                  </Field>
                </div>
                <div className="grid grid-cols-3 gap-3">
                  <Field label="Width" htmlFor="bulk-v-w">
                    <Input
                      id="bulk-v-w"
                      type="number"
                      min={0}
                      step={1}
                      value={vWidth}
                      onChange={(e) => setVWidth(e.target.value)}
                    />
                  </Field>
                  <Field label="Height" htmlFor="bulk-v-h">
                    <Input
                      id="bulk-v-h"
                      type="number"
                      min={0}
                      step={1}
                      value={vHeight}
                      onChange={(e) => setVHeight(e.target.value)}
                    />
                  </Field>
                  <Field label="FPS" htmlFor="bulk-v-fps">
                    <Input
                      id="bulk-v-fps"
                      type="number"
                      min={0}
                      step={1}
                      value={vFps}
                      onChange={(e) => setVFps(e.target.value)}
                    />
                  </Field>
                </div>
                <div className="grid grid-cols-2 gap-3">
                  <Field label="Bitrate (kbps)" htmlFor="bulk-v-br">
                    <Input
                      id="bulk-v-br"
                      type="number"
                      min={0}
                      step={1}
                      value={vBitrate}
                      onChange={(e) => setVBitrate(e.target.value)}
                    />
                  </Field>
                  <Field label="GOP" htmlFor="bulk-v-gop">
                    <Input
                      id="bulk-v-gop"
                      type="number"
                      min={0}
                      step={1}
                      value={vGop}
                      onChange={(e) => setVGop(e.target.value)}
                    />
                  </Field>
                </div>
              </div>
            )}
          </div>

          {/* Target cameras */}
          <div className="space-y-2">
            <div className="flex items-center justify-between">
              <span className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
                Target cameras
              </span>
              <div className="flex items-center gap-1.5">
                <Button size="sm" onClick={() => setSelected(new Set(list.map((c) => c.id)))}>
                  All
                </Button>
                <Button size="sm" onClick={() => setSelected(new Set())}>
                  Clear
                </Button>
              </div>
            </div>
            <p className="font-mono text-[10px] text-fg-muted">
              {selected.size === 0
                ? "None selected = every enabled camera."
                : `${selected.size} selected.`}
            </p>
            {list.length === 0 ? (
              <p className="font-mono text-xs text-fg-muted">
                {cameras.error ?? "No cameras registered."}
              </p>
            ) : (
              <ul className="-mr-1 max-h-60 space-y-1 overflow-y-auto pr-1">
                {list.map((c) => {
                  const on = selected.has(c.id);
                  return (
                    <li key={c.id}>
                      <label
                        className={cx(
                          "flex cursor-pointer items-center gap-2 rounded-md border px-2.5 py-1.5 transition-colors duration-150",
                          on
                            ? "border-accent/50 bg-accent/10"
                            : "border-line bg-canvas hover:border-[#34373e]",
                        )}
                      >
                        <input
                          type="checkbox"
                          className="accent-accent"
                          checked={on}
                          onChange={() => toggleCamera(c.id)}
                        />
                        <span className="min-w-0 flex-1">
                          <span className="block truncate text-xs font-medium text-fg">{c.name}</span>
                          <span className="block truncate font-mono text-[10px] text-fg-muted">
                            {c.vendor}
                            {c.address ? ` · ${c.address}` : ""}
                          </span>
                        </span>
                      </label>
                    </li>
                  );
                })}
              </ul>
            )}
          </div>
        </div>
      )}

      {error && (
        <p className="mt-3 font-mono text-xs text-danger">{error}</p>
      )}

      {result && (
        <div className="mt-4 border-t border-line pt-3">
          <div className="mb-2 flex items-center gap-3 font-mono text-[11px]">
            <span className="text-rec">{result.succeeded} ok</span>
            <span className="text-fg-muted/60">·</span>
            <span className={result.failed > 0 ? "text-danger" : "text-fg-muted"}>
              {result.failed} failed
            </span>
          </div>
          <div className="overflow-x-auto rounded-md border border-line">
            <table className="w-full border-collapse">
              <thead>
                <tr>
                  <th className="px-3 py-2 text-left font-mono text-[10px] font-medium uppercase tracking-micro text-fg-muted">
                    Camera
                  </th>
                  <th className="px-3 py-2 text-left font-mono text-[10px] font-medium uppercase tracking-micro text-fg-muted">
                    Result
                  </th>
                </tr>
              </thead>
              <tbody>
                {result.results.map((r) => (
                  <tr key={r.camera_id} className="border-t border-line">
                    <td className="px-3 py-2">
                      <span className="block truncate text-xs font-medium text-fg">
                        {nameById.get(r.camera_id) ?? r.camera_id}
                      </span>
                      <span className="block truncate font-mono text-[10px] text-fg-muted">
                        {r.camera_id}
                      </span>
                    </td>
                    <td className="px-3 py-2">
                      {r.ok ? (
                        <span className="font-mono text-[11px] text-rec">ok</span>
                      ) : (
                        <span
                          className="block max-w-[320px] truncate font-mono text-[11px] text-danger"
                          title={r.error ?? "error"}
                        >
                          {r.error ?? "error"}
                        </span>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </Panel>
  );
}
