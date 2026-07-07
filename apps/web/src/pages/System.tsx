// Heldar Core — System / observability console.
// One screen where a non-developer operator can see system health and explain faults:
//   (1) storage horizon, (2) per-camera recorder health, (3) recent events feed.

import { useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { Link } from "react-router-dom";
import { api, ApiError } from "../lib/api";
import { usePoll } from "../lib/usePoll";
import type {
  AuditLogEntry,
  CameraStatus,
  CameraView,
  DiskStats,
  Principal,
  Severity,
  StorageReport,
  SystemInfo,
  VisionEvent,
} from "../lib/types";
import { BulkConfigPanel } from "../components/CameraConfigPanel";
import { WebhooksPanel } from "../components/WebhooksPanel";
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
import { formatBytes, formatClock, timeAgo } from "../lib/format";

const SEVERITY_COLOR: Record<Severity, string> = {
  info: "#71717a",
  warning: "#fbbf24",
  critical: "#ef4444",
};

// Anchor styled like a default <Button size="sm"> (anchors can't be Buttons).
const ANCHOR_BTN =
  "inline-flex items-center justify-center gap-1.5 rounded-md border border-line bg-raised px-2.5 py-1 font-mono text-[11px] font-medium text-fg-secondary transition-colors duration-150 hover:border-[#34373e] hover:bg-[#23262c] hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-canvas";

/* ------------------------------ helpers ------------------------------ */

/** Humanize a projected-days-remaining figure ("4.2d", "18h", ">1y", "—"). */
function formatHorizon(days: number | null): string {
  if (days == null || !Number.isFinite(days)) return "—";
  if (days >= 365) return ">1y";
  if (days >= 10) return `${Math.round(days)}d`;
  if (days >= 1) return `${days.toFixed(1)}d`;
  return `${Math.round(days * 24)}h`;
}

function horizonTone(days: number | null): "default" | "good" | "warn" | "bad" {
  if (days == null) return "default";
  if (days < 3) return "bad";
  if (days < 14) return "warn";
  return "good";
}

/* --------------------------- storage panel --------------------------- */

function DiskGauge({ disk }: { disk: DiskStats | null }) {
  if (!disk) {
    return (
      <div className="rounded-md border border-dashed border-line bg-canvas/40 px-3 py-4 font-mono text-[11px] text-fg-muted">
        Disk stats unavailable — the recordings filesystem could not be read.
      </div>
    );
  }
  const pct = Math.max(0, Math.min(100, disk.used_percent));
  const fill = pct > 90 ? "#ef4444" : pct > 75 ? "#fbbf24" : "#f59e0b";
  const pctText = pct > 90 ? "text-danger" : pct > 75 ? "text-connecting" : "text-fg";
  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-end justify-between gap-4">
        <div>
          <SectionLabel>Disk Usage</SectionLabel>
          <div className="mt-1 flex items-baseline gap-1.5">
            <span className={cx("font-mono text-3xl font-semibold tabular-nums", pctText)}>
              {pct.toFixed(1)}
            </span>
            <span className="font-mono text-sm text-fg-muted">%</span>
          </div>
        </div>
        <div className="text-right font-mono text-xs leading-relaxed text-fg-secondary">
          <div>
            <span className="tabular-nums">{formatBytes(disk.used_bytes)}</span> used
          </div>
          <div className="text-fg-muted">
            <span className="tabular-nums">{formatBytes(disk.total_bytes)}</span> total
          </div>
        </div>
      </div>
      <div className="h-2.5 w-full overflow-hidden rounded-full bg-line">
        <div
          className="h-full rounded-full transition-[width] duration-500"
          style={{ width: `${pct}%`, backgroundColor: fill }}
        />
      </div>
    </div>
  );
}

function StoragePanel({
  storage,
  loading,
  error,
}: {
  storage: StorageReport | null;
  loading: boolean;
  error: string | null;
}) {
  if (!storage) {
    return (
      <Panel title="Storage" subtitle="Disk horizon & recordings footprint">
        <PanelStatus loading={loading} error={error} label="storage telemetry" />
      </Panel>
    );
  }

  const days = storage.projected_days_remaining;
  const freeBytes = storage.disk?.free_bytes ?? null;
  const lowHorizon = days != null && days < 14;

  return (
    <Panel title="Storage" subtitle="Disk horizon & recordings footprint">
      <DiskGauge disk={storage.disk} />

      {lowHorizon && (
        <div
          className={cx(
            "mt-4 flex items-start gap-2 rounded-md border px-3 py-2 font-mono text-[11px] leading-relaxed",
            days != null && days < 3
              ? "border-danger/40 bg-danger/10 text-red-300"
              : "border-connecting/40 bg-connecting/10 text-amber-200",
          )}
        >
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
            className="mt-0.5 shrink-0"
          >
            <path d="M8 1.5l6.5 11.5H1.5z" />
            <path d="M8 6.5v3.5" />
            <path d="M8 11.6v.4" />
          </svg>
          <span>
            At the current write rate, free disk space runs out in about{" "}
            <span className="font-semibold">{formatHorizon(days)}</span>.
          </span>
        </div>
      )}

      <div className="mt-4 grid grid-cols-2 gap-x-4 gap-y-4 sm:grid-cols-3">
        <Stat
          label="Free space"
          value={freeBytes != null ? formatBytes(freeBytes) : "—"}
          tone={freeBytes != null && freeBytes < 5 * 1024 ** 3 ? "warn" : "default"}
        />
        <Stat label="Recordings" value={formatBytes(storage.recordings_bytes)} />
        <Stat label="Segments" value={storage.segment_count.toLocaleString()} />
        <Stat label="Write / day" value={formatBytes(storage.write_rate_bytes_per_day)} />
        <Stat label="Retention left" value={formatHorizon(days)} tone={horizonTone(days)} />
        <Stat label="Oldest segment" value={storage.oldest_segment ? timeAgo(storage.oldest_segment) : "—"} />
      </div>

      <div className="mt-4 border-t border-line pt-3 font-mono text-[10px] uppercase tracking-micro text-fg-muted">
        Coverage:&nbsp;
        <span className="text-fg-secondary normal-case">
          {storage.oldest_segment ? formatClock(storage.oldest_segment) : "—"}
        </span>
        &nbsp;→&nbsp;
        <span className="text-fg-secondary normal-case">
          {storage.newest_segment ? formatClock(storage.newest_segment) : "—"}
        </span>
      </div>
    </Panel>
  );
}

/* --------------------- recording disk-limit panel --------------------- */

function RecordingLimitsPanel({
  canAdmin,
  recordingsBytes,
}: {
  canAdmin: boolean;
  recordingsBytes: number | null;
}) {
  const limits = usePoll(() => api.getRetention(), 15000);
  const [editing, setEditing] = useState(false);
  const [maxGb, setMaxGb] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const data = limits.data;

  function startEdit() {
    setMaxGb(data ? String(Math.round(data.max_recordings_gb * 10) / 10) : "");
    setError(null);
    setEditing(true);
  }
  async function save() {
    const gb = parseFloat(maxGb);
    if (!Number.isFinite(gb) || gb <= 0) {
      setError("Enter a size in GB greater than 0.");
      return;
    }
    setSaving(true);
    setError(null);
    try {
      await api.setRetention({ max_recordings_gb: gb });
      await limits.refresh();
      setEditing(false);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  }

  const usedPct =
    data && recordingsBytes != null && data.max_recordings_bytes > 0
      ? Math.min(100, (recordingsBytes / data.max_recordings_bytes) * 100)
      : null;
  const barColor = usedPct != null && usedPct > 90 ? "#ef4444" : usedPct != null && usedPct > 75 ? "#fbbf24" : "#f59e0b";

  return (
    <Panel
      title="Recording limit"
      subtitle="Cap on total recordings — oldest footage is evicted to stay under it"
      actions={
        canAdmin && !editing && data ? (
          <Button size="sm" onClick={startEdit}>
            Edit
          </Button>
        ) : undefined
      }
    >
      {!data ? (
        <PanelStatus loading={limits.loading} error={limits.error} label="recording limit" />
      ) : editing ? (
        <div className="space-y-3">
          <label className="block">
            <SectionLabel>Maximum total size (GB)</SectionLabel>
            <input
              type="number"
              min="0.01"
              step="0.01"
              value={maxGb}
              onChange={(e) => setMaxGb(e.target.value)}
              className="mt-1.5 w-full rounded-md border border-line bg-canvas px-3 py-2 font-mono text-sm text-fg outline-none focus:border-accent"
              autoFocus
            />
          </label>
          {error && (
            <div className="rounded-md border border-danger/40 bg-danger/10 px-3 py-2 font-mono text-[11px] text-red-300">
              {error}
            </div>
          )}
          <div className="flex gap-2">
            <Button size="sm" variant="primary" disabled={saving} onClick={() => void save()}>
              {saving ? <Spinner size={13} /> : "Save"}
            </Button>
            <Button size="sm" disabled={saving} onClick={() => setEditing(false)}>
              Cancel
            </Button>
          </div>
        </div>
      ) : (
        <div className="space-y-3">
          <div className="flex items-end justify-between gap-4">
            <div>
              <SectionLabel>Maximum size</SectionLabel>
              <div className="mt-1 font-mono text-3xl font-semibold tabular-nums text-fg">
                {Math.round(data.max_recordings_gb)}
                <span className="ml-1 text-sm text-fg-muted">GB</span>
              </div>
            </div>
            <div className="text-right font-mono text-[11px] leading-relaxed text-fg-muted">
              <div>{data.max_overridden ? "operator-set" : "default (env)"}</div>
              <div className="mt-0.5">free-disk floor {Math.round(data.min_free_disk_gb)} GB</div>
            </div>
          </div>
          {usedPct != null && (
            <div>
              <div className="h-2.5 w-full overflow-hidden rounded-full bg-line">
                <div
                  className="h-full rounded-full transition-[width] duration-500"
                  style={{ width: `${usedPct}%`, backgroundColor: barColor }}
                />
              </div>
              <div className="mt-1 font-mono text-[11px] text-fg-muted">
                {recordingsBytes != null ? formatBytes(recordingsBytes) : "—"} of{" "}
                {Math.round(data.max_recordings_gb)} GB used ({usedPct.toFixed(0)}%)
              </div>
            </div>
          )}
          <p className="font-mono text-[11px] leading-relaxed text-fg-muted">
            When recordings exceed this, the retention sweeper deletes the oldest segments first
            (evidence-locked footage is never touched), so the disk can&apos;t fill up.
          </p>
        </div>
      )}
    </Panel>
  );
}

/* ------------------------- camera health table ------------------------ */

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

function HealthRow({ status, name }: { status: CameraStatus; name: string }) {
  return (
    <tr className="border-t border-line transition-colors duration-150 hover:bg-raised/40">
      <td className="px-3 py-2.5">
        <Link
          to={`/cameras/${encodeURIComponent(status.camera_id)}`}
          className="group flex items-center gap-2.5"
        >
          <StatusLed state={status.state} />
          <span className="min-w-0">
            <span className="block truncate text-sm font-medium text-fg group-hover:text-accent-soft">
              {name}
            </span>
            <span className="block truncate font-mono text-[10px] text-fg-muted">
              {status.camera_id}
            </span>
          </span>
        </Link>
      </td>
      <td className="px-3 py-2.5">
        <StatusPill state={status.state} />
      </td>
      <td className="whitespace-nowrap px-3 py-2.5 font-mono text-xs tabular-nums text-fg-secondary">
        {timeAgo(status.last_segment_at)}
      </td>
      <td
        className={cx(
          "px-3 py-2.5 text-right font-mono text-xs tabular-nums",
          status.reconnect_count > 0 ? "text-connecting" : "text-fg-secondary",
        )}
      >
        {status.reconnect_count}
      </td>
      <td className="px-3 py-2.5 text-right font-mono text-xs tabular-nums text-fg-secondary">
        {status.segments_written.toLocaleString()}
      </td>
      <td className="px-3 py-2.5 text-right font-mono text-xs tabular-nums text-fg-secondary">
        {status.bitrate_kbps != null ? Math.round(status.bitrate_kbps).toLocaleString() : "—"}
      </td>
      <td className="px-3 py-2.5 text-right font-mono text-xs tabular-nums text-fg-secondary">
        {status.fps_observed != null ? status.fps_observed.toFixed(1) : "—"}
      </td>
      <td className="max-w-[260px] px-3 py-2.5">
        {status.last_error ? (
          <span className="block truncate font-mono text-[11px] text-danger" title={status.last_error}>
            {status.last_error}
          </span>
        ) : (
          <span className="font-mono text-[11px] text-fg-muted">—</span>
        )}
      </td>
    </tr>
  );
}

function HealthPanel({
  statuses,
  cameras,
  loading,
  error,
}: {
  statuses: CameraStatus[] | null;
  cameras: CameraView[] | null;
  loading: boolean;
  error: string | null;
}) {
  const nameById = useMemo(() => {
    const map = new Map<string, string>();
    for (const c of cameras ?? []) map.set(c.id, c.name);
    return map;
  }, [cameras]);

  const rows = statuses ?? [];

  return (
    <Panel
      title="Recorder Health"
      subtitle="Per-camera capture telemetry"
      padded={false}
      actions={
        rows.length > 0 ? (
          <span className="font-mono text-[11px] tabular-nums text-fg-muted">{rows.length}</span>
        ) : undefined
      }
    >
      {rows.length === 0 ? (
        <div className="p-4">
          {loading && !statuses ? (
            <PanelStatus loading={loading} error={error} label="recorder health" />
          ) : (
            <EmptyState
              title="No recorder activity"
              hint="No camera recorders have reported health yet. Enable recording on a camera to populate this table."
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
                <Th>Last segment</Th>
                <Th className="text-right">Reconn</Th>
                <Th className="text-right">Segments</Th>
                <Th className="text-right">kbps</Th>
                <Th className="text-right">fps</Th>
                <Th>Last error</Th>
              </tr>
            </thead>
            <tbody>
              {rows.map((s) => (
                <HealthRow key={s.camera_id} status={s} name={nameById.get(s.camera_id) ?? s.camera_id} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Panel>
  );
}

/* ----------------------------- events feed ---------------------------- */

function EventRow({ ev, name }: { ev: VisionEvent; name?: string }) {
  const payloadKeys = Object.keys(ev.payload ?? {});
  const color = SEVERITY_COLOR[ev.severity] ?? SEVERITY_COLOR.info;
  return (
    <li className="rounded-md border border-line bg-canvas px-2.5 py-2 transition-colors duration-150 hover:border-[#34373e]">
      <div className="flex items-center justify-between gap-2">
        <span className="truncate text-xs font-semibold text-fg">{ev.event_type}</span>
        <span
          className="shrink-0 rounded border px-1.5 py-0.5 font-mono text-[9px] font-semibold uppercase tracking-micro"
          style={{ color, borderColor: `${color}55`, backgroundColor: `${color}1a` }}
        >
          {ev.severity}
        </span>
      </div>
      <div className="mt-0.5 flex flex-wrap items-center gap-x-2 font-mono text-[10px] text-fg-muted">
        <span>{formatClock(ev.timestamp)}</span>
        {ev.camera_id && (
          <>
            <span className="text-fg-muted/60">·</span>
            <span className="truncate text-fg-secondary">{name ?? ev.camera_id}</span>
          </>
        )}
      </div>
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

function EventsPanel({
  events,
  cameras,
  loading,
  error,
}: {
  events: VisionEvent[] | null;
  cameras: CameraView[] | null;
  loading: boolean;
  error: string | null;
}) {
  const nameById = useMemo(() => {
    const map = new Map<string, string>();
    for (const c of cameras ?? []) map.set(c.id, c.name);
    return map;
  }, [cameras]);

  const list = events ?? [];

  return (
    <Panel
      title="Recent Events"
      subtitle="System & camera activity"
      actions={
        list.length > 0 ? (
          <span className="font-mono text-[11px] tabular-nums text-fg-muted">{list.length}</span>
        ) : undefined
      }
    >
      {list.length === 0 ? (
        loading && !events ? (
          <PanelStatus loading={loading} error={error} label="events" />
        ) : (
          <p className="font-mono text-xs text-fg-muted">No events recorded.</p>
        )
      ) : (
        <ul className="-mr-1 max-h-[640px] space-y-1.5 overflow-y-auto pr-1">
          {list.map((ev) => (
            <EventRow key={ev.id} ev={ev} name={ev.camera_id ? nameById.get(ev.camera_id) : undefined} />
          ))}
        </ul>
      )}
    </Panel>
  );
}

/* ------------------------- shared status block ------------------------ */

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

/* ---- System status: remote-access overlay + disk health + transcode engine ---- */

function StatusRow({
  label,
  state,
  value,
  hint,
}: {
  label: ReactNode;
  state: string;
  value: ReactNode;
  hint?: ReactNode;
}) {
  return (
    <div className="flex items-start justify-between gap-3 border-b border-line/60 py-2.5 last:border-0">
      <div className="flex items-center gap-2">
        <StatusLed state={state} />
        <span className="text-sm text-fg">{label}</span>
      </div>
      <div className="min-w-0 text-right">
        <div className="font-mono text-xs text-fg-secondary">{value}</div>
        {hint != null && <div className="mt-0.5 text-[11px] text-fg-muted">{hint}</div>}
      </div>
    </div>
  );
}

function SystemStatusPanel({
  info,
  loading,
  error,
}: {
  info: SystemInfo | null;
  loading: boolean;
  error: string | null;
}) {
  return (
    <Panel title="System status" subtitle="Remote access · disk health · transcode">
      {!info ? (
        <PanelStatus loading={loading} error={error} label="system status" />
      ) : (
        <div>
          {/* Remote access */}
          <StatusRow
            label="Remote access"
            state={
              !info.remote_access.enabled ? "disabled" : info.remote_access.up ? "recording" : "error"
            }
            value={
              !info.remote_access.enabled
                ? "LAN-only"
                : `${info.remote_access.kind}${info.remote_access.iface ? ` · ${info.remote_access.iface}` : ""}`
            }
            hint={info.remote_access.enabled ? info.remote_access.note : undefined}
          />
          {/* Disk / array health */}
          <StatusRow
            label="Disk health"
            state={info.disk_health_ok ? "recording" : "error"}
            value={info.disk_health_ok ? "OK" : "Alert"}
            hint={
              info.last_disk_alert_at
                ? `last alert ${timeAgo(info.last_disk_alert_at)}`
                : "no SMART/RAID alerts"
            }
          />
          {/* Live transcode engine */}
          <StatusRow
            label="Transcode"
            state={info.live_transcode_engine === "software" ? "connecting" : "recording"}
            value={info.live_transcode_engine}
            hint={info.live_transcode_engine === "software" ? "CPU (libx264)" : "hardware-accelerated"}
          />
        </div>
      )}
    </Panel>
  );
}

/* ---- Audit log viewer (manager+) — who did what ---- */

function AuditPanel() {
  const audit = usePoll(() => api.listAudit({ limit: 100 }), 30000);
  return (
    <Panel
      title="Audit log"
      subtitle="Privileged actions (who did what)"
      actions={
        <Button size="sm" disabled={audit.loading} onClick={() => void audit.refresh()}>
          {audit.loading ? <Spinner size={13} /> : "Refresh"}
        </Button>
      }
    >
      {audit.error ? (
        <PanelStatus loading={false} error={audit.error} label="audit log" />
      ) : !audit.data || audit.data.length === 0 ? (
        <EmptyState title="No audit entries" hint="Privileged actions (config, registry, plugins) are recorded here." />
      ) : (
        <div className="max-h-96 overflow-y-auto">
          {audit.data.map((e: AuditLogEntry) => (
            <div key={e.id} className="flex items-start justify-between gap-3 border-b border-line/60 py-2 last:border-0">
              <div className="min-w-0">
                <div className="flex items-center gap-2">
                  <span className="font-mono text-xs font-semibold text-accent">{e.action}</span>
                  {e.target_type && (
                    <span className="truncate font-mono text-[11px] text-fg-secondary">
                      {e.target_type}
                      {e.target_id ? `:${e.target_id}` : ""}
                    </span>
                  )}
                </div>
                <div className="mt-0.5 font-mono text-[10px] uppercase tracking-micro text-fg-muted">
                  {e.actor_name ?? e.actor}
                  {e.role ? ` · ${e.role}` : ""}
                </div>
              </div>
              <span className="shrink-0 font-mono text-[10px] text-fg-muted">{timeAgo(e.created_at)}</span>
            </div>
          ))}
        </div>
      )}
    </Panel>
  );
}

/* -------------------------------- page -------------------------------- */

export function System() {
  const system = usePoll(() => api.system(), 5000);
  const health = usePoll(() => api.listHealth(), 5000);
  const cameras = usePoll(() => api.listCameras(), 30000);
  const events = usePoll(() => api.listEvents({ limit: 50 }), 10000);

  // ---- Principal / manager gating ----
  // Bulk camera-config actions are manager+. When auth is disabled the server returns the `system`
  // admin principal; when unauthenticated the controls stay gated off.
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

  const [refreshing, setRefreshing] = useState(false);
  const refresh = () => {
    setRefreshing(true);
    void Promise.all([
      system.refresh(),
      health.refresh(),
      cameras.refresh(),
      events.refresh(),
    ]).finally(() => setRefreshing(false));
  };

  return (
    <div className="mx-auto max-w-[1600px] px-4 py-6 sm:px-6">
      {/* ---- Header ---- */}
      <header className="animate-rise">
        <div className="flex flex-wrap items-end justify-between gap-4">
          <div className="min-w-0">
            <SectionLabel>Operations · System</SectionLabel>
            <h1 className="mt-1 font-display text-2xl font-extrabold tracking-tight text-fg">
              System Health
            </h1>
          </div>
          <Button onClick={refresh} disabled={refreshing} aria-label="Refresh system">
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
      </header>

      {/* ---- Body ---- */}
      <div className="mt-5 grid grid-cols-1 gap-4 lg:grid-cols-3">
        {/* Main column: storage + recorder health */}
        <div className="stagger space-y-4 lg:col-span-2">
          <StoragePanel
            storage={system.data?.storage ?? null}
            loading={system.loading}
            error={system.error}
          />
          <RecordingLimitsPanel
            canAdmin={principal?.role === "admin"}
            recordingsBytes={system.data?.recordings_bytes ?? null}
          />
          <HealthPanel
            statuses={health.data}
            cameras={cameras.data}
            loading={health.loading}
            error={health.error}
          />
        </div>

        {/* Side column: system status + events feed */}
        <div className="stagger space-y-4">
          <SystemStatusPanel info={system.data} loading={system.loading} error={system.error} />
          <EventsPanel
            events={events.data}
            cameras={cameras.data}
            loading={events.loading}
            error={events.error}
          />
        </div>
      </div>

      {/* ---- Webhooks (event-delivery subscriptions) ---- */}
      <div className="mt-4 stagger">
        <WebhooksPanel canManage={canManage} />
      </div>

      {/* ---- Full-width: bulk camera configuration ---- */}
      <div className="mt-4 stagger">
        <BulkConfigPanel canManage={canManage} />
      </div>

      {/* ---- Audit log (manager+; the endpoint enforces this too) ---- */}
      {canManage && (
        <div className="mt-4 stagger">
          <AuditPanel />
        </div>
      )}

      {/* ---- Footer: raw endpoints ---- */}
      <footer className="mt-6 flex flex-wrap items-center gap-3 border-t border-line pt-4">
        <span className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
          Raw endpoints
        </span>
        <a className={ANCHOR_BTN} href="/metrics" target="_blank" rel="noreferrer">
          /metrics
        </a>
        <a className={ANCHOR_BTN} href="/healthz" target="_blank" rel="noreferrer">
          /healthz
        </a>
        <a className={ANCHOR_BTN} href="/readyz" target="_blank" rel="noreferrer">
          /readyz
        </a>
        <span className="font-mono text-[10px] text-fg-muted">
          Prometheus text · liveness · readiness (200/503)
        </span>
      </footer>
    </div>
  );
}

export default System;
