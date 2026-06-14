// Heldar Core — Stage 4 access-control operator console.
// One screen for the gatehouse: live entry events with confirm/reject workflow,
// visitor passes, the vehicle registry, the plate watchlist, daily reports, and
// (for admins) user + API-key administration. Auth-gated via /auth/me.

import { useCallback, useEffect, useMemo, useState } from "react";
import type { FormEvent, ReactNode } from "react";
import { api, ApiError, setAuthToken } from "../lib/api";
import { usePoll } from "../lib/usePoll";
import { Login } from "../components/Login";
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
  StatusPill,
  cx,
} from "../components/ui";
import { formatClock, localInputToIso, timeAgo } from "../lib/format";
import type {
  ApiKeyCreated,
  AuthStatus,
  EntryEvent,
  OwnerType,
  PassStatus,
  Principal,
  Role,
  Severity,
  UserCreate,
  VehicleCreate,
  VisitorPassCreate,
  WatchKind,
  WatchlistCreate,
  WorkflowStatus,
} from "../lib/types";

/* ====================================================================== */
/* Palettes — map domain enums onto the SOC signal colors.                */
/* ====================================================================== */

// auth_status -> camera-state palette consumed by StatusLed / StatusPill.
const AUTH_TO_STATE: Record<AuthStatus, string> = {
  matched: "recording", // green / ok
  exception: "connecting", // amber / warning
  blocked: "error", // red / critical
  unmatched: "offline", // neutral
};

const AUTH_COLOR: Record<AuthStatus, string> = {
  matched: "#10b981",
  exception: "#fbbf24",
  blocked: "#ef4444",
  unmatched: "#52525b",
};

const WORKFLOW_COLOR: Record<WorkflowStatus, string> = {
  pending: "#fbbf24",
  confirmed: "#10b981",
  rejected: "#ef4444",
  auto: "#71717a",
};

const PASS_COLOR: Record<PassStatus, string> = {
  active: "#fbbf24",
  checked_in: "#10b981",
  checked_out: "#71717a",
  expired: "#52525b",
  revoked: "#ef4444",
};

const WATCH_COLOR: Record<WatchKind, string> = {
  block: "#ef4444",
  vip: "#f59e0b",
  alert: "#fbbf24",
};

const SEVERITY_COLOR: Record<Severity, string> = {
  info: "#71717a",
  warning: "#fbbf24",
  critical: "#ef4444",
};

const ROLES: Role[] = ["admin", "manager", "guard", "viewer", "integration"];
const OWNER_TYPES: OwnerType[] = ["student", "staff", "resident", "contractor", "visitor"];

/* ====================================================================== */
/* Small shared bits.                                                     */
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

/** Inline status chip styled like the System.tsx severity badge. */
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

/** Safe string read from an untyped serde JSON object. */
function field(obj: Record<string, unknown> | null | undefined, key: string): string | null {
  if (!obj) return null;
  const v = obj[key];
  if (typeof v === "string") return v.trim() ? v : null;
  if (typeof v === "number" || typeof v === "boolean") return String(v);
  return null;
}

/* ====================================================================== */
/* Entry event card (shared by Live Entry + Reports).                     */
/* ====================================================================== */

function EvidenceThumb({ path, alt }: { path: string; alt: string }) {
  const [err, setErr] = useState(false);
  if (err) return null;
  return (
    <img
      src={path}
      alt={alt}
      loading="lazy"
      onError={() => setErr(true)}
      className="h-16 w-24 shrink-0 rounded-md border border-line bg-black object-cover"
    />
  );
}

function EntryEventCard({
  ev,
  canOperate,
  acting,
  onConfirm,
  onReject,
}: {
  ev: EntryEvent;
  canOperate?: boolean;
  acting?: boolean;
  onConfirm?: (ev: EntryEvent) => void;
  onReject?: (ev: EntryEvent) => void;
}) {
  const edge = AUTH_COLOR[ev.auth_status] ?? "#52525b";
  const snapshot = field(ev.evidence, "snapshot_path");
  const source = field(ev.authorization, "source");
  const vType = field(ev.subject, "vehicle_type");
  const color = field(ev.subject, "color");
  const pending = ev.workflow_status === "pending";
  const showActions = pending && !!canOperate && !!onConfirm && !!onReject;

  return (
    <div
      className="flex gap-3 rounded-md border border-line bg-panel2/40 p-3 transition-colors duration-150 hover:border-[#34373e]"
      style={{ borderLeftColor: edge, borderLeftWidth: 3 }}
    >
      {snapshot && <EvidenceThumb path={snapshot} alt={`Entry ${ev.plate ?? ev.id}`} />}
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <StatusPill state={AUTH_TO_STATE[ev.auth_status] ?? "unknown"} label={ev.auth_status} />
          <Pill label={ev.workflow_status} color={WORKFLOW_COLOR[ev.workflow_status] ?? "#71717a"} />
          <span className="ml-auto whitespace-nowrap font-mono text-[10px] text-fg-muted">
            {formatClock(ev.timestamp)}
          </span>
        </div>

        <div className="mt-2 flex flex-wrap items-baseline gap-x-2 gap-y-1">
          <span className="font-mono text-base font-semibold tracking-wide text-fg">
            {ev.plate ?? "—"}
          </span>
          {ev.plate_confidence != null && (
            <span className="font-mono text-[10px] text-fg-muted">
              {(ev.plate_confidence * 100).toFixed(0)}%
            </span>
          )}
          <span className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
            {ev.event_type}
          </span>
        </div>

        <div className="mt-1 flex flex-wrap gap-x-3 gap-y-0.5 font-mono text-[10px] text-fg-secondary">
          <span className="text-fg-muted">dir:&nbsp;<span className="text-fg-secondary">{ev.direction}</span></span>
          {vType && <span className="text-fg-muted">type:&nbsp;<span className="text-fg-secondary">{vType}</span></span>}
          {color && <span className="text-fg-muted">color:&nbsp;<span className="text-fg-secondary">{color}</span></span>}
          {source && <span className="text-fg-muted">src:&nbsp;<span className="text-fg-secondary">{source}</span></span>}
        </div>

        {showActions && (
          <div className="mt-2.5 flex items-center gap-2">
            <Button size="sm" variant="primary" disabled={acting} onClick={() => onConfirm!(ev)}>
              Confirm
            </Button>
            <Button size="sm" variant="danger" disabled={acting} onClick={() => onReject!(ev)}>
              Reject
            </Button>
            {acting && <Spinner size={13} />}
          </div>
        )}
      </div>
    </div>
  );
}

/* ====================================================================== */
/* Tab: Live Entry                                                        */
/* ====================================================================== */

function LiveEntryTab({ canOperate }: { canOperate: boolean }) {
  const events = usePoll(() => api.listEntryEvents({ limit: 50 }), 3000);
  const [actingId, setActingId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function act(ev: EntryEvent, kind: "confirm" | "reject") {
    const input = window.prompt(
      `${kind === "confirm" ? "Confirm" : "Reject"} entry ${ev.plate ?? ""} — optional note:`,
      "",
    );
    if (input === null) return; // cancelled
    const note = input.trim() ? input.trim() : undefined;
    setActingId(ev.id);
    setError(null);
    try {
      if (kind === "confirm") await api.confirmEntryEvent(ev.id, note);
      else await api.rejectEntryEvent(ev.id, note);
      await events.refresh();
    } catch (e) {
      setError(e instanceof ApiError ? e.message : String(e));
    } finally {
      setActingId(null);
    }
  }

  const list = events.data ?? [];
  const pending = list.filter((e) => e.workflow_status === "pending").length;

  return (
    <div className="stagger space-y-4">
      <div className="grid grid-cols-2 gap-px overflow-hidden rounded-panel border border-line bg-line sm:grid-cols-4">
        <div className="bg-panel px-4 py-3">
          <Stat label="Events" value={list.length} />
        </div>
        <div className="bg-panel px-4 py-3">
          <Stat label="Pending" value={pending} tone={pending > 0 ? "warn" : "default"} />
        </div>
        <div className="bg-panel px-4 py-3">
          <Stat
            label="Blocked"
            value={list.filter((e) => e.auth_status === "blocked").length}
            tone={list.some((e) => e.auth_status === "blocked") ? "bad" : "default"}
          />
        </div>
        <div className="bg-panel px-4 py-3">
          <Stat
            label="Matched"
            value={list.filter((e) => e.auth_status === "matched").length}
            tone="good"
          />
        </div>
      </div>

      <Panel
        title="Live Entry Feed"
        subtitle="Newest first · refreshes every 3s"
        actions={
          <span className="font-mono text-[11px] tabular-nums text-fg-muted">{list.length}</span>
        }
      >
        {error && <div className="mb-3"><ErrorNote>{error}</ErrorNote></div>}
        {events.error && !events.data ? (
          <ErrorNote>Failed to load entry events: {events.error}</ErrorNote>
        ) : list.length === 0 ? (
          events.loading ? (
            <Loading label="entry feed" />
          ) : (
            <EmptyState
              title="No entry events"
              hint="Entry and exit events from the gate cameras appear here as they are detected."
            />
          )
        ) : (
          <div className="space-y-2.5">
            {list.map((ev) => (
              <EntryEventCard
                key={ev.id}
                ev={ev}
                canOperate={canOperate}
                acting={actingId === ev.id}
                onConfirm={(e) => void act(e, "confirm")}
                onReject={(e) => void act(e, "reject")}
              />
            ))}
          </div>
        )}
      </Panel>
    </div>
  );
}

/* ====================================================================== */
/* Tab: Visitor Passes                                                    */
/* ====================================================================== */

function PassesTab() {
  const passes = usePoll(() => api.listPasses({ limit: 100 }), 8000);

  const [visitorName, setVisitorName] = useState("");
  const [phone, setPhone] = useState("");
  const [host, setHost] = useState("");
  const [purpose, setPurpose] = useState("");
  const [plate, setPlate] = useState("");
  const [validFrom, setValidFrom] = useState("");
  const [validUntil, setValidUntil] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const [rowActing, setRowActing] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!visitorName.trim()) {
      setFormError("Visitor name is required.");
      return;
    }
    const body: VisitorPassCreate = { visitor_name: visitorName.trim() };
    if (phone.trim()) body.phone = phone.trim();
    if (host.trim()) body.host = host.trim();
    if (purpose.trim()) body.purpose = purpose.trim();
    if (plate.trim()) body.plate = plate.trim();
    const vf = localInputToIso(validFrom);
    if (vf) body.valid_from = vf;
    const vu = localInputToIso(validUntil);
    if (vu) body.valid_until = vu;

    setSubmitting(true);
    setFormError(null);
    try {
      await api.createPass(body);
      setVisitorName("");
      setPhone("");
      setHost("");
      setPurpose("");
      setPlate("");
      setValidFrom("");
      setValidUntil("");
      await passes.refresh();
    } catch (err) {
      setFormError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  }

  async function gate(id: string, kind: "checkin" | "checkout") {
    setRowActing(id);
    try {
      if (kind === "checkin") await api.checkinPass(id);
      else await api.checkoutPass(id);
      await passes.refresh();
    } catch {
      /* surfaced on next poll; keep the gatehouse fast */
    } finally {
      setRowActing(null);
    }
  }

  const list = passes.data ?? [];

  return (
    <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
      <div className="stagger space-y-4 lg:col-span-1">
        <Panel title="Register Visitor" subtitle="Issue a new visitor pass">
          <form onSubmit={submit} className="space-y-4">
            <Field label={<>Visitor name <span className="text-accent">*</span></>} htmlFor="v-name">
              <Input id="v-name" value={visitorName} onChange={(e) => setVisitorName(e.target.value)} placeholder="Jane Doe" required />
            </Field>
            <div className="grid grid-cols-2 gap-3">
              <Field label="Phone" htmlFor="v-phone">
                <Input id="v-phone" value={phone} onChange={(e) => setPhone(e.target.value)} placeholder="+60…" />
              </Field>
              <Field label="Plate" htmlFor="v-plate">
                <Input id="v-plate" value={plate} onChange={(e) => setPlate(e.target.value)} placeholder="ABC1234" />
              </Field>
            </div>
            <Field label="Host" htmlFor="v-host">
              <Input id="v-host" value={host} onChange={(e) => setHost(e.target.value)} placeholder="Dept / staff name" />
            </Field>
            <Field label="Purpose" htmlFor="v-purpose">
              <Input id="v-purpose" value={purpose} onChange={(e) => setPurpose(e.target.value)} placeholder="Meeting, delivery…" />
            </Field>
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              <Field label="Valid from" htmlFor="v-from">
                <Input id="v-from" type="datetime-local" step={1} value={validFrom} onChange={(e) => setValidFrom(e.target.value)} />
              </Field>
              <Field label="Valid until" htmlFor="v-until">
                <Input id="v-until" type="datetime-local" step={1} value={validUntil} onChange={(e) => setValidUntil(e.target.value)} />
              </Field>
            </div>
            {formError && <ErrorNote>{formError}</ErrorNote>}
            <div className="flex justify-end">
              <Button type="submit" variant="primary" disabled={submitting}>
                {submitting ? (<><Spinner size={14} />Registering…</>) : "Register visitor"}
              </Button>
            </div>
          </form>
        </Panel>
      </div>

      <div className="stagger space-y-4 lg:col-span-2">
        <Panel
          title="Visitor Passes"
          subtitle="Active & recent passes"
          padded={false}
          actions={list.length > 0 ? <span className="font-mono text-[11px] tabular-nums text-fg-muted">{list.length}</span> : undefined}
        >
          {passes.error && !passes.data ? (
            <div className="p-4"><ErrorNote>Failed to load passes: {passes.error}</ErrorNote></div>
          ) : list.length === 0 ? (
            <div className="p-4">
              {passes.loading ? <Loading label="passes" /> : <EmptyState title="No visitor passes" hint="Register a visitor on the left to issue the first pass." />}
            </div>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full border-collapse">
                <thead>
                  <tr>
                    <Th>Visitor</Th>
                    <Th>Host</Th>
                    <Th>Plate</Th>
                    <Th>Status</Th>
                    <Th>Valid until</Th>
                    <Th className="text-right">Action</Th>
                  </tr>
                </thead>
                <tbody>
                  {list.map((p) => (
                    <tr key={p.id} className="border-t border-line transition-colors duration-150 hover:bg-raised/40">
                      <Td>
                        <span className="block truncate text-sm font-medium text-fg">{p.visitor_name}</span>
                        <span className="block truncate font-mono text-[10px] text-fg-muted">{p.code}</span>
                      </Td>
                      <Td><span className="font-mono text-xs text-fg-secondary">{p.host ?? "—"}</span></Td>
                      <Td><span className="font-mono text-xs text-fg">{p.plate ?? "—"}</span></Td>
                      <Td><Pill label={p.status.replace("_", " ")} color={PASS_COLOR[p.status] ?? "#71717a"} /></Td>
                      <Td><span className="whitespace-nowrap font-mono text-[11px] text-fg-secondary">{formatClock(p.valid_until)}</span></Td>
                      <Td className="text-right">
                        {p.status === "active" ? (
                          <Button size="sm" variant="primary" disabled={rowActing === p.id} onClick={() => void gate(p.id, "checkin")}>Check-in</Button>
                        ) : p.status === "checked_in" ? (
                          <Button size="sm" disabled={rowActing === p.id} onClick={() => void gate(p.id, "checkout")}>Check-out</Button>
                        ) : (
                          <span className="font-mono text-[10px] text-fg-muted">—</span>
                        )}
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
/* Tab: Vehicles                                                          */
/* ====================================================================== */

function VehiclesTab() {
  const vehicles = usePoll(() => api.listVehicles({ limit: 200 }), 15000);

  const [plate, setPlate] = useState("");
  const [ownerName, setOwnerName] = useState("");
  const [ownerType, setOwnerType] = useState<OwnerType>("staff");
  const [vehicleType, setVehicleType] = useState("");
  const [make, setMake] = useState("");
  const [model, setModel] = useState("");
  const [color, setColor] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!plate.trim()) {
      setFormError("Plate is required.");
      return;
    }
    const body: VehicleCreate = { plate: plate.trim(), owner_type: ownerType };
    if (ownerName.trim()) body.owner_name = ownerName.trim();
    if (vehicleType.trim()) body.vehicle_type = vehicleType.trim();
    if (make.trim()) body.make = make.trim();
    if (model.trim()) body.model = model.trim();
    if (color.trim()) body.color = color.trim();

    setSubmitting(true);
    setFormError(null);
    try {
      await api.createVehicle(body);
      setPlate("");
      setOwnerName("");
      setVehicleType("");
      setMake("");
      setModel("");
      setColor("");
      await vehicles.refresh();
    } catch (err) {
      setFormError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  }

  async function remove(id: string, label: string) {
    if (!window.confirm(`Delete vehicle ${label}? This cannot be undone.`)) return;
    setDeleting(id);
    try {
      await api.deleteVehicle(id);
      await vehicles.refresh();
    } catch {
      /* keep gatehouse responsive; reappears on next poll if it failed */
    } finally {
      setDeleting(null);
    }
  }

  const list = vehicles.data ?? [];

  return (
    <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
      <div className="stagger space-y-4 lg:col-span-1">
        <Panel title="Register Vehicle" subtitle="Add to the authorized registry">
          <form onSubmit={submit} className="space-y-4">
            <Field label={<>Plate <span className="text-accent">*</span></>} htmlFor="ve-plate">
              <Input id="ve-plate" value={plate} onChange={(e) => setPlate(e.target.value)} placeholder="ABC1234" required />
            </Field>
            <Field label="Owner name" htmlFor="ve-owner">
              <Input id="ve-owner" value={ownerName} onChange={(e) => setOwnerName(e.target.value)} placeholder="John Smith" />
            </Field>
            <Field label="Owner type" htmlFor="ve-otype">
              <Select id="ve-otype" value={ownerType} onChange={(e) => setOwnerType(e.target.value as OwnerType)}>
                {OWNER_TYPES.map((t) => (
                  <option key={t} value={t}>{t[0].toUpperCase() + t.slice(1)}</option>
                ))}
              </Select>
            </Field>
            <div className="grid grid-cols-2 gap-3">
              <Field label="Vehicle type" htmlFor="ve-vtype">
                <Input id="ve-vtype" value={vehicleType} onChange={(e) => setVehicleType(e.target.value)} placeholder="car / van" />
              </Field>
              <Field label="Color" htmlFor="ve-color">
                <Input id="ve-color" value={color} onChange={(e) => setColor(e.target.value)} placeholder="silver" />
              </Field>
              <Field label="Make" htmlFor="ve-make">
                <Input id="ve-make" value={make} onChange={(e) => setMake(e.target.value)} placeholder="Toyota" />
              </Field>
              <Field label="Model" htmlFor="ve-model">
                <Input id="ve-model" value={model} onChange={(e) => setModel(e.target.value)} placeholder="Hilux" />
              </Field>
            </div>
            {formError && <ErrorNote>{formError}</ErrorNote>}
            <div className="flex justify-end">
              <Button type="submit" variant="primary" disabled={submitting}>
                {submitting ? (<><Spinner size={14} />Adding…</>) : "Add vehicle"}
              </Button>
            </div>
          </form>
        </Panel>
      </div>

      <div className="stagger space-y-4 lg:col-span-2">
        <Panel
          title="Vehicle Registry"
          subtitle="Authorized vehicles"
          padded={false}
          actions={list.length > 0 ? <span className="font-mono text-[11px] tabular-nums text-fg-muted">{list.length}</span> : undefined}
        >
          {vehicles.error && !vehicles.data ? (
            <div className="p-4"><ErrorNote>Failed to load vehicles: {vehicles.error}</ErrorNote></div>
          ) : list.length === 0 ? (
            <div className="p-4">
              {vehicles.loading ? <Loading label="vehicles" /> : <EmptyState title="No vehicles registered" hint="Add an authorized vehicle on the left to populate the registry." />}
            </div>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full border-collapse">
                <thead>
                  <tr>
                    <Th>Plate</Th>
                    <Th>Owner</Th>
                    <Th>Type</Th>
                    <Th>Vehicle</Th>
                    <Th className="text-right">Action</Th>
                  </tr>
                </thead>
                <tbody>
                  {list.map((v) => (
                    <tr key={v.id} className="border-t border-line transition-colors duration-150 hover:bg-raised/40">
                      <Td><span className="font-mono text-sm font-semibold tracking-wide text-fg">{v.plate}</span></Td>
                      <Td>
                        <span className="block truncate text-xs text-fg-secondary">{v.owner_name ?? "—"}</span>
                        <span className="mt-0.5 inline-block"><Pill label={v.owner_type} color="#71717a" /></span>
                      </Td>
                      <Td><span className="font-mono text-xs text-fg-secondary">{v.vehicle_type ?? "—"}</span></Td>
                      <Td>
                        <span className="font-mono text-[11px] text-fg-secondary">
                          {[v.make, v.model].filter(Boolean).join(" ") || "—"}
                          {v.color ? <span className="text-fg-muted"> · {v.color}</span> : null}
                        </span>
                      </Td>
                      <Td className="text-right">
                        <Button size="sm" variant="danger" disabled={deleting === v.id} onClick={() => void remove(v.id, v.plate)}>Delete</Button>
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
/* Tab: Watchlist                                                         */
/* ====================================================================== */

function WatchlistTab() {
  const watch = usePoll(() => api.listWatchlist(), 15000);

  const [plate, setPlate] = useState("");
  const [kind, setKind] = useState<WatchKind>("block");
  const [reason, setReason] = useState("");
  const [severity, setSeverity] = useState<Severity>("warning");
  const [submitting, setSubmitting] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!plate.trim()) {
      setFormError("Plate is required.");
      return;
    }
    const body: WatchlistCreate = { plate: plate.trim(), kind, severity };
    if (reason.trim()) body.reason = reason.trim();
    setSubmitting(true);
    setFormError(null);
    try {
      await api.createWatch(body);
      setPlate("");
      setReason("");
      await watch.refresh();
    } catch (err) {
      setFormError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  }

  async function remove(id: string, label: string) {
    if (!window.confirm(`Remove ${label} from the watchlist?`)) return;
    setDeleting(id);
    try {
      await api.deleteWatch(id);
      await watch.refresh();
    } catch {
      /* reappears on next poll if it failed */
    } finally {
      setDeleting(null);
    }
  }

  const list = watch.data ?? [];

  return (
    <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
      <div className="stagger space-y-4 lg:col-span-1">
        <Panel title="Add to Watchlist" subtitle="Flag a plate for the gate">
          <form onSubmit={submit} className="space-y-4">
            <Field label={<>Plate <span className="text-accent">*</span></>} htmlFor="w-plate">
              <Input id="w-plate" value={plate} onChange={(e) => setPlate(e.target.value)} placeholder="ABC1234" required />
            </Field>
            <div className="grid grid-cols-2 gap-3">
              <Field label="Kind" htmlFor="w-kind">
                <Select id="w-kind" value={kind} onChange={(e) => setKind(e.target.value as WatchKind)}>
                  <option value="block">Block</option>
                  <option value="vip">VIP</option>
                  <option value="alert">Alert</option>
                </Select>
              </Field>
              <Field label="Severity" htmlFor="w-sev">
                <Select id="w-sev" value={severity} onChange={(e) => setSeverity(e.target.value as Severity)}>
                  <option value="info">Info</option>
                  <option value="warning">Warning</option>
                  <option value="critical">Critical</option>
                </Select>
              </Field>
            </div>
            <Field label="Reason" htmlFor="w-reason">
              <Input id="w-reason" value={reason} onChange={(e) => setReason(e.target.value)} placeholder="Unpaid fines, stolen, …" />
            </Field>
            {formError && <ErrorNote>{formError}</ErrorNote>}
            <div className="flex justify-end">
              <Button type="submit" variant="primary" disabled={submitting}>
                {submitting ? (<><Spinner size={14} />Adding…</>) : "Add to watchlist"}
              </Button>
            </div>
          </form>
        </Panel>
      </div>

      <div className="stagger space-y-4 lg:col-span-2">
        <Panel
          title="Watchlist"
          subtitle="Flagged plates"
          padded={false}
          actions={list.length > 0 ? <span className="font-mono text-[11px] tabular-nums text-fg-muted">{list.length}</span> : undefined}
        >
          {watch.error && !watch.data ? (
            <div className="p-4"><ErrorNote>Failed to load watchlist: {watch.error}</ErrorNote></div>
          ) : list.length === 0 ? (
            <div className="p-4">
              {watch.loading ? <Loading label="watchlist" /> : <EmptyState title="Watchlist empty" hint="Flag a plate on the left to block, VIP, or alert on it at the gate." />}
            </div>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full border-collapse">
                <thead>
                  <tr>
                    <Th>Plate</Th>
                    <Th>Kind</Th>
                    <Th>Severity</Th>
                    <Th>Reason</Th>
                    <Th className="text-right">Action</Th>
                  </tr>
                </thead>
                <tbody>
                  {list.map((w) => (
                    <tr key={w.id} className="border-t border-line transition-colors duration-150 hover:bg-raised/40">
                      <Td><span className="font-mono text-sm font-semibold tracking-wide text-fg">{w.plate}</span></Td>
                      <Td><Pill label={w.kind} color={WATCH_COLOR[w.kind] ?? "#71717a"} /></Td>
                      <Td><Pill label={w.severity} color={SEVERITY_COLOR[w.severity] ?? "#71717a"} /></Td>
                      <Td><span className="font-mono text-[11px] text-fg-secondary">{w.reason ?? "—"}</span></Td>
                      <Td className="text-right">
                        <Button size="sm" variant="danger" disabled={deleting === w.id} onClick={() => void remove(w.id, w.plate)}>Delete</Button>
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
/* Tab: Reports                                                           */
/* ====================================================================== */

function todayStr(): string {
  const d = new Date();
  const local = new Date(d.getTime() - d.getTimezoneOffset() * 60000);
  return local.toISOString().slice(0, 10);
}

const AUTH_ORDER: AuthStatus[] = ["matched", "exception", "blocked", "unmatched"];
const AUTH_TONE: Record<AuthStatus, "good" | "warn" | "bad" | "default"> = {
  matched: "good",
  exception: "warn",
  blocked: "bad",
  unmatched: "default",
};

function ReportsTab() {
  const [date, setDate] = useState<string>(() => todayStr());
  const log = usePoll(() => api.reportEntryLog({ date }), 0, [date]);
  const exceptions = usePoll(() => api.reportExceptions({ date }), 0, [date]);

  const tally = log.data?.by_auth_status ?? {};
  const events = log.data?.events ?? [];

  return (
    <div className="stagger space-y-4">
      <Panel title="Daily Report" subtitle="Entry log & exceptions for a single day">
        <div className="flex flex-wrap items-end gap-3">
          <div className="w-48">
            <Field label="Date" htmlFor="r-date">
              <Input id="r-date" type="date" value={date} max={todayStr()} onChange={(e) => setDate(e.target.value)} />
            </Field>
          </div>
          <Button onClick={() => { void log.refresh(); void exceptions.refresh(); }} disabled={log.loading}>
            {log.loading ? <Spinner size={14} /> : "Reload"}
          </Button>
        </div>

        {log.error ? (
          <div className="mt-4"><ErrorNote>Failed to load report: {log.error}</ErrorNote></div>
        ) : (
          <div className="mt-4 grid grid-cols-2 gap-px overflow-hidden rounded-panel border border-line bg-line sm:grid-cols-3 lg:grid-cols-6">
            <div className="bg-panel px-4 py-3">
              <Stat label="Total" value={log.data?.total ?? 0} />
            </div>
            {AUTH_ORDER.map((s) => (
              <div key={s} className="bg-panel px-4 py-3">
                <Stat label={s} value={tally[s] ?? 0} tone={AUTH_TONE[s]} />
              </div>
            ))}
            <div className="bg-panel px-4 py-3">
              <Stat
                label="Exceptions"
                value={exceptions.data?.total ?? 0}
                tone={(exceptions.data?.total ?? 0) > 0 ? "warn" : "default"}
              />
            </div>
          </div>
        )}
      </Panel>

      <Panel
        title="Report Events"
        subtitle={log.data ? `${formatClock(log.data.from)} → ${formatClock(log.data.to)}` : "—"}
        actions={events.length > 0 ? <span className="font-mono text-[11px] tabular-nums text-fg-muted">{events.length}</span> : undefined}
      >
        {log.loading && !log.data ? (
          <Loading label="report" />
        ) : events.length === 0 ? (
          <EmptyState title="No events for this day" hint="Pick another date, or wait for gate activity to be recorded." />
        ) : (
          <div className="space-y-2.5">
            {events.map((ev) => (
              <EntryEventCard key={ev.id} ev={ev} />
            ))}
          </div>
        )}
      </Panel>
    </div>
  );
}

/* ====================================================================== */
/* Tab: Admin                                                             */
/* ====================================================================== */

function AdminTab() {
  const users = usePoll(() => api.listUsers(), 0);
  const keys = usePoll(() => api.listApiKeys(), 0);

  // ---- user create ----
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [userRole, setUserRole] = useState<Role>("guard");
  const [userSubmitting, setUserSubmitting] = useState(false);
  const [userError, setUserError] = useState<string | null>(null);
  const [userActing, setUserActing] = useState<string | null>(null);

  async function createUser(e: FormEvent) {
    e.preventDefault();
    if (!username.trim() || !password) {
      setUserError("Username and password are required.");
      return;
    }
    const body: UserCreate = { username: username.trim(), password, role: userRole };
    if (displayName.trim()) body.display_name = displayName.trim();
    setUserSubmitting(true);
    setUserError(null);
    try {
      await api.createUser(body);
      setUsername("");
      setPassword("");
      setDisplayName("");
      await users.refresh();
    } catch (err) {
      setUserError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setUserSubmitting(false);
    }
  }

  async function toggleUser(id: string, active: boolean) {
    setUserActing(id);
    try {
      await api.updateUser(id, { active: !active });
      await users.refresh();
    } catch {
      /* reflected on next refresh */
    } finally {
      setUserActing(null);
    }
  }

  // ---- api keys ----
  const [keyName, setKeyName] = useState("");
  const [keyRole, setKeyRole] = useState<Role>("integration");
  const [keySubmitting, setKeySubmitting] = useState(false);
  const [keyError, setKeyError] = useState<string | null>(null);
  const [createdKey, setCreatedKey] = useState<ApiKeyCreated | null>(null);
  const [copied, setCopied] = useState(false);
  const [keyDeleting, setKeyDeleting] = useState<string | null>(null);

  async function createKey(e: FormEvent) {
    e.preventDefault();
    if (!keyName.trim()) {
      setKeyError("Key name is required.");
      return;
    }
    setKeySubmitting(true);
    setKeyError(null);
    try {
      const created = await api.createApiKey(keyName.trim(), keyRole);
      setCreatedKey(created);
      setCopied(false);
      setKeyName("");
      await keys.refresh();
    } catch (err) {
      setKeyError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setKeySubmitting(false);
    }
  }

  async function copyKey() {
    if (!createdKey) return;
    try {
      await navigator.clipboard.writeText(createdKey.key);
      setCopied(true);
    } catch {
      /* clipboard blocked; the operator can still select the text manually */
    }
  }

  async function deleteKey(id: string, name: string) {
    if (!window.confirm(`Revoke API key “${name}”? Integrations using it will stop working.`)) return;
    setKeyDeleting(id);
    try {
      await api.deleteApiKey(id);
      await keys.refresh();
    } catch {
      /* reflected on next refresh */
    } finally {
      setKeyDeleting(null);
    }
  }

  const userList = users.data ?? [];
  const keyList = keys.data ?? [];

  return (
    <div className="stagger space-y-4">
      {/* Users */}
      <Panel title="Users" subtitle="Operator accounts & roles">
        <form onSubmit={createUser} className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-5 lg:items-end">
          <Field label="Username" htmlFor="u-name">
            <Input id="u-name" value={username} onChange={(e) => setUsername(e.target.value)} autoComplete="off" placeholder="guard01" />
          </Field>
          <Field label="Password" htmlFor="u-pass">
            <Input id="u-pass" type="password" value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="new-password" placeholder="••••••••" />
          </Field>
          <Field label="Display name" htmlFor="u-disp">
            <Input id="u-disp" value={displayName} onChange={(e) => setDisplayName(e.target.value)} placeholder="Optional" />
          </Field>
          <Field label="Role" htmlFor="u-role">
            <Select id="u-role" value={userRole} onChange={(e) => setUserRole(e.target.value as Role)}>
              {ROLES.map((r) => (
                <option key={r} value={r}>{r[0].toUpperCase() + r.slice(1)}</option>
              ))}
            </Select>
          </Field>
          <Button type="submit" variant="primary" disabled={userSubmitting}>
            {userSubmitting ? (<><Spinner size={14} />Creating…</>) : "Create user"}
          </Button>
        </form>
        {userError && <div className="mt-3"><ErrorNote>{userError}</ErrorNote></div>}

        <div className="mt-4 overflow-x-auto rounded-md border border-line">
          {users.error && !users.data ? (
            <div className="p-4"><ErrorNote>Failed to load users: {users.error}</ErrorNote></div>
          ) : userList.length === 0 ? (
            <div className="p-4">{users.loading ? <Loading label="users" /> : <p className="font-mono text-xs text-fg-muted">No users.</p>}</div>
          ) : (
            <table className="w-full border-collapse">
              <thead>
                <tr>
                  <Th>User</Th>
                  <Th>Role</Th>
                  <Th>Status</Th>
                  <Th>Created</Th>
                  <Th className="text-right">Action</Th>
                </tr>
              </thead>
              <tbody>
                {userList.map((u) => (
                  <tr key={u.id} className="border-t border-line transition-colors duration-150 hover:bg-raised/40">
                    <Td>
                      <span className="block truncate text-sm font-medium text-fg">{u.display_name || u.username}</span>
                      <span className="block truncate font-mono text-[10px] text-fg-muted">{u.username}</span>
                    </Td>
                    <Td><Pill label={u.role} color="#f59e0b" /></Td>
                    <Td><Pill label={u.active ? "active" : "disabled"} color={u.active ? "#10b981" : "#71717a"} /></Td>
                    <Td><span className="whitespace-nowrap font-mono text-[11px] text-fg-secondary">{timeAgo(u.created_at)}</span></Td>
                    <Td className="text-right">
                      <Button size="sm" disabled={userActing === u.id} onClick={() => void toggleUser(u.id, u.active)}>
                        {u.active ? "Disable" : "Enable"}
                      </Button>
                    </Td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      </Panel>

      {/* API keys */}
      <Panel title="API Keys" subtitle="Machine credentials for integrations">
        <form onSubmit={createKey} className="grid grid-cols-1 gap-3 sm:grid-cols-3 lg:items-end">
          <Field label="Name" htmlFor="k-name">
            <Input id="k-name" value={keyName} onChange={(e) => setKeyName(e.target.value)} placeholder="anpr-worker" />
          </Field>
          <Field label="Role" htmlFor="k-role">
            <Select id="k-role" value={keyRole} onChange={(e) => setKeyRole(e.target.value as Role)}>
              {ROLES.map((r) => (
                <option key={r} value={r}>{r[0].toUpperCase() + r.slice(1)}</option>
              ))}
            </Select>
          </Field>
          <Button type="submit" variant="primary" disabled={keySubmitting}>
            {keySubmitting ? (<><Spinner size={14} />Creating…</>) : "Create key"}
          </Button>
        </form>
        {keyError && <div className="mt-3"><ErrorNote>{keyError}</ErrorNote></div>}

        {createdKey && (
          <div className="mt-3 rounded-md border border-accent/40 bg-accent/[0.07] p-3">
            <div className="flex items-center gap-2 font-mono text-[10px] font-semibold uppercase tracking-micro text-accent">
              <WarnIcon /> New key — copy it now
            </div>
            <p className="mt-1 text-xs leading-relaxed text-fg-secondary">
              The key for <span className="text-fg">{createdKey.name}</span> ({createdKey.role}) is shown
              only once and cannot be retrieved again.
            </p>
            <div className="mt-2 flex items-center gap-2">
              <code className="min-w-0 flex-1 truncate rounded border border-line bg-canvas px-2 py-1.5 font-mono text-xs text-accent-soft">
                {createdKey.key}
              </code>
              <Button size="sm" onClick={() => void copyKey()}>{copied ? "Copied" : "Copy"}</Button>
              <Button size="sm" variant="ghost" onClick={() => setCreatedKey(null)}>Dismiss</Button>
            </div>
          </div>
        )}

        <div className="mt-4 overflow-x-auto rounded-md border border-line">
          {keys.error && !keys.data ? (
            <div className="p-4"><ErrorNote>Failed to load API keys: {keys.error}</ErrorNote></div>
          ) : keyList.length === 0 ? (
            <div className="p-4">{keys.loading ? <Loading label="API keys" /> : <p className="font-mono text-xs text-fg-muted">No API keys.</p>}</div>
          ) : (
            <table className="w-full border-collapse">
              <thead>
                <tr>
                  <Th>Name</Th>
                  <Th>Prefix</Th>
                  <Th>Role</Th>
                  <Th>Last used</Th>
                  <Th className="text-right">Action</Th>
                </tr>
              </thead>
              <tbody>
                {keyList.map((k) => (
                  <tr key={k.id} className="border-t border-line transition-colors duration-150 hover:bg-raised/40">
                    <Td><span className="text-sm font-medium text-fg">{k.name}</span></Td>
                    <Td><span className="font-mono text-xs text-fg-secondary">{k.key_prefix}…</span></Td>
                    <Td><Pill label={k.role} color="#f59e0b" /></Td>
                    <Td><span className="whitespace-nowrap font-mono text-[11px] text-fg-secondary">{k.last_used_at ? timeAgo(k.last_used_at) : "never"}</span></Td>
                    <Td className="text-right">
                      <Button size="sm" variant="danger" disabled={keyDeleting === k.id} onClick={() => void deleteKey(k.id, k.name)}>Revoke</Button>
                    </Td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      </Panel>
    </div>
  );
}

/* ====================================================================== */
/* Page shell: auth gate + tabs.                                          */
/* ====================================================================== */

type TabKey = "live" | "passes" | "vehicles" | "watchlist" | "reports" | "admin";

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

export function Entry() {
  const [principal, setPrincipal] = useState<Principal | null>(null);
  const [authLoading, setAuthLoading] = useState(true);
  const [needsLogin, setNeedsLogin] = useState(false);
  const [authError, setAuthError] = useState<string | null>(null);
  const [tab, setTab] = useState<TabKey>("live");

  const loadMe = useCallback(async () => {
    setAuthLoading(true);
    setAuthError(null);
    try {
      const p = await api.me();
      setPrincipal(p);
      setNeedsLogin(false);
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) {
        setPrincipal(null);
        setNeedsLogin(true);
      } else {
        setAuthError(e instanceof Error ? e.message : String(e));
      }
    } finally {
      setAuthLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadMe();
  }, [loadMe]);

  const isAdmin = principal?.role === "admin";
  const canOperate =
    principal?.role === "admin" || principal?.role === "manager" || principal?.role === "guard";

  const tabs = useMemo(() => {
    const base: { key: TabKey; label: string }[] = [
      { key: "live", label: "Live Entry" },
      { key: "passes", label: "Visitor Passes" },
      { key: "vehicles", label: "Vehicles" },
      { key: "watchlist", label: "Watchlist" },
      { key: "reports", label: "Reports" },
    ];
    if (isAdmin) base.push({ key: "admin", label: "Admin" });
    return base;
  }, [isAdmin]);

  async function signOut() {
    try {
      await api.logout();
    } catch {
      /* token may already be invalid — clear locally regardless */
    }
    setAuthToken(null);
    setPrincipal(null);
    setNeedsLogin(true);
  }

  // ---- Gate states ----
  if (needsLogin) {
    return (
      <Login
        onSuccess={(p) => {
          setPrincipal(p);
          setNeedsLogin(false);
          setAuthError(null);
        }}
      />
    );
  }

  if (authLoading && !principal) {
    return (
      <div className="flex min-h-[60vh] items-center justify-center gap-3 text-fg-secondary">
        <Spinner />
        <span className="font-mono text-xs uppercase tracking-micro">Authenticating…</span>
      </div>
    );
  }

  if (authError && !principal) {
    return (
      <div className="mx-auto max-w-md px-4 py-20">
        <Panel title="Console unavailable">
          <ErrorNote>{authError}</ErrorNote>
          <div className="mt-3 flex justify-end">
            <Button variant="primary" onClick={() => void loadMe()}>Retry</Button>
          </div>
        </Panel>
      </div>
    );
  }

  if (!principal) return null;

  // Guard: never render Admin for a non-admin (e.g. after sign-out/role change).
  const activeTab: TabKey = tab === "admin" && !isAdmin ? "live" : tab;

  return (
    <div className="mx-auto max-w-[1600px] px-4 py-6 sm:px-6">
      <header className="animate-rise">
        <div className="flex flex-wrap items-end justify-between gap-4">
          <div className="min-w-0">
            <SectionLabel>Operations · Entry</SectionLabel>
            <h1 className="mt-1 font-display text-2xl font-extrabold tracking-tight text-fg">
              Access Control
            </h1>
          </div>
          <div className="flex items-center gap-3">
            <div className="flex flex-col items-end leading-none">
              <span className="font-mono text-[12px] font-semibold text-fg">{principal.name}</span>
              <span className="mt-1 font-mono text-[9px] uppercase tracking-micro text-accent">
                {principal.role}
                {principal.kind === "system" && <span className="text-fg-muted"> · auth off</span>}
              </span>
            </div>
            {principal.kind === "user" && (
              <Button size="sm" onClick={() => void signOut()}>Sign out</Button>
            )}
          </div>
        </div>

        {/* Tab bar */}
        <div className="mt-5 flex flex-wrap gap-1 overflow-x-auto border-b border-line">
          {tabs.map((t) => (
            <TabButton key={t.key} active={activeTab === t.key} onClick={() => setTab(t.key)}>
              {t.label}
            </TabButton>
          ))}
        </div>
      </header>

      <div className="mt-5">
        {activeTab === "live" && <LiveEntryTab canOperate={canOperate} />}
        {activeTab === "passes" && <PassesTab />}
        {activeTab === "vehicles" && <VehiclesTab />}
        {activeTab === "watchlist" && <WatchlistTab />}
        {activeTab === "reports" && <ReportsTab />}
        {activeTab === "admin" && isAdmin && <AdminTab />}
      </div>
    </div>
  );
}

export default Entry;
