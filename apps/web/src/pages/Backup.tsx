// Heldar Core — Backup & Archive admin console.
// One screen to manage backup destinations (local/SFTP/FTP/S3), scheduled policies that ship a
// camera selection's recent footage to a destination, the job ledger, and on-demand archive
// (.zip) exports. Listings are readable by any authenticated principal; all mutations are
// manager+ (the API enforces this — the UI mirrors it by gating the action controls).

import { useEffect, useMemo, useState } from "react";
import type { FormEvent, ReactNode } from "react";
import { api, ApiError } from "../lib/api";
import { usePoll } from "../lib/usePoll";
import {
  Button,
  EmptyState,
  Field,
  Input,
  Panel,
  SectionLabel,
  Select,
  Spinner,
  Stat,
  cx,
} from "../components/ui";
import { formatBytes, localInputToIso, timeAgo } from "../lib/format";
import type {
  ArchiveExportRequest,
  BackupDestinationCreate,
  BackupDestinationView,
  BackupJob,
  BackupJobStatus,
  BackupKind,
  BackupPolicy,
  BackupPolicyCreate,
  BackupTestResult,
  CameraView,
  Principal,
} from "../lib/types";

/* ====================================================================== */
/* Palettes + static config                                               */
/* ====================================================================== */

const KINDS: BackupKind[] = ["local", "sftp", "ftp", "s3"];

const JOB_STATUS_COLOR: Record<BackupJobStatus, string> = {
  pending: "#71717a",
  running: "#fbbf24",
  completed: "#10b981",
  error: "#ef4444",
};

/** Per-kind config field schema. `secret` fields round-trip the `***` mask on edit. */
interface CfgField {
  key: string;
  label: string;
  secret?: boolean;
  type?: "text" | "number";
  placeholder?: string;
}

const KIND_FIELDS: Record<BackupKind, CfgField[]> = {
  local: [{ key: "path", label: "Path", placeholder: "/mnt/nas/heldar" }],
  sftp: [
    { key: "host", label: "Host", placeholder: "10.0.0.5" },
    { key: "port", label: "Port", type: "number", placeholder: "22" },
    { key: "user", label: "User", placeholder: "backup" },
    { key: "pass", label: "Password", secret: true },
    { key: "path", label: "Remote path", placeholder: "/backups/heldar" },
  ],
  ftp: [
    { key: "host", label: "Host", placeholder: "10.0.0.5" },
    { key: "port", label: "Port", type: "number", placeholder: "21" },
    { key: "user", label: "User", placeholder: "backup" },
    { key: "pass", label: "Password", secret: true },
    { key: "path", label: "Remote path", placeholder: "/backups/heldar" },
  ],
  s3: [
    { key: "bucket", label: "Bucket", placeholder: "heldar-evidence" },
    { key: "prefix", label: "Prefix", placeholder: "site-a/" },
    { key: "access_key", label: "Access key" },
    { key: "secret_key", label: "Secret key", secret: true },
    { key: "endpoint", label: "Endpoint", placeholder: "https://s3.amazonaws.com" },
    { key: "region", label: "Region", placeholder: "us-east-1" },
  ],
};

// Anchor styled like a default <Button size="sm"> (anchors can't be Buttons).
const ANCHOR_BTN =
  "inline-flex items-center justify-center gap-1.5 rounded-md border border-line bg-raised px-2.5 py-1 font-mono text-[11px] font-medium text-fg-secondary transition-colors duration-150 hover:border-[#34373e] hover:bg-[#23262c] hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-canvas";

/* ====================================================================== */
/* Small shared bits (mirrors Entry.tsx conventions)                       */
/* ====================================================================== */

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

function Td({ children, className }: { children?: ReactNode; className?: string }) {
  return <td className={cx("px-3 py-2.5 align-top", className)}>{children}</td>;
}

function Pill({ label, color }: { label: ReactNode; color: string }) {
  return (
    <span
      className="inline-flex shrink-0 items-center rounded border px-1.5 py-0.5 font-mono text-[9px] font-semibold uppercase tracking-micro leading-none"
      style={{ color, borderColor: `${color}55`, backgroundColor: `${color}1a` }}
    >
      {label}
    </span>
  );
}

function WarnIcon({ className }: { className?: string }) {
  return (
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
      className={className}
    >
      <path d="M8 1.5l6.5 11.5H1.5z" />
      <path d="M8 6.5v3.5" />
      <path d="M8 11.6v.4" />
    </svg>
  );
}

function ErrorNote({ children }: { children: ReactNode }) {
  return (
    <div
      role="alert"
      className="flex items-start gap-2 rounded-md border border-danger/40 bg-danger/10 px-3 py-2 font-mono text-xs text-red-300"
    >
      <WarnIcon className="mt-0.5 shrink-0" />
      <span className="break-words">{children}</span>
    </div>
  );
}

function Loading({ label }: { label: string }) {
  return (
    <div className="flex items-center gap-2 px-1 py-2 font-mono text-xs text-fg-muted">
      <Spinner size={14} /> Loading {label}…
    </div>
  );
}

/** Small read-only badge shown to non-managers in place of an action control. */
function ManagerOnly() {
  return <span className="font-mono text-[10px] text-fg-muted">manager+</span>;
}

/* ====================================================================== */
/* Camera multi-select (empty selection = all cameras)                     */
/* ====================================================================== */

function CameraMultiSelect({
  cameras,
  selected,
  onToggle,
  disabled,
}: {
  cameras: CameraView[];
  selected: string[];
  onToggle: (id: string) => void;
  disabled?: boolean;
}) {
  return (
    <div className="max-h-44 space-y-1 overflow-y-auto rounded-md border border-line bg-canvas p-2">
      {cameras.length === 0 ? (
        <p className="px-1 py-1 font-mono text-[11px] text-fg-muted">No cameras registered.</p>
      ) : (
        cameras.map((c) => (
          <label
            key={c.id}
            className="flex cursor-pointer items-center gap-2 rounded px-1.5 py-1 transition-colors duration-150 hover:bg-raised/60"
          >
            <input
              type="checkbox"
              className="h-3.5 w-3.5 shrink-0 accent-accent"
              checked={selected.includes(c.id)}
              onChange={() => onToggle(c.id)}
              disabled={disabled}
            />
            <span className="min-w-0 truncate text-xs text-fg">{c.name}</span>
            <span className="ml-auto shrink-0 font-mono text-[10px] text-fg-muted">{c.id}</span>
          </label>
        ))
      )}
    </div>
  );
}

/* ====================================================================== */
/* Jobs table (shared by Jobs tab + Archive exports list)                  */
/* ====================================================================== */

function camerasLabel(ids: string[]): string {
  if (!ids || ids.length === 0) return "all";
  if (ids.length <= 2) return ids.join(", ");
  return `${ids.length} cameras`;
}

function JobsTable({
  jobs,
  canManage,
  deletingId,
  onDelete,
}: {
  jobs: BackupJob[];
  canManage: boolean;
  deletingId: string | null;
  onDelete: (job: BackupJob) => void;
}) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full border-collapse">
        <thead>
          <tr>
            <Th>Status</Th>
            <Th>Kind</Th>
            <Th>Cameras</Th>
            <Th className="text-right">Files</Th>
            <Th className="text-right">Bytes</Th>
            <Th>Started</Th>
            <Th>Finished</Th>
            <Th>Output</Th>
            <Th className="text-right">Action</Th>
          </tr>
        </thead>
        <tbody>
          {jobs.map((j) => (
            <tr
              key={j.id}
              className="border-t border-line transition-colors duration-150 hover:bg-raised/40"
            >
              <Td>
                <Pill label={j.status} color={JOB_STATUS_COLOR[j.status] ?? "#71717a"} />
                {j.error && (
                  <div
                    className="mt-1 max-w-[220px] truncate font-mono text-[10px] text-danger"
                    title={j.error}
                  >
                    {j.error}
                  </div>
                )}
              </Td>
              <Td>
                <span className="font-mono text-[11px] text-fg-secondary">{j.kind}</span>
              </Td>
              <Td>
                <span className="font-mono text-[11px] text-fg-secondary">
                  {camerasLabel(j.camera_ids)}
                </span>
              </Td>
              <Td className="text-right">
                <span className="font-mono text-xs tabular-nums text-fg-secondary">
                  {j.files_copied}/{j.files_total}
                </span>
              </Td>
              <Td className="text-right">
                <span className="font-mono text-xs tabular-nums text-fg-secondary">
                  {formatBytes(j.bytes_copied)}
                </span>
              </Td>
              <Td>
                <span className="whitespace-nowrap font-mono text-[11px] text-fg-muted">
                  {j.started_at ? timeAgo(j.started_at) : "—"}
                </span>
              </Td>
              <Td>
                <span className="whitespace-nowrap font-mono text-[11px] text-fg-muted">
                  {j.finished_at ? timeAgo(j.finished_at) : "—"}
                </span>
              </Td>
              <Td>
                {j.output_url ? (
                  <a className={ANCHOR_BTN} href={j.output_url} download>
                    Download
                  </a>
                ) : (
                  <span className="font-mono text-[11px] text-fg-muted">—</span>
                )}
              </Td>
              <Td className="text-right">
                {canManage ? (
                  <Button
                    size="sm"
                    variant="danger"
                    disabled={deletingId === j.id}
                    onClick={() => onDelete(j)}
                  >
                    Delete
                  </Button>
                ) : (
                  <ManagerOnly />
                )}
              </Td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/* ====================================================================== */
/* Tab: Destinations                                                       */
/* ====================================================================== */

function DestinationsTab({ canManage }: { canManage: boolean }) {
  const dests = usePoll(() => api.listBackupDestinations(), 0);

  const [editingId, setEditingId] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [kind, setKind] = useState<BackupKind>("local");
  const [enabled, setEnabled] = useState(true);
  const [cfg, setCfg] = useState<Record<string, string>>({});
  const [submitting, setSubmitting] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);

  const [testing, setTesting] = useState<string | null>(null);
  const [testResult, setTestResult] = useState<Record<string, BackupTestResult>>({});
  const [rowBusy, setRowBusy] = useState<string | null>(null);

  function resetForm() {
    setEditingId(null);
    setName("");
    setKind("local");
    setEnabled(true);
    setCfg({});
    setFormError(null);
  }

  function startEdit(d: BackupDestinationView) {
    setEditingId(d.id);
    setName(d.name);
    setKind(d.kind);
    setEnabled(d.enabled);
    const next: Record<string, string> = {};
    for (const f of KIND_FIELDS[d.kind]) {
      const v = (d.config ?? {})[f.key];
      if (v != null) next[f.key] = String(v);
    }
    setCfg(next);
    setFormError(null);
  }

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!name.trim()) {
      setFormError("Name is required.");
      return;
    }
    const config: Record<string, unknown> = {};
    for (const f of KIND_FIELDS[kind]) {
      const raw = (cfg[f.key] ?? "").trim();
      if (!raw) continue;
      if (f.type === "number") {
        const n = Number(raw);
        if (!Number.isNaN(n)) config[f.key] = n;
      } else {
        config[f.key] = raw;
      }
    }
    const body: BackupDestinationCreate = { name: name.trim(), kind, config, enabled };
    setSubmitting(true);
    setFormError(null);
    try {
      if (editingId) await api.updateBackupDestination(editingId, body);
      else await api.createBackupDestination(body);
      resetForm();
      await dests.refresh();
    } catch (err) {
      setFormError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  }

  async function runTest(d: BackupDestinationView) {
    setTesting(d.id);
    try {
      const r = await api.testDestination(d.id);
      setTestResult((prev) => ({ ...prev, [d.id]: r }));
    } catch (err) {
      setTestResult((prev) => ({
        ...prev,
        [d.id]: { ok: false, error: err instanceof Error ? err.message : String(err), latency_ms: 0 },
      }));
    } finally {
      setTesting(null);
    }
  }

  async function remove(d: BackupDestinationView) {
    if (!window.confirm(`Delete backup destination "${d.name}"? Policies using it must be updated.`)) {
      return;
    }
    setRowBusy(d.id);
    try {
      await api.deleteBackupDestination(d.id);
      if (editingId === d.id) resetForm();
      await dests.refresh();
    } catch (err) {
      window.alert(err instanceof Error ? err.message : String(err));
    } finally {
      setRowBusy(null);
    }
  }

  const list = dests.data ?? [];

  return (
    <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
      {/* Form */}
      <div className="stagger space-y-4 lg:col-span-1">
        <Panel
          title={editingId ? "Edit Destination" : "Add Destination"}
          subtitle="Where backups + archives are shipped"
        >
          {!canManage && (
            <div className="mb-3">
              <ErrorNote>Manager role required to create or edit destinations.</ErrorNote>
            </div>
          )}
          <form onSubmit={submit} className="space-y-4">
            <Field label={<>Name <span className="text-accent">*</span></>} htmlFor="d-name">
              <Input
                id="d-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="Off-site NAS"
                disabled={!canManage}
              />
            </Field>
            <div className="grid grid-cols-2 gap-3">
              <Field label="Kind" htmlFor="d-kind">
                <Select
                  id="d-kind"
                  value={kind}
                  onChange={(e) => setKind(e.target.value as BackupKind)}
                  disabled={!canManage}
                >
                  {KINDS.map((k) => (
                    <option key={k} value={k}>
                      {k.toUpperCase()}
                    </option>
                  ))}
                </Select>
              </Field>
              <Field label="Enabled" htmlFor="d-enabled">
                <label className="flex h-[38px] items-center gap-2 rounded-md border border-line bg-canvas px-3">
                  <input
                    id="d-enabled"
                    type="checkbox"
                    className="h-4 w-4 accent-accent"
                    checked={enabled}
                    onChange={(e) => setEnabled(e.target.checked)}
                    disabled={!canManage}
                  />
                  <span className="font-mono text-xs text-fg-secondary">{enabled ? "On" : "Off"}</span>
                </label>
              </Field>
            </div>

            {KIND_FIELDS[kind].map((f) => (
              <Field
                key={f.key}
                label={f.label}
                htmlFor={`d-cfg-${f.key}`}
                hint={f.secret && editingId ? "Leave as *** to keep the stored secret." : undefined}
              >
                <Input
                  id={`d-cfg-${f.key}`}
                  type={f.secret ? "password" : f.type === "number" ? "number" : "text"}
                  value={cfg[f.key] ?? ""}
                  onChange={(e) => setCfg((prev) => ({ ...prev, [f.key]: e.target.value }))}
                  placeholder={f.placeholder}
                  autoComplete="off"
                  disabled={!canManage}
                />
              </Field>
            ))}

            {formError && <ErrorNote>{formError}</ErrorNote>}
            <div className="flex justify-end gap-2">
              {editingId && (
                <Button type="button" variant="ghost" onClick={resetForm} disabled={submitting}>
                  Cancel
                </Button>
              )}
              <Button type="submit" variant="primary" disabled={submitting || !canManage}>
                {submitting ? (
                  <>
                    <Spinner size={14} />
                    Saving…
                  </>
                ) : editingId ? (
                  "Save changes"
                ) : (
                  "Add destination"
                )}
              </Button>
            </div>
          </form>
        </Panel>
      </div>

      {/* List */}
      <div className="stagger space-y-4 lg:col-span-2">
        <Panel
          title="Destinations"
          subtitle="Backup transports"
          padded={false}
          actions={
            list.length > 0 ? (
              <span className="font-mono text-[11px] tabular-nums text-fg-muted">{list.length}</span>
            ) : undefined
          }
        >
          {dests.error && !dests.data ? (
            <div className="p-4">
              <ErrorNote>Failed to load destinations: {dests.error}</ErrorNote>
            </div>
          ) : list.length === 0 ? (
            <div className="p-4">
              {dests.loading ? (
                <Loading label="destinations" />
              ) : (
                <EmptyState
                  title="No destinations"
                  hint="Add a local, SFTP, FTP, or S3 destination to ship backups and archive exports off-box."
                />
              )}
            </div>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full border-collapse">
                <thead>
                  <tr>
                    <Th>Name</Th>
                    <Th>Kind</Th>
                    <Th>Target</Th>
                    <Th>Creds</Th>
                    <Th>Status</Th>
                    <Th className="text-right">Action</Th>
                  </tr>
                </thead>
                <tbody>
                  {list.map((d) => {
                    const tr = testResult[d.id];
                    return (
                      <tr
                        key={d.id}
                        className="border-t border-line transition-colors duration-150 hover:bg-raised/40"
                      >
                        <Td>
                          <span className="block truncate text-sm font-medium text-fg">{d.name}</span>
                          <span className="block truncate font-mono text-[10px] text-fg-muted">
                            {d.id}
                          </span>
                        </Td>
                        <Td>
                          <Pill label={d.kind} color="#f59e0b" />
                        </Td>
                        <Td>
                          <span className="block max-w-[260px] truncate font-mono text-[11px] text-fg-secondary">
                            {destSummary(d)}
                          </span>
                          {tr && (
                            <span
                              className={cx(
                                "mt-1 block font-mono text-[10px]",
                                tr.ok ? "text-rec" : "text-danger",
                              )}
                            >
                              {tr.ok
                                ? `reachable · ${tr.latency_ms}ms`
                                : `failed — ${tr.error ?? "error"}`}
                            </span>
                          )}
                        </Td>
                        <Td>
                          {d.has_credentials ? (
                            <Pill label="set" color="#10b981" />
                          ) : (
                            <span className="font-mono text-[10px] text-fg-muted">none</span>
                          )}
                        </Td>
                        <Td>
                          <Pill
                            label={d.enabled ? "enabled" : "disabled"}
                            color={d.enabled ? "#10b981" : "#71717a"}
                          />
                        </Td>
                        <Td className="text-right">
                          <div className="flex justify-end gap-1.5">
                            <Button
                              size="sm"
                              disabled={!canManage || testing === d.id}
                              onClick={() => void runTest(d)}
                            >
                              {testing === d.id ? <Spinner size={13} /> : "Test"}
                            </Button>
                            <Button
                              size="sm"
                              disabled={!canManage}
                              onClick={() => startEdit(d)}
                            >
                              Edit
                            </Button>
                            <Button
                              size="sm"
                              variant="danger"
                              disabled={!canManage || rowBusy === d.id}
                              onClick={() => void remove(d)}
                            >
                              Delete
                            </Button>
                          </div>
                        </Td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
        </Panel>
      </div>
    </div>
  );
}

function destSummary(d: BackupDestinationView): string {
  const c = (d.config ?? {}) as Record<string, unknown>;
  const s = (k: string): string => {
    const v = c[k];
    return v == null ? "" : String(v);
  };
  switch (d.kind) {
    case "local":
      return s("path") || "—";
    case "sftp":
    case "ftp": {
      const user = s("user");
      const host = s("host");
      const port = s("port");
      const path = s("path");
      const head = `${user ? `${user}@` : ""}${host}${port ? `:${port}` : ""}`;
      return `${head}${path ? ` ${path}` : ""}`.trim() || "—";
    }
    case "s3": {
      const bucket = s("bucket");
      const prefix = s("prefix");
      const endpoint = s("endpoint");
      return `${bucket}${prefix ? `/${prefix}` : ""}${endpoint ? ` @ ${endpoint}` : ""}`.trim() || "—";
    }
    default:
      return "—";
  }
}

/* ====================================================================== */
/* Tab: Policies                                                           */
/* ====================================================================== */

function PoliciesTab({
  canManage,
  cameras,
}: {
  canManage: boolean;
  cameras: CameraView[];
}) {
  const policies = usePoll(() => api.listBackupPolicies(), 0);
  const dests = usePoll(() => api.listBackupDestinations(), 0);

  const destName = useMemo(() => {
    const m = new Map<string, string>();
    for (const d of dests.data ?? []) m.set(d.id, d.name);
    return m;
  }, [dests.data]);

  const [editingId, setEditingId] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [destinationId, setDestinationId] = useState("");
  const [cameraIds, setCameraIds] = useState<string[]>([]);
  const [incidentOnly, setIncidentOnly] = useState(false);
  const [intervalHours, setIntervalHours] = useState("24");
  const [lookbackHours, setLookbackHours] = useState("24");
  const [enabled, setEnabled] = useState(true);
  const [submitting, setSubmitting] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const [rowBusy, setRowBusy] = useState<string | null>(null);

  function resetForm() {
    setEditingId(null);
    setName("");
    setDestinationId("");
    setCameraIds([]);
    setIncidentOnly(false);
    setIntervalHours("24");
    setLookbackHours("24");
    setEnabled(true);
    setFormError(null);
  }

  function startEdit(p: BackupPolicy) {
    setEditingId(p.id);
    setName(p.name);
    setDestinationId(p.destination_id);
    setCameraIds(p.camera_ids ?? []);
    setIncidentOnly(p.incident_lock_only);
    setIntervalHours(String(Math.round((p.schedule_interval_s / 3600) * 100) / 100));
    setLookbackHours(String(p.lookback_hours));
    setEnabled(p.enabled);
    setFormError(null);
  }

  function toggleCamera(id: string) {
    setCameraIds((prev) => (prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id]));
  }

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!name.trim()) {
      setFormError("Name is required.");
      return;
    }
    if (!destinationId) {
      setFormError("Pick a destination.");
      return;
    }
    const intervalS = Math.max(60, Math.round(Number(intervalHours || "0") * 3600));
    const lookback = Math.max(0, Math.round(Number(lookbackHours || "0")));
    const body: BackupPolicyCreate = {
      name: name.trim(),
      destination_id: destinationId,
      camera_ids: cameraIds,
      incident_lock_only: incidentOnly,
      schedule_interval_s: intervalS,
      lookback_hours: lookback,
      enabled,
    };
    setSubmitting(true);
    setFormError(null);
    try {
      if (editingId) await api.updateBackupPolicy(editingId, body);
      else await api.createBackupPolicy(body);
      resetForm();
      await policies.refresh();
    } catch (err) {
      setFormError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  }

  async function trigger(p: BackupPolicy) {
    setRowBusy(p.id);
    try {
      await api.triggerPolicy(p.id);
      await policies.refresh();
    } catch (err) {
      window.alert(err instanceof Error ? err.message : String(err));
    } finally {
      setRowBusy(null);
    }
  }

  async function remove(p: BackupPolicy) {
    if (!window.confirm(`Delete backup policy "${p.name}"?`)) return;
    setRowBusy(p.id);
    try {
      await api.deleteBackupPolicy(p.id);
      if (editingId === p.id) resetForm();
      await policies.refresh();
    } catch (err) {
      window.alert(err instanceof Error ? err.message : String(err));
    } finally {
      setRowBusy(null);
    }
  }

  const list = policies.data ?? [];
  const destList = dests.data ?? [];

  return (
    <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
      {/* Form */}
      <div className="stagger space-y-4 lg:col-span-1">
        <Panel
          title={editingId ? "Edit Policy" : "Add Policy"}
          subtitle="Ship recent footage on a schedule"
        >
          {!canManage && (
            <div className="mb-3">
              <ErrorNote>Manager role required to create or edit policies.</ErrorNote>
            </div>
          )}
          <form onSubmit={submit} className="space-y-4">
            <Field label={<>Name <span className="text-accent">*</span></>} htmlFor="p-name">
              <Input
                id="p-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="Nightly off-site"
                disabled={!canManage}
              />
            </Field>
            <Field label={<>Destination <span className="text-accent">*</span></>} htmlFor="p-dest">
              <Select
                id="p-dest"
                value={destinationId}
                onChange={(e) => setDestinationId(e.target.value)}
                disabled={!canManage}
              >
                <option value="">Select a destination…</option>
                {destList.map((d) => (
                  <option key={d.id} value={d.id}>
                    {d.name} ({d.kind})
                  </option>
                ))}
              </Select>
            </Field>
            <Field label="Cameras" hint="No selection = all cameras.">
              <CameraMultiSelect
                cameras={cameras}
                selected={cameraIds}
                onToggle={toggleCamera}
                disabled={!canManage}
              />
            </Field>
            <div className="grid grid-cols-2 gap-3">
              <Field label="Interval (hours)" htmlFor="p-interval">
                <Input
                  id="p-interval"
                  type="number"
                  min={0.0167}
                  step={0.5}
                  value={intervalHours}
                  onChange={(e) => setIntervalHours(e.target.value)}
                  disabled={!canManage}
                />
              </Field>
              <Field label="Lookback (hours)" htmlFor="p-lookback" hint="0 = everything">
                <Input
                  id="p-lookback"
                  type="number"
                  min={0}
                  step={1}
                  value={lookbackHours}
                  onChange={(e) => setLookbackHours(e.target.value)}
                  disabled={!canManage}
                />
              </Field>
            </div>
            <div className="grid grid-cols-2 gap-3">
              <label className="flex items-center gap-2 rounded-md border border-line bg-canvas px-3 py-2">
                <input
                  type="checkbox"
                  className="h-4 w-4 accent-accent"
                  checked={incidentOnly}
                  onChange={(e) => setIncidentOnly(e.target.checked)}
                  disabled={!canManage}
                />
                <span className="font-mono text-[11px] text-fg-secondary">Incident-locked only</span>
              </label>
              <label className="flex items-center gap-2 rounded-md border border-line bg-canvas px-3 py-2">
                <input
                  type="checkbox"
                  className="h-4 w-4 accent-accent"
                  checked={enabled}
                  onChange={(e) => setEnabled(e.target.checked)}
                  disabled={!canManage}
                />
                <span className="font-mono text-[11px] text-fg-secondary">Enabled</span>
              </label>
            </div>
            {formError && <ErrorNote>{formError}</ErrorNote>}
            <div className="flex justify-end gap-2">
              {editingId && (
                <Button type="button" variant="ghost" onClick={resetForm} disabled={submitting}>
                  Cancel
                </Button>
              )}
              <Button type="submit" variant="primary" disabled={submitting || !canManage}>
                {submitting ? (
                  <>
                    <Spinner size={14} />
                    Saving…
                  </>
                ) : editingId ? (
                  "Save changes"
                ) : (
                  "Add policy"
                )}
              </Button>
            </div>
          </form>
        </Panel>
      </div>

      {/* List */}
      <div className="stagger space-y-4 lg:col-span-2">
        <Panel
          title="Policies"
          subtitle="Scheduled backups"
          padded={false}
          actions={
            list.length > 0 ? (
              <span className="font-mono text-[11px] tabular-nums text-fg-muted">{list.length}</span>
            ) : undefined
          }
        >
          {policies.error && !policies.data ? (
            <div className="p-4">
              <ErrorNote>Failed to load policies: {policies.error}</ErrorNote>
            </div>
          ) : list.length === 0 ? (
            <div className="p-4">
              {policies.loading ? (
                <Loading label="policies" />
              ) : (
                <EmptyState
                  title="No policies"
                  hint="Add a policy to ship a camera selection's recent footage to a destination on a schedule."
                />
              )}
            </div>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full border-collapse">
                <thead>
                  <tr>
                    <Th>Name</Th>
                    <Th>Destination</Th>
                    <Th>Cameras</Th>
                    <Th className="text-right">Interval</Th>
                    <Th className="text-right">Lookback</Th>
                    <Th>Last run</Th>
                    <Th>Status</Th>
                    <Th className="text-right">Action</Th>
                  </tr>
                </thead>
                <tbody>
                  {list.map((p) => (
                    <tr
                      key={p.id}
                      className="border-t border-line transition-colors duration-150 hover:bg-raised/40"
                    >
                      <Td>
                        <span className="block truncate text-sm font-medium text-fg">{p.name}</span>
                        {p.incident_lock_only && (
                          <span className="mt-0.5 inline-block">
                            <Pill label="incident-only" color="#f59e0b" />
                          </span>
                        )}
                      </Td>
                      <Td>
                        <span className="font-mono text-[11px] text-fg-secondary">
                          {destName.get(p.destination_id) ?? p.destination_id}
                        </span>
                      </Td>
                      <Td>
                        <span className="font-mono text-[11px] text-fg-secondary">
                          {camerasLabel(p.camera_ids)}
                        </span>
                      </Td>
                      <Td className="text-right">
                        <span className="whitespace-nowrap font-mono text-xs tabular-nums text-fg-secondary">
                          {(p.schedule_interval_s / 3600).toFixed(1)}h
                        </span>
                      </Td>
                      <Td className="text-right">
                        <span className="whitespace-nowrap font-mono text-xs tabular-nums text-fg-secondary">
                          {p.lookback_hours === 0 ? "all" : `${p.lookback_hours}h`}
                        </span>
                      </Td>
                      <Td>
                        <span className="whitespace-nowrap font-mono text-[11px] text-fg-muted">
                          {p.last_run_at ? timeAgo(p.last_run_at) : "never"}
                        </span>
                      </Td>
                      <Td>
                        <Pill
                          label={p.enabled ? "enabled" : "disabled"}
                          color={p.enabled ? "#10b981" : "#71717a"}
                        />
                      </Td>
                      <Td className="text-right">
                        <div className="flex justify-end gap-1.5">
                          <Button
                            size="sm"
                            variant="primary"
                            disabled={!canManage || rowBusy === p.id}
                            onClick={() => void trigger(p)}
                          >
                            {rowBusy === p.id ? <Spinner size={13} /> : "Trigger"}
                          </Button>
                          <Button size="sm" disabled={!canManage} onClick={() => startEdit(p)}>
                            Edit
                          </Button>
                          <Button
                            size="sm"
                            variant="danger"
                            disabled={!canManage || rowBusy === p.id}
                            onClick={() => void remove(p)}
                          >
                            Delete
                          </Button>
                        </div>
                      </Td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </Panel>
      </div>
    </div>
  );
}

/* ====================================================================== */
/* Tab: Jobs                                                               */
/* ====================================================================== */

function JobsTab({ canManage }: { canManage: boolean }) {
  const jobs = usePoll(() => api.listBackupJobs({ limit: 200 }), 5000);
  const [deletingId, setDeletingId] = useState<string | null>(null);

  async function remove(job: BackupJob) {
    if (!window.confirm("Delete this job record and its archive artifact (if any)?")) return;
    setDeletingId(job.id);
    try {
      await api.deleteBackupJob(job.id);
      await jobs.refresh();
    } catch (err) {
      window.alert(err instanceof Error ? err.message : String(err));
    } finally {
      setDeletingId(null);
    }
  }

  const list = jobs.data ?? [];
  const running = list.filter((j) => j.status === "running" || j.status === "pending").length;
  const errored = list.filter((j) => j.status === "error").length;
  const bytes = list.reduce((sum, j) => sum + (j.bytes_copied ?? 0), 0);

  return (
    <div className="stagger space-y-4">
      <div className="grid grid-cols-2 gap-px overflow-hidden rounded-panel border border-line bg-line sm:grid-cols-4">
        <div className="bg-panel px-4 py-3">
          <Stat label="Jobs" value={list.length} />
        </div>
        <div className="bg-panel px-4 py-3">
          <Stat label="Active" value={running} tone={running > 0 ? "warn" : "default"} />
        </div>
        <div className="bg-panel px-4 py-3">
          <Stat label="Errors" value={errored} tone={errored > 0 ? "bad" : "default"} />
        </div>
        <div className="bg-panel px-4 py-3">
          <Stat label="Copied" value={formatBytes(bytes)} />
        </div>
      </div>

      <Panel
        title="Backup Jobs"
        subtitle="All runs · refreshes every 5s"
        padded={false}
        actions={
          list.length > 0 ? (
            <span className="font-mono text-[11px] tabular-nums text-fg-muted">{list.length}</span>
          ) : undefined
        }
      >
        {jobs.error && !jobs.data ? (
          <div className="p-4">
            <ErrorNote>Failed to load jobs: {jobs.error}</ErrorNote>
          </div>
        ) : list.length === 0 ? (
          <div className="p-4">
            {jobs.loading ? (
              <Loading label="jobs" />
            ) : (
              <EmptyState
                title="No backup jobs"
                hint="Trigger a policy or run an archive export to populate the job ledger."
              />
            )}
          </div>
        ) : (
          <JobsTable
            jobs={list}
            canManage={canManage}
            deletingId={deletingId}
            onDelete={(j) => void remove(j)}
          />
        )}
      </Panel>
    </div>
  );
}

/* ====================================================================== */
/* Tab: Archive Export                                                     */
/* ====================================================================== */

function ArchiveTab({
  canManage,
  cameras,
}: {
  canManage: boolean;
  cameras: CameraView[];
}) {
  const exports = usePoll(() => api.listArchiveExports(100), 5000);

  const [cameraIds, setCameraIds] = useState<string[]>([]);
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [incidentOnly, setIncidentOnly] = useState(false);
  const [trim, setTrim] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const [deletingId, setDeletingId] = useState<string | null>(null);

  function toggleCamera(id: string) {
    setCameraIds((prev) => (prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id]));
  }

  async function submit(e: FormEvent) {
    e.preventDefault();
    const fromIso = localInputToIso(from);
    const toIso = localInputToIso(to);
    if (trim && (!fromIso || !toIso)) {
      setFormError("Trim requires both a from and to time.");
      return;
    }
    if (fromIso && toIso && new Date(toIso) <= new Date(fromIso)) {
      setFormError("To must be after from.");
      return;
    }
    const body: ArchiveExportRequest = { incident_lock_only: incidentOnly, trim };
    if (cameraIds.length > 0) body.camera_ids = cameraIds;
    if (fromIso) body.from = fromIso;
    if (toIso) body.to = toIso;
    setSubmitting(true);
    setFormError(null);
    try {
      await api.archiveExport(body);
      await exports.refresh();
    } catch (err) {
      setFormError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  }

  async function remove(job: BackupJob) {
    if (!window.confirm("Delete this archive export and its .zip artifact?")) return;
    setDeletingId(job.id);
    try {
      await api.deleteBackupJob(job.id);
      await exports.refresh();
    } catch (err) {
      window.alert(err instanceof Error ? err.message : String(err));
    } finally {
      setDeletingId(null);
    }
  }

  const list = exports.data ?? [];

  return (
    <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
      {/* Form */}
      <div className="stagger space-y-4 lg:col-span-1">
        <Panel title="Archive Export" subtitle="On-demand .zip of a footage selection">
          {!canManage && (
            <div className="mb-3">
              <ErrorNote>Manager role required to export archives.</ErrorNote>
            </div>
          )}
          <form onSubmit={submit} className="space-y-4">
            <Field label="Cameras" hint="No selection = all cameras.">
              <CameraMultiSelect
                cameras={cameras}
                selected={cameraIds}
                onToggle={toggleCamera}
                disabled={!canManage}
              />
            </Field>
            <Field label="From" htmlFor="a-from">
              <Input
                id="a-from"
                type="datetime-local"
                step={1}
                value={from}
                onChange={(e) => setFrom(e.target.value)}
                disabled={!canManage}
              />
            </Field>
            <Field label="To" htmlFor="a-to">
              <Input
                id="a-to"
                type="datetime-local"
                step={1}
                value={to}
                onChange={(e) => setTo(e.target.value)}
                disabled={!canManage}
              />
            </Field>
            <div className="space-y-2">
              <label className="flex items-center gap-2 rounded-md border border-line bg-canvas px-3 py-2">
                <input
                  type="checkbox"
                  className="h-4 w-4 accent-accent"
                  checked={incidentOnly}
                  onChange={(e) => setIncidentOnly(e.target.checked)}
                  disabled={!canManage}
                />
                <span className="font-mono text-[11px] text-fg-secondary">Incident-locked only</span>
              </label>
              <label className="flex items-center gap-2 rounded-md border border-line bg-canvas px-3 py-2">
                <input
                  type="checkbox"
                  className="h-4 w-4 accent-accent"
                  checked={trim}
                  onChange={(e) => setTrim(e.target.checked)}
                  disabled={!canManage}
                />
                <span className="font-mono text-[11px] text-fg-secondary">
                  Trim segments to window
                </span>
              </label>
            </div>
            {formError && <ErrorNote>{formError}</ErrorNote>}
            <div className="flex justify-end">
              <Button type="submit" variant="primary" disabled={submitting || !canManage}>
                {submitting ? (
                  <>
                    <Spinner size={14} />
                    Exporting…
                  </>
                ) : (
                  "Start export"
                )}
              </Button>
            </div>
          </form>
        </Panel>
      </div>

      {/* List */}
      <div className="stagger space-y-4 lg:col-span-2">
        <Panel
          title="Archive Exports"
          subtitle="Newest first · refreshes every 5s"
          padded={false}
          actions={
            list.length > 0 ? (
              <span className="font-mono text-[11px] tabular-nums text-fg-muted">{list.length}</span>
            ) : undefined
          }
        >
          {exports.error && !exports.data ? (
            <div className="p-4">
              <ErrorNote>Failed to load archive exports: {exports.error}</ErrorNote>
            </div>
          ) : list.length === 0 ? (
            <div className="p-4">
              {exports.loading ? (
                <Loading label="archive exports" />
              ) : (
                <EmptyState
                  title="No archive exports"
                  hint="Start an export on the left to package a footage selection as a downloadable .zip."
                />
              )}
            </div>
          ) : (
            <JobsTable
              jobs={list}
              canManage={canManage}
              deletingId={deletingId}
              onDelete={(j) => void remove(j)}
            />
          )}
        </Panel>
      </div>
    </div>
  );
}

/* ====================================================================== */
/* Page shell: tabs                                                        */
/* ====================================================================== */

type TabKey = "destinations" | "policies" | "jobs" | "archive";

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cx(
        "relative -mb-px whitespace-nowrap border-b-2 px-3.5 py-2.5 font-mono text-[11px] font-semibold uppercase tracking-micro transition-colors duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-canvas",
        active
          ? "border-accent text-fg"
          : "border-transparent text-fg-muted hover:text-fg-secondary",
      )}
    >
      {children}
    </button>
  );
}

export function Backup() {
  const [principal, setPrincipal] = useState<Principal | null>(null);
  const [tab, setTab] = useState<TabKey>("destinations");

  // Determine manager capability (mutations are manager+). When auth is disabled the server
  // returns the `system` admin principal; if not authenticated, mutations stay gated off and the
  // listings surface their own errors. We never block the read-only view behind a login screen.
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

  const cameraPoll = usePoll(() => api.listCameras(), 30000);
  const cameras = cameraPoll.data ?? [];

  const tabs: { key: TabKey; label: string }[] = [
    { key: "destinations", label: "Destinations" },
    { key: "policies", label: "Policies" },
    { key: "jobs", label: "Jobs" },
    { key: "archive", label: "Archive Export" },
  ];

  return (
    <div className="mx-auto max-w-[1600px] px-4 py-6 sm:px-6">
      <header className="animate-rise">
        <div className="flex flex-wrap items-end justify-between gap-4">
          <div className="min-w-0">
            <SectionLabel>Operations · Backup</SectionLabel>
            <h1 className="mt-1 font-display text-2xl font-extrabold tracking-tight text-fg">
              Backup &amp; Archive
            </h1>
          </div>
          {principal && (
            <div className="flex flex-col items-end leading-none">
              <span className="font-mono text-[12px] font-semibold text-fg">{principal.name}</span>
              <span className="mt-1 font-mono text-[9px] uppercase tracking-micro text-accent">
                {principal.role}
                {!canManage && <span className="text-fg-muted"> · read-only</span>}
              </span>
            </div>
          )}
        </div>

        <div className="mt-5 flex flex-wrap gap-1 overflow-x-auto border-b border-line">
          {tabs.map((t) => (
            <TabButton key={t.key} active={tab === t.key} onClick={() => setTab(t.key)}>
              {t.label}
            </TabButton>
          ))}
        </div>
      </header>

      <div className="mt-5">
        {tab === "destinations" && <DestinationsTab canManage={canManage} />}
        {tab === "policies" && <PoliciesTab canManage={canManage} cameras={cameras} />}
        {tab === "jobs" && <JobsTab canManage={canManage} />}
        {tab === "archive" && <ArchiveTab canManage={canManage} cameras={cameras} />}
      </div>
    </div>
  );
}

export default Backup;
