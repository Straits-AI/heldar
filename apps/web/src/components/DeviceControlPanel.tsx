// Capability-driven camera device controls (issue #45).
//
// Renders ONLY the surfaces the kernel's device-control probe confirmed for this camera
// (capabilities.device_control): day/night (IR-cut), image/lighting, alarm/relay outputs, and the
// on-board ANPR toggle. Nothing here is vendor-hardcoded — a camera that exposes no control
// surface shows just the "Detect features" probe button, and probe failures never affect
// streaming/recording (the map is advisory).

import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../lib/api";
import { usePoll } from "../lib/usePoll";
import type {
  CameraView,
  DayNightConfig,
  DeviceControlCapabilities,
  ImageConfig,
} from "../lib/types";
import { Button, Field, Panel, Spinner } from "./ui";

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

const SELECT_CLS =
  "w-full rounded-md border border-line bg-raised px-2.5 py-1.5 font-mono text-xs text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent";

const SECTION_LABEL =
  "font-mono text-[10px] uppercase tracking-micro text-fg-muted";

export function DeviceControlPanel({
  camera,
  canManage,
  onCameraUpdated,
}: {
  camera: CameraView;
  canManage: boolean;
  onCameraUpdated: () => void;
}) {
  const cameraId = camera.id;
  const caps = usePoll(() => api.getCameraControlCapabilities(cameraId), 0, [cameraId]);
  const [probing, setProbing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function probe() {
    setProbing(true);
    setError(null);
    try {
      await api.probeCameraControl(cameraId);
      await caps.refresh();
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setProbing(false);
    }
  }

  const map: DeviceControlCapabilities = caps.data ?? {};
  const probed = Boolean(map.probed_at);
  const outputs = map.io_outputs ?? [];
  const detections = map.built_in_detections ?? [];
  const hasAny =
    Boolean(map.day_night) ||
    Boolean(map.image) ||
    outputs.length > 0 ||
    detections.length > 0 ||
    Boolean(map.native_anpr);

  // Auto-detect on first view: a camera that has never been probed (added before background
  // probing existed, or whose probe failed at add time) is probed once automatically, so the
  // panel populates without anyone pressing the button. Manual "Re-detect" stays for refreshes.
  const autoProbedRef = useRef(false);
  useEffect(() => {
    if (!caps.data || autoProbedRef.current || probing) return;
    if (!caps.data.probed_at && canManage) {
      autoProbedRef.current = true;
      void probe();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [caps.data, canManage]);

  return (
    <Panel
      title="Device"
      subtitle={probed ? `Features detected ${map.vendor ? `· ${map.vendor}` : ""}` : "On-camera controls"}
      actions={
        <Button size="sm" onClick={() => void probe()} disabled={!canManage || probing}>
          {probing ? "Probing…" : probed ? "Re-detect" : "Detect features"}
        </Button>
      }
    >
      {error && (
        <div className="mb-3 rounded-md border border-danger/40 bg-danger/10 px-2.5 py-2 font-mono text-[11px] text-red-300">
          {error}
        </div>
      )}

      {!probed && (
        <p className="font-mono text-xs text-fg-muted">
          Probe the camera to discover its on-device features (day/night, lighting, relay outputs,
          on-board ANPR). Controls appear only for what the device actually supports.
        </p>
      )}

      {probed && !hasAny && (
        <p className="font-mono text-xs text-fg-muted">
          No controllable device features detected for this camera
          {map.ptz ? " (PTZ is available in its own panel)" : ""}.
        </p>
      )}

      {map.day_night && <DayNightSection cameraId={cameraId} canManage={canManage} />}
      {map.image && (
        <ImageSection
          cameraId={cameraId}
          canManage={canManage}
          supplementModes={map.supplement_light_modes ?? []}
        />
      )}
      {detections.length > 0 && <BuiltinDetectionsSection detections={detections} />}
      {outputs.length > 0 && (
        <OutputsSection cameraId={cameraId} canManage={canManage} outputs={outputs} />
      )}
      {map.native_anpr && (
        <NativeAnprSection camera={camera} canManage={canManage} onSaved={onCameraUpdated} />
      )}
    </Panel>
  );
}

/* ------------------------------ day / night ------------------------------ */

function DayNightSection({ cameraId, canManage }: { cameraId: string; canManage: boolean }) {
  const [cfg, setCfg] = useState<DayNightConfig | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setCfg(await api.getCameraDayNight(cameraId));
      setError(null);
    } catch (e) {
      setError(errMsg(e));
    }
  }, [cameraId]);
  useEffect(() => {
    void load();
  }, [load]);

  async function save(patch: Partial<DayNightConfig>) {
    setBusy(true);
    setError(null);
    try {
      setCfg(await api.putCameraDayNight(cameraId, patch));
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="mt-4 border-t border-line pt-3 first:mt-0 first:border-t-0 first:pt-0">
      <div className={`${SECTION_LABEL} mb-2 flex items-center justify-between`}>
        <span>Day / Night (IR-cut)</span>
        {busy && <Spinner size={12} />}
      </div>
      {error && <p className="mb-2 font-mono text-[11px] text-danger">{error}</p>}
      {!cfg ? (
        <p className="font-mono text-[11px] text-fg-muted">Loading…</p>
      ) : (
        <div className="grid grid-cols-4 gap-1.5">
          {(["auto", "day", "night", "schedule"] as const).map((mode) => (
            <Button
              key={mode}
              size="sm"
              variant={cfg.mode === mode ? "primary" : "default"}
              disabled={!canManage || busy}
              onClick={() => void save({ mode })}
              className="w-full capitalize"
            >
              {mode}
            </Button>
          ))}
        </div>
      )}
    </div>
  );
}

/* ------------------------------ image / lighting ------------------------------ */

/** Friendly labels for the vendor supplement-light mode tokens ("smart night mode" etc.). */
const SUPPLEMENT_MODE_LABELS: Record<string, string> = {
  eventIntelligence: "Smart night (white light on events)",
  colorVuWhiteLight: "White light (full-color night)",
  irLight: "Infrared (black & white)",
  close: "Off",
};

function ImageSection({
  cameraId,
  canManage,
  supplementModes,
}: {
  cameraId: string;
  canManage: boolean;
  supplementModes: string[];
}) {
  const [cfg, setCfg] = useState<ImageConfig | null>(null);
  const [draft, setDraft] = useState<ImageConfig>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const c = await api.getCameraImageConfig(cameraId);
      setCfg(c);
      setDraft(c);
      setError(null);
    } catch (e) {
      setError(errMsg(e));
    }
  }, [cameraId]);
  useEffect(() => {
    void load();
  }, [load]);

  async function save() {
    setBusy(true);
    setError(null);
    try {
      const c = await api.putCameraImageConfig(cameraId, draft);
      setCfg(c);
      setDraft(c);
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setBusy(false);
    }
  }

  const dirty = cfg != null && JSON.stringify(cfg) !== JSON.stringify(draft);

  function level(
    key:
      | "brightness"
      | "contrast"
      | "saturation"
      | "wdr_level"
      | "white_light_brightness"
      | "ir_light_brightness",
    label: string,
    disabled = false,
  ) {
    const v = draft[key];
    if (v == null) return null; // device doesn't expose it
    return (
      <label key={key} className="block">
        <span className="flex items-baseline justify-between">
          <span className={SECTION_LABEL}>{label}</span>
          <span className="font-mono text-[11px] tabular-nums text-fg-secondary">{v}</span>
        </span>
        <input
          type="range"
          min={0}
          max={100}
          value={v}
          disabled={!canManage || busy || disabled}
          onChange={(e) => setDraft((d) => ({ ...d, [key]: Number(e.target.value) }))}
          className="mt-1 w-full accent-[var(--accent,#6ee7b7)]"
        />
      </label>
    );
  }

  return (
    <div className="mt-4 border-t border-line pt-3">
      <div className={`${SECTION_LABEL} mb-2 flex items-center justify-between`}>
        <span>Image &amp; Lighting</span>
        {busy && <Spinner size={12} />}
      </div>
      {error && <p className="mb-2 font-mono text-[11px] text-danger">{error}</p>}
      {!cfg ? (
        <p className="font-mono text-[11px] text-fg-muted">Loading…</p>
      ) : (
        <div className="space-y-2.5">
          {level("brightness", "Brightness")}
          {level("contrast", "Contrast")}
          {level("saturation", "Saturation")}
          {draft.wdr_mode != null && (
            <Field label="Wide dynamic range" htmlFor="img-wdr">
              <select
                id="img-wdr"
                className={SELECT_CLS}
                value={draft.wdr_mode}
                disabled={!canManage || busy}
                onChange={(e) => setDraft((d) => ({ ...d, wdr_mode: e.target.value }))}
              >
                <option value="open">On</option>
                <option value="close">Off</option>
                <option value="auto">Auto</option>
              </select>
            </Field>
          )}
          {draft.wdr_mode === "open" && level("wdr_level", "WDR level")}
          {draft.blc_enabled != null && (
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={draft.blc_enabled}
                disabled={!canManage || busy}
                onChange={(e) => setDraft((d) => ({ ...d, blc_enabled: e.target.checked }))}
              />
              <span className="font-mono text-xs text-fg-secondary">Backlight compensation</span>
            </label>
          )}
          {draft.supplement_light_mode != null && supplementModes.length > 0 && (
            <>
              <Field label="Night light (supplement light)" htmlFor="img-suplight">
                <select
                  id="img-suplight"
                  className={SELECT_CLS}
                  value={draft.supplement_light_mode}
                  disabled={!canManage || busy}
                  onChange={(e) =>
                    setDraft((d) => ({ ...d, supplement_light_mode: e.target.value }))
                  }
                >
                  {supplementModes.map((m) => (
                    <option key={m} value={m}>
                      {SUPPLEMENT_MODE_LABELS[m] ?? m}
                    </option>
                  ))}
                </select>
              </Field>
              {draft.supplement_brightness_mode != null && (
                <Field label="Light brightness control" htmlFor="img-suplight-reg">
                  <select
                    id="img-suplight-reg"
                    className={SELECT_CLS}
                    value={draft.supplement_brightness_mode}
                    disabled={!canManage || busy}
                    onChange={(e) =>
                      setDraft((d) => ({ ...d, supplement_brightness_mode: e.target.value }))
                    }
                  >
                    <option value="auto">Auto</option>
                    <option value="manual">Manual</option>
                  </select>
                </Field>
              )}
              {/* IR-only models still report a whiteLightBrightness field — only offer the
                  slider when the capability list actually includes a white-light mode. */}
              {supplementModes.some((m) => m === "colorVuWhiteLight" || m === "eventIntelligence") &&
                level(
                  "white_light_brightness",
                  "White light brightness",
                  draft.supplement_brightness_mode === "auto",
                )}
              {level(
                "ir_light_brightness",
                "IR light brightness",
                draft.supplement_brightness_mode === "auto",
              )}
            </>
          )}
          <div className="flex gap-2 pt-1">
            <Button
              size="sm"
              variant="primary"
              disabled={!canManage || busy || !dirty}
              onClick={() => void save()}
            >
              {busy ? "Saving…" : "Apply"}
            </Button>
            <Button size="sm" variant="ghost" disabled={busy || !dirty} onClick={() => cfg && setDraft(cfg)}>
              Reset
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}

/* ------------------------------ built-in detections ------------------------------ */

/** Friendly labels for the camera's own smart-event detection kinds. */
const DETECTION_LABELS: Record<string, string> = {
  motion: "Motion detection",
  line_crossing: "Line crossing",
  intrusion: "Intrusion (area)",
  region_entrance: "Region entrance",
  region_exiting: "Region exiting",
  loitering: "Loitering",
  face_detection: "Face detection",
  audio_detection: "Audio exception",
  scene_change: "Scene change",
  defocus: "Defocus",
  rapid_move: "Rapid movement",
  parking: "Parking",
  unattended_baggage: "Unattended object",
};

function BuiltinDetectionsSection({
  detections,
}: {
  detections: NonNullable<DeviceControlCapabilities["built_in_detections"]>;
}) {
  return (
    <div className="mt-4 border-t border-line pt-3">
      <div className={`${SECTION_LABEL} mb-2`}>Built-in detections (on-camera)</div>
      <div className="flex flex-wrap gap-1.5">
        {detections.map((d) => (
          <span
            key={d.kind}
            className="inline-flex items-center gap-1.5 rounded-md border border-line bg-canvas px-2 py-1 font-mono text-[11px] text-fg-secondary"
          >
            {DETECTION_LABELS[d.kind] ?? d.kind}
            {d.enabled != null && (
              <span className={d.enabled ? "text-emerald-400" : "text-fg-muted"}>
                {d.enabled ? "on" : "off"}
              </span>
            )}
          </span>
        ))}
      </div>
      <p className="mt-2 font-mono text-[10px] text-fg-muted">
        The camera's own smart events, currently configured on the device. Heldar's server-side
        zones (Zones panel above) work on any camera; ingesting these on-camera events is planned.
      </p>
    </div>
  );
}

/* ------------------------------ relay outputs ------------------------------ */

function OutputsSection({
  cameraId,
  canManage,
  outputs,
}: {
  cameraId: string;
  canManage: boolean;
  outputs: NonNullable<DeviceControlCapabilities["io_outputs"]>;
}) {
  const [busyPort, setBusyPort] = useState<number | null>(null);
  const [note, setNote] = useState<string | null>(null);

  async function pulse(port: number) {
    if (
      !window.confirm(
        `Pulse output ${port}? This fires the physical relay (e.g. a barrier) for ~1 second.`,
      )
    ) {
      return;
    }
    setBusyPort(port);
    setNote(null);
    try {
      const r = await api.pulseCameraIoOutput(cameraId, port);
      setNote(`Output ${port} pulsed for ${r.pulse_ms} ms.`);
    } catch (e) {
      setNote(`Pulse failed: ${errMsg(e)}`);
    } finally {
      setBusyPort(null);
    }
  }

  return (
    <div className="mt-4 border-t border-line pt-3">
      <div className={`${SECTION_LABEL} mb-2`}>Relay outputs</div>
      <ul className="space-y-1.5">
        {outputs.map((o) => (
          <li
            key={o.id}
            className="flex items-center justify-between gap-2 rounded-md border border-line bg-canvas px-2.5 py-1.5"
          >
            <span className="font-mono text-xs text-fg-secondary">
              #{o.id} {o.name ? `· ${o.name}` : ""}
              {o.default_state ? (
                <span className="ml-1 text-fg-muted">(idle {o.default_state})</span>
              ) : null}
            </span>
            <Button
              size="sm"
              disabled={!canManage || busyPort != null}
              onClick={() => void pulse(o.id)}
            >
              {busyPort === o.id ? "Pulsing…" : "Test pulse"}
            </Button>
          </li>
        ))}
      </ul>
      {note && <p className="mt-2 font-mono text-[11px] text-fg-muted">{note}</p>}
      <p className="mt-2 font-mono text-[10px] text-fg-muted">
        Barrier auto-open policy lives in the Entry module (gate policies).
      </p>
    </div>
  );
}

/* ------------------------------ on-board ANPR ------------------------------ */

function NativeAnprSection({
  camera,
  canManage,
  onSaved,
}: {
  camera: CameraView;
  canManage: boolean;
  onSaved: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function toggle() {
    setBusy(true);
    setError(null);
    try {
      await api.updateCamera(camera.id, { native_anpr_enabled: !camera.native_anpr_enabled });
      onSaved();
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="mt-4 border-t border-line pt-3">
      <div className={`${SECTION_LABEL} mb-2`}>On-board ANPR</div>
      <div className="flex items-center justify-between gap-2">
        <p className="font-mono text-[11px] text-fg-muted">
          Use the camera&apos;s built-in plate recognition as the ANPR source (feeds the Entry
          module directly; more accurate than server-side OCR at a gate lane and uses no GPU).
        </p>
        <Button
          size="sm"
          variant={camera.native_anpr_enabled ? "primary" : "default"}
          disabled={!canManage || busy}
          onClick={() => void toggle()}
        >
          {busy ? "…" : camera.native_anpr_enabled ? "Enabled" : "Enable"}
        </Button>
      </div>
      {camera.native_anpr_enabled && (
        <p className="mt-2 font-mono text-[10px] text-fg-muted">
          Tip: disable any server-side <span className="text-fg-secondary">anpr</span> AI task on
          this camera to avoid double sources (the AI panel above).
        </p>
      )}
      {error && <p className="mt-2 font-mono text-[11px] text-danger">{error}</p>}
    </div>
  );
}
