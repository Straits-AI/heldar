// Heldar Core — Stage 6 "Movement intelligence" console.
// Cross-camera movement reasoning: red-zone breach incidents, probabilistic ReID candidates
// (anchored on plates, awaiting a human decision), the camera-link topology that bounds plausible
// transits, and an audited plate trail search. NOTHING here asserts identity — every correlation is
// a candidate the operator confirms or rejects. Auth-gated via /auth/me (reads need can_view; the
// confirm/reject/ack/resolve mutations are gated server-side, so we simply surface their errors).

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
  StatusLed,
  StatusPill,
  cx,
} from "../components/ui";
import { formatClock, formatDuration, timeAgo } from "../lib/format";
import type {
  BreachAlert,
  CameraLink,
  CameraView,
  MovementCandidate,
  PlateSearchResult,
  Principal,
  Severity,
} from "../lib/types";

/* ====================================================================== */
/* Palettes — map domain enums onto the SOC signal colors.                */
/* ====================================================================== */

const SEVERITY_COLOR: Record<Severity, string> = {
  info: "#71717a",
  warning: "#fbbf24",
  critical: "#ef4444",
};

// breach severity -> camera-state palette consumed by StatusLed / StatusPill.
const SEVERITY_TO_STATE: Record<Severity, string> = {
  info: "offline", // neutral
  warning: "connecting", // amber
  critical: "error", // red
};

const BREACH_STATUS_COLOR: Record<BreachAlert["status"], string> = {
  open: "#f59e0b", // active — needs attention
  acknowledged: "#fbbf24", // seen, not yet closed
  resolved: "#10b981", // closed
};

const CANDIDATE_STATUS_COLOR: Record<MovementCandidate["status"], string> = {
  pending: "#fbbf24",
  confirmed: "#10b981",
  rejected: "#ef4444",
};

// Preferred render order + friendly labels for the ReID signal breakdown.
const SIGNAL_ORDER = ["plate_exact", "transit", "color_match", "type_match"];
const SIGNAL_LABELS: Record<string, string> = {
  plate_exact: "Plate exact",
  transit: "Transit",
  color_match: "Color match",
  type_match: "Type match",
};

type NameFor = (id?: string | null) => string;

/* ====================================================================== */
/* Small shared bits (mirrors Entry.tsx).                    */
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

/** Inline status chip styled like the Entry.tsx severity badge. */
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

/** Probabilistic-not-identity banner reused at the top of the console + the search tab. */
function HumanLoopNote({ children }: { children: ReactNode }) {
  return (
    <div className="flex items-start gap-3 rounded-panel border border-line bg-panel px-4 py-3">
      <svg
        viewBox="0 0 20 20"
        className="mt-0.5 h-4 w-4 shrink-0 text-accent"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
      >
        <circle cx="10" cy="10" r="7.5" />
        <path d="M10 9v4" />
        <path d="M10 6.6v.4" />
      </svg>
      <p className="font-mono text-[11px] leading-relaxed text-fg-secondary">{children}</p>
    </div>
  );
}

/* ====================================================================== */
/* Signal breakdown helpers.                                              */
/* ====================================================================== */

function fmtSignal(v: unknown): { text: string; ok: boolean | null } {
  if (typeof v === "boolean") return { text: v ? "yes" : "no", ok: v };
  if (typeof v === "number" && Number.isFinite(v)) {
    // Sub-scores arrive in 0..1; transit-like values arrive as raw magnitudes.
    if (v > 0 && v <= 1) return { text: `${Math.round(v * 100)}%`, ok: v >= 0.5 };
    return { text: Number.isInteger(v) ? String(v) : v.toFixed(2), ok: null };
  }
  if (typeof v === "string") return { text: v, ok: null };
  if (v == null) return { text: "—", ok: null };
  return { text: String(v), ok: null };
}

function signalLabel(key: string): string {
  return SIGNAL_LABELS[key] ?? key.replace(/_/g, " ");
}

function SignalChips({ signals }: { signals: Record<string, unknown> }) {
  const keys = [
    ...SIGNAL_ORDER.filter((k) => k in signals),
    ...Object.keys(signals).filter((k) => !SIGNAL_ORDER.includes(k)),
  ];
  if (keys.length === 0) {
    return <span className="font-mono text-[10px] text-fg-muted">No per-signal evidence.</span>;
  }
  return (
    <div className="flex flex-wrap gap-1.5">
      {keys.map((k) => {
        const { text, ok } = fmtSignal(signals[k]);
        const color = ok === true ? "#10b981" : ok === false ? "#52525b" : "#71717a";
        return (
          <span
            key={k}
            className="inline-flex items-center gap-1 rounded border border-line bg-canvas px-1.5 py-0.5 font-mono text-[9px] leading-none"
          >
            <span className="uppercase tracking-micro text-fg-muted">{signalLabel(k)}</span>
            <span className="font-semibold" style={{ color }}>
              {text}
            </span>
          </span>
        );
      })}
    </div>
  );
}

/** Match-score meter — restrained on purpose; this is a likelihood, not a verdict. */
function ScoreBar({ score }: { score: number }) {
  const pct = Math.max(0, Math.min(100, Math.round(score * 100)));
  const color = pct >= 75 ? "#10b981" : pct >= 50 ? "#fbbf24" : "#71717a";
  return (
    <div className="flex items-center gap-2">
      <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-line">
        <div
          className="h-full rounded-full transition-[width] duration-500"
          style={{ width: `${pct}%`, backgroundColor: color }}
        />
      </div>
      <span className="w-10 shrink-0 text-right font-mono text-xs font-semibold tabular-nums text-fg">
        {pct}%
      </span>
    </div>
  );
}

/* ====================================================================== */
/* Tab: Breaches                                                          */
/* ====================================================================== */

type BreachFilter = "open" | "acknowledged" | "resolved" | "all";

function subjectText(subjectType?: string | null, subject?: string | null): string {
  if (!subjectType && !subject) return "unknown";
  if (subjectType && subject) return `${subjectType}: ${subject}`;
  return subject ?? subjectType ?? "unknown";
}

function BreachCard({
  b,
  nameFor,
  acting,
  onAck,
  onResolve,
}: {
  b: BreachAlert;
  nameFor: NameFor;
  acting: boolean;
  onAck: (b: BreachAlert) => void;
  onResolve: (b: BreachAlert) => void;
}) {
  const edge = SEVERITY_COLOR[b.severity] ?? "#52525b";
  const detail =
    field(b.detail, "message") ?? field(b.detail, "reason") ?? field(b.detail, "note");
  const isOpen = b.status === "open";

  return (
    <div
      className="flex gap-3 rounded-md border border-line bg-panel2/40 p-3 transition-colors duration-150 hover:border-[#34373e]"
      style={{ borderLeftColor: edge, borderLeftWidth: 3 }}
    >
      {b.evidence_path && <EvidenceThumb path={b.evidence_path} alt={`Breach ${b.rule}`} />}
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          {isOpen && <StatusLed state={SEVERITY_TO_STATE[b.severity] ?? "error"} pulse />}
          <StatusPill state={SEVERITY_TO_STATE[b.severity] ?? "unknown"} label={b.severity} />
          <Pill label={b.status} color={BREACH_STATUS_COLOR[b.status] ?? "#71717a"} />
          <span className="ml-auto whitespace-nowrap font-mono text-[10px] text-fg-muted">
            {formatClock(b.created_at)}
          </span>
        </div>

        <div className="mt-2 font-display text-sm font-semibold leading-snug text-fg">{b.rule}</div>

        <div className="mt-1.5 flex flex-wrap gap-x-3 gap-y-0.5 font-mono text-[10px] text-fg-secondary">
          <span className="text-fg-muted">
            subject:&nbsp;
            <span className="text-fg-secondary">{subjectText(b.subject_type, b.subject)}</span>
          </span>
          <span className="text-fg-muted">
            zone:&nbsp;
            <span className="text-fg-secondary">{b.zone_name ?? b.zone_id ?? "—"}</span>
          </span>
          <span className="text-fg-muted">
            camera:&nbsp;<span className="text-fg-secondary">{nameFor(b.camera_id)}</span>
          </span>
          {b.track_id && (
            <span className="text-fg-muted">
              track:&nbsp;<span className="text-fg-secondary">{b.track_id}</span>
            </span>
          )}
        </div>

        {detail && <p className="mt-1.5 text-xs leading-relaxed text-fg-secondary">{detail}</p>}

        {b.status !== "resolved" && (
          <div className="mt-2.5 flex items-center gap-2">
            {b.status === "open" && (
              <Button size="sm" disabled={acting} onClick={() => onAck(b)}>
                Acknowledge
              </Button>
            )}
            <Button size="sm" variant="primary" disabled={acting} onClick={() => onResolve(b)}>
              Resolve
            </Button>
            {acting && <Spinner size={13} />}
          </div>
        )}
        {b.status === "resolved" && b.resolved_at && (
          <div className="mt-2 font-mono text-[10px] text-fg-muted">
            resolved {timeAgo(b.resolved_at)}
            {b.resolved_by ? ` · by ${b.resolved_by}` : ""}
          </div>
        )}
      </div>
    </div>
  );
}

function BreachesTab({ reloadKey, nameFor }: { reloadKey: number; nameFor: NameFor }) {
  const [filter, setFilter] = useState<BreachFilter>("open");
  const breaches = usePoll(
    () => api.movementBreaches(filter === "all" ? { limit: 200 } : { status: filter, limit: 200 }),
    5000,
    [filter, reloadKey],
  );
  const [actingId, setActingId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function act(b: BreachAlert, kind: "ack" | "resolve") {
    setActingId(b.id);
    setError(null);
    try {
      if (kind === "ack") await api.ackBreach(b.id);
      else await api.resolveBreach(b.id);
      await breaches.refresh();
    } catch (e) {
      setError(e instanceof ApiError ? e.message : String(e));
    } finally {
      setActingId(null);
    }
  }

  const list = breaches.data ?? [];
  const counts = useMemo(() => {
    let critical = 0;
    let warning = 0;
    let info = 0;
    for (const b of list) {
      if (b.severity === "critical") critical += 1;
      else if (b.severity === "warning") warning += 1;
      else info += 1;
    }
    return { critical, warning, info };
  }, [list]);

  return (
    <div className="stagger space-y-4">
      <div className="grid grid-cols-2 gap-px overflow-hidden rounded-panel border border-line bg-line sm:grid-cols-4">
        <div className="bg-panel px-4 py-3">
          <Stat label="Showing" value={list.length} />
        </div>
        <div className="bg-panel px-4 py-3">
          <Stat label="Critical" value={counts.critical} tone={counts.critical > 0 ? "bad" : "default"} />
        </div>
        <div className="bg-panel px-4 py-3">
          <Stat label="Warning" value={counts.warning} tone={counts.warning > 0 ? "warn" : "default"} />
        </div>
        <div className="bg-panel px-4 py-3">
          <Stat label="Info" value={counts.info} />
        </div>
      </div>

      <Panel
        title="Red-Zone Breaches"
        subtitle="Correlated incidents · refreshes every 5s"
        actions={
          <div className="flex items-center gap-2">
            <div className="w-40">
              <Select
                aria-label="Breach status filter"
                value={filter}
                onChange={(e) => setFilter(e.target.value as BreachFilter)}
              >
                <option value="open">Open</option>
                <option value="acknowledged">Acknowledged</option>
                <option value="resolved">Resolved</option>
                <option value="all">All</option>
              </Select>
            </div>
            <span className="font-mono text-[11px] tabular-nums text-fg-muted">{list.length}</span>
          </div>
        }
      >
        {error && (
          <div className="mb-3">
            <ErrorNote>{error}</ErrorNote>
          </div>
        )}
        {breaches.error && !breaches.data ? (
          <ErrorNote>Failed to load breaches: {breaches.error}</ErrorNote>
        ) : list.length === 0 ? (
          breaches.loading ? (
            <Loading label="breaches" />
          ) : (
            <EmptyState
              title="No breaches"
              hint="Movement breaches are raised when a tracked subject crosses a restricted zone or violates a movement rule. Confirmed ones appear here colour-coded by severity."
            />
          )
        ) : (
          <div className="space-y-2.5">
            {list.map((b) => (
              <BreachCard
                key={b.id}
                b={b}
                nameFor={nameFor}
                acting={actingId === b.id}
                onAck={(x) => void act(x, "ack")}
                onResolve={(x) => void act(x, "resolve")}
              />
            ))}
          </div>
        )}
      </Panel>
    </div>
  );
}

/* ====================================================================== */
/* ReID candidate card (shared by ReID tab + Search).                     */
/* ====================================================================== */

function CandidateCard({
  c,
  nameFor,
  acting,
  onConfirm,
  onReject,
}: {
  c: MovementCandidate;
  nameFor: NameFor;
  acting?: boolean;
  onConfirm?: (c: MovementCandidate) => void;
  onReject?: (c: MovementCandidate) => void;
}) {
  const edge = CANDIDATE_STATUS_COLOR[c.status] ?? "#71717a";
  const showActions = c.status === "pending" && !!onConfirm && !!onReject;

  return (
    <div
      className="flex flex-col gap-2.5 rounded-md border border-line bg-panel2/40 p-3.5 transition-colors duration-150 hover:border-[#34373e]"
      style={{ borderLeftColor: edge, borderLeftWidth: 3 }}
    >
      <div className="flex flex-wrap items-center gap-2">
        <Pill label={c.subject_type} color="#71717a" />
        <Pill label={c.status} color={CANDIDATE_STATUS_COLOR[c.status] ?? "#71717a"} />
        <span className="ml-auto whitespace-nowrap font-mono text-[10px] text-fg-muted">
          {timeAgo(c.created_at)}
        </span>
      </div>

      <div className="flex items-baseline gap-2">
        <span className="font-mono text-[9px] uppercase tracking-micro text-fg-muted">anchor</span>
        <span className="font-mono text-base font-semibold tracking-wide text-fg">
          {c.anchor ?? "—"}
        </span>
      </div>

      <div className="flex flex-wrap items-center gap-2 font-mono text-[11px] text-fg-secondary">
        <span className="text-fg">{nameFor(c.from_camera)}</span>
        <svg viewBox="0 0 24 12" className="h-2.5 w-6 text-fg-muted" fill="none" aria-hidden="true">
          <path d="M0 6h20M16 2l5 4-5 4" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
        <span className="text-fg">{nameFor(c.to_camera)}</span>
        <span className="text-fg-muted">
          · transit&nbsp;
          <span className="text-fg-secondary">
            {c.transit_seconds != null ? formatDuration(c.transit_seconds) : "—"}
          </span>
        </span>
      </div>

      <div>
        <span className="font-mono text-[9px] uppercase tracking-micro text-fg-muted">
          Match score
        </span>
        <div className="mt-1">
          <ScoreBar score={c.score} />
        </div>
      </div>

      <div>
        <span className="font-mono text-[9px] uppercase tracking-micro text-fg-muted">
          Per-signal evidence
        </span>
        <div className="mt-1">
          <SignalChips signals={c.signals} />
        </div>
      </div>

      <p className="flex items-start gap-1.5 border-t border-line pt-2.5 text-[11px] leading-relaxed text-fg-muted">
        <WarnIcon className="mt-0.5 shrink-0 text-fg-muted/80" />
        <span>
          This is a <span className="text-fg-secondary">candidate, not an identity</span> — a
          probabilistic cross-camera correlation that requires a human decision.
        </span>
      </p>

      {showActions ? (
        <div className="flex items-center gap-2">
          <Button size="sm" variant="primary" disabled={acting} onClick={() => onConfirm!(c)}>
            Confirm
          </Button>
          <Button size="sm" variant="danger" disabled={acting} onClick={() => onReject!(c)}>
            Reject
          </Button>
          {acting && <Spinner size={13} />}
        </div>
      ) : (
        c.reviewed_at && (
          <div className="font-mono text-[10px] text-fg-muted">
            reviewed {timeAgo(c.reviewed_at)}
            {c.reviewed_by ? ` · by ${c.reviewed_by}` : ""}
          </div>
        )
      )}
    </div>
  );
}

/* ====================================================================== */
/* Tab: ReID candidates                                                   */
/* ====================================================================== */

function CandidatesTab({ reloadKey, nameFor }: { reloadKey: number; nameFor: NameFor }) {
  const candidates = usePoll(
    () => api.movementCandidates({ status: "pending", limit: 100 }),
    8000,
    [reloadKey],
  );
  const [actingId, setActingId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function act(c: MovementCandidate, kind: "confirm" | "reject") {
    setActingId(c.id);
    setError(null);
    try {
      if (kind === "confirm") await api.confirmMovementCandidate(c.id);
      else await api.rejectMovementCandidate(c.id);
      await candidates.refresh();
    } catch (e) {
      setError(e instanceof ApiError ? e.message : String(e));
    } finally {
      setActingId(null);
    }
  }

  const list = candidates.data ?? [];

  return (
    <div className="stagger space-y-4">
      <HumanLoopNote>
        <span className="text-fg">Candidates, not identities.</span> Each card below is a probabilistic
        match between two camera appearances, anchored on a plate and scored from independent signals.
        It is a lead for review — not a confirmed identity. Confirm only when the evidence warrants it;
        reject otherwise. Every decision is attributed and audited.
      </HumanLoopNote>

      <Panel
        title="ReID Candidates"
        subtitle="Pending cross-camera matches awaiting a human decision"
        actions={<span className="font-mono text-[11px] tabular-nums text-fg-muted">{list.length}</span>}
      >
        {error && (
          <div className="mb-3">
            <ErrorNote>{error}</ErrorNote>
          </div>
        )}
        {candidates.error && !candidates.data ? (
          <ErrorNote>Failed to load candidates: {candidates.error}</ErrorNote>
        ) : list.length === 0 ? (
          candidates.loading ? (
            <Loading label="candidates" />
          ) : (
            <EmptyState
              title="No pending candidates"
              hint="When movement correlation finds a plausible cross-camera match it queues a candidate here for confirm/reject. Use Recompute to re-run correlation over recent activity."
            />
          )
        ) : (
          <div className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3">
            {list.map((c) => (
              <CandidateCard
                key={c.id}
                c={c}
                nameFor={nameFor}
                acting={actingId === c.id}
                onConfirm={(x) => void act(x, "confirm")}
                onReject={(x) => void act(x, "reject")}
              />
            ))}
          </div>
        )}
      </Panel>
    </div>
  );
}

/* ====================================================================== */
/* Tab: Topology                                                          */
/* ====================================================================== */

function CameraPicker({
  id,
  value,
  cameras,
  onChange,
}: {
  id: string;
  value: string;
  cameras: CameraView[];
  onChange: (v: string) => void;
}) {
  // A camera <Select> when we know the roster; a free-text id field otherwise.
  if (cameras.length === 0) {
    return (
      <Input
        id={id}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder="camera-id"
      />
    );
  }
  return (
    <Select id={id} value={value} onChange={(e) => onChange(e.target.value)}>
      <option value="">Select camera…</option>
      {cameras.map((c) => (
        <option key={c.id} value={c.id}>
          {c.name}
        </option>
      ))}
    </Select>
  );
}

function TopologyTab({ cameras, nameFor }: { cameras: CameraView[]; nameFor: NameFor }) {
  const links = usePoll(() => api.movementLinks(), 0);

  const [fromCam, setFromCam] = useState("");
  const [toCam, setToCam] = useState("");
  const [transit, setTransit] = useState("60");
  const [bidir, setBidir] = useState(true);
  const [note, setNote] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!fromCam.trim() || !toCam.trim()) {
      setFormError("Both a from-camera and a to-camera are required.");
      return;
    }
    if (fromCam.trim() === toCam.trim()) {
      setFormError("A link must connect two different cameras.");
      return;
    }
    const body: {
      from_camera: string;
      to_camera: string;
      transit_seconds?: number;
      bidirectional?: boolean;
      note?: string;
    } = {
      from_camera: fromCam.trim(),
      to_camera: toCam.trim(),
      bidirectional: bidir,
    };
    const t = Number(transit);
    if (transit.trim() && Number.isFinite(t) && t > 0) body.transit_seconds = t;
    if (note.trim()) body.note = note.trim();

    setSubmitting(true);
    setFormError(null);
    try {
      await api.createMovementLink(body);
      setFromCam("");
      setToCam("");
      setTransit("60");
      setBidir(true);
      setNote("");
      await links.refresh();
    } catch (err) {
      setFormError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  }

  async function remove(id: string, label: string) {
    if (!window.confirm(`Delete camera link ${label}?`)) return;
    setDeleting(id);
    try {
      await api.deleteMovementLink(id);
      await links.refresh();
    } catch {
      /* reappears on next refresh if it failed */
    } finally {
      setDeleting(null);
    }
  }

  const list = links.data ?? [];

  return (
    <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
      <div className="stagger space-y-4 lg:col-span-1">
        <Panel title="Add Camera Link" subtitle="Define a plausible transit between two cameras">
          <form onSubmit={submit} className="space-y-4">
            <Field
              label={
                <>
                  From camera <span className="text-accent">*</span>
                </>
              }
              htmlFor="ml-from"
            >
              <CameraPicker id="ml-from" value={fromCam} cameras={cameras} onChange={setFromCam} />
            </Field>
            <Field
              label={
                <>
                  To camera <span className="text-accent">*</span>
                </>
              }
              htmlFor="ml-to"
            >
              <CameraPicker id="ml-to" value={toCam} cameras={cameras} onChange={setToCam} />
            </Field>
            <div className="grid grid-cols-2 gap-3">
              <Field label="Transit (s)" htmlFor="ml-transit" hint="Expected travel time">
                <Input
                  id="ml-transit"
                  type="number"
                  min={1}
                  value={transit}
                  onChange={(e) => setTransit(e.target.value)}
                  placeholder="60"
                />
              </Field>
              <Field label="Direction" htmlFor="ml-dir">
                <Select
                  id="ml-dir"
                  value={bidir ? "both" : "one"}
                  onChange={(e) => setBidir(e.target.value === "both")}
                >
                  <option value="both">Both directions</option>
                  <option value="one">One-way</option>
                </Select>
              </Field>
            </div>
            <Field label="Note" htmlFor="ml-note">
              <Input
                id="ml-note"
                value={note}
                onChange={(e) => setNote(e.target.value)}
                placeholder="Lobby → car park ramp"
              />
            </Field>
            {formError && <ErrorNote>{formError}</ErrorNote>}
            <div className="flex justify-end">
              <Button type="submit" variant="primary" disabled={submitting}>
                {submitting ? (
                  <>
                    <Spinner size={14} />
                    Adding…
                  </>
                ) : (
                  "Add link"
                )}
              </Button>
            </div>
          </form>
        </Panel>
      </div>

      <div className="stagger space-y-4 lg:col-span-2">
        <Panel
          title="Camera Topology"
          subtitle="Links that bound plausible cross-camera transits"
          padded={false}
          actions={
            list.length > 0 ? (
              <span className="font-mono text-[11px] tabular-nums text-fg-muted">{list.length}</span>
            ) : undefined
          }
        >
          {links.error && !links.data ? (
            <div className="p-4">
              <ErrorNote>Failed to load links: {links.error}</ErrorNote>
            </div>
          ) : list.length === 0 ? (
            <div className="p-4">
              {links.loading ? (
                <Loading label="topology" />
              ) : (
                <EmptyState
                  title="No camera links"
                  hint="Add a link on the left so movement correlation knows which cameras a subject can plausibly travel between, and how long that transit should take."
                />
              )}
            </div>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full border-collapse">
                <thead>
                  <tr>
                    <Th>From</Th>
                    <Th>To</Th>
                    <Th>Transit</Th>
                    <Th>Direction</Th>
                    <Th>Note</Th>
                    <Th className="text-right">Action</Th>
                  </tr>
                </thead>
                <tbody>
                  {list.map((l: CameraLink) => (
                    <tr
                      key={l.id}
                      className="border-t border-line transition-colors duration-150 hover:bg-raised/40"
                    >
                      <Td>
                        <span className="font-mono text-xs font-semibold text-fg">
                          {nameFor(l.from_camera)}
                        </span>
                      </Td>
                      <Td>
                        <span className="font-mono text-xs font-semibold text-fg">
                          {nameFor(l.to_camera)}
                        </span>
                      </Td>
                      <Td>
                        <span className="whitespace-nowrap font-mono text-[11px] text-fg-secondary">
                          {formatDuration(l.transit_seconds)}
                        </span>
                      </Td>
                      <Td>
                        <Pill
                          label={l.bidirectional ? "both" : "one-way"}
                          color={l.bidirectional ? "#10b981" : "#71717a"}
                        />
                      </Td>
                      <Td>
                        <span className="font-mono text-[11px] text-fg-secondary">
                          {l.note ?? "—"}
                        </span>
                      </Td>
                      <Td className="text-right">
                        <Button
                          size="sm"
                          variant="danger"
                          disabled={deleting === l.id}
                          onClick={() =>
                            void remove(l.id, `${nameFor(l.from_camera)} → ${nameFor(l.to_camera)}`)
                          }
                        >
                          Delete
                        </Button>
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
/* Tab: Search                                                            */
/* ====================================================================== */

function PlateTrail({
  result,
  nameFor,
}: {
  result: PlateSearchResult;
  nameFor: NameFor;
}) {
  // Oldest-first so the trail reads as a movement over time.
  const trail = useMemo(
    () =>
      [...result.appearances].sort(
        (a, b) => new Date(a.timestamp).getTime() - new Date(b.timestamp).getTime(),
      ),
    [result.appearances],
  );

  if (trail.length === 0) {
    return (
      <EmptyState
        title="No appearances"
        hint="No entry/exit events were recorded for this plate. It may not have passed a camera, or the read may have differed."
      />
    );
  }

  return (
    <ol className="relative space-y-3 pl-5">
      <span className="absolute left-[5px] top-1 bottom-1 w-px bg-line" aria-hidden="true" />
      {trail.map((a) => (
        <li key={a.event_id} className="relative">
          <span
            className="absolute -left-5 top-1.5 h-2.5 w-2.5 rounded-full border-2 border-canvas bg-accent"
            aria-hidden="true"
          />
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-mono text-xs font-semibold text-fg">{nameFor(a.camera_id)}</span>
            <Pill label={a.event_type} color="#71717a" />
            {a.direction && a.direction !== "unknown" && (
              <span className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
                {a.direction}
              </span>
            )}
            {a.auth_status && (
              <span className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
                {a.auth_status}
              </span>
            )}
            <span className="ml-auto whitespace-nowrap font-mono text-[10px] text-fg-muted">
              {formatClock(a.timestamp)}
            </span>
          </div>
        </li>
      ))}
    </ol>
  );
}

function SearchTab({ nameFor }: { nameFor: NameFor }) {
  const [plate, setPlate] = useState("");
  const [result, setResult] = useState<PlateSearchResult | null>(null);
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [searched, setSearched] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!plate.trim()) return;
    setSearching(true);
    setError(null);
    try {
      const r = await api.searchPlate(plate.trim());
      setResult(r);
      setSearched(true);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : String(err));
      setResult(null);
      setSearched(true);
    } finally {
      setSearching(false);
    }
  }

  return (
    <div className="stagger space-y-4">
      <Panel title="Plate Trail Search" subtitle="Reconstruct where a plate was seen, in time order">
        <form onSubmit={submit} className="flex flex-wrap items-end gap-3">
          <div className="w-64">
            <Field label="Plate" htmlFor="search-plate">
              <Input
                id="search-plate"
                value={plate}
                onChange={(e) => setPlate(e.target.value)}
                placeholder="ABC1234"
                autoComplete="off"
              />
            </Field>
          </div>
          <Button type="submit" variant="primary" disabled={searching || !plate.trim()}>
            {searching ? (
              <>
                <Spinner size={14} />
                Searching…
              </>
            ) : (
              "Search"
            )}
          </Button>
        </form>

        <p className="mt-3 flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-micro text-fg-muted">
          <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
            <rect x="3" y="7" width="10" height="7" rx="1.5" />
            <path d="M5.5 7V5a2.5 2.5 0 0 1 5 0v2" strokeLinecap="round" />
          </svg>
          This query is audited — the plate, the operator, and the time are recorded.
        </p>

        {error && (
          <div className="mt-3">
            <ErrorNote>{error}</ErrorNote>
          </div>
        )}
      </Panel>

      {!searched ? (
        <EmptyState
          title="Search a plate to begin"
          hint="Enter a plate above to retrieve its time-ordered appearances across cameras, plus any cross-camera candidates. Results are probabilistic and never assert identity."
        />
      ) : result ? (
        <>
          <HumanLoopNote>
            <span className="text-fg">Probabilistic, not identity.</span> {result.note}
          </HumanLoopNote>

          <Panel
            title="Appearance Trail"
            subtitle={`Plate ${result.plate} · ${result.appearances.length} appearance${
              result.appearances.length === 1 ? "" : "s"
            }`}
          >
            <PlateTrail result={result} nameFor={nameFor} />
          </Panel>

          <Panel
            title="Related Candidates"
            subtitle="Cross-camera matches anchored on this plate"
            actions={
              result.candidates.length > 0 ? (
                <span className="font-mono text-[11px] tabular-nums text-fg-muted">
                  {result.candidates.length}
                </span>
              ) : undefined
            }
          >
            {result.candidates.length === 0 ? (
              <EmptyState
                title="No related candidates"
                hint="No cross-camera correlation candidates reference this plate yet."
              />
            ) : (
              <div className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3">
                {result.candidates.map((c) => (
                  <CandidateCard key={c.id} c={c} nameFor={nameFor} />
                ))}
              </div>
            )}
          </Panel>
        </>
      ) : null}
    </div>
  );
}

/* ====================================================================== */
/* Page shell: auth gate + tabs.                                          */
/* ====================================================================== */

type TabKey = "breaches" | "candidates" | "topology" | "search";

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

export function Movement() {
  const [principal, setPrincipal] = useState<Principal | null>(null);
  const [authLoading, setAuthLoading] = useState(true);
  const [needsLogin, setNeedsLogin] = useState(false);
  const [authError, setAuthError] = useState<string | null>(null);
  const [tab, setTab] = useState<TabKey>("breaches");

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

  // Camera roster for id -> name resolution across all tabs.
  const cameras = usePoll(() => api.listCameras(), 0);
  const cameraList = cameras.data ?? [];
  const nameById = useMemo(() => {
    const m = new Map<string, string>();
    for (const c of cameraList) m.set(c.id, c.name);
    return m;
  }, [cameras.data]); // eslint-disable-line react-hooks/exhaustive-deps
  const nameFor = useCallback<NameFor>(
    (id) => (id ? (nameById.get(id) ?? id) : "—"),
    [nameById],
  );

  // Recompute correlation, then nudge the polled tabs to refetch.
  const [reloadKey, setReloadKey] = useState(0);
  const [recomputing, setRecomputing] = useState(false);
  const [recomputeError, setRecomputeError] = useState<string | null>(null);

  async function recompute() {
    setRecomputing(true);
    setRecomputeError(null);
    try {
      await api.triggerMovement();
      setReloadKey((k) => k + 1);
    } catch (e) {
      setRecomputeError(e instanceof ApiError ? e.message : String(e));
    } finally {
      setRecomputing(false);
    }
  }

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

  const tabs: { key: TabKey; label: string }[] = [
    { key: "breaches", label: "Breaches" },
    { key: "candidates", label: "ReID Candidates" },
    { key: "topology", label: "Topology" },
    { key: "search", label: "Search" },
  ];

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
            <Button variant="primary" onClick={() => void loadMe()}>
              Retry
            </Button>
          </div>
        </Panel>
      </div>
    );
  }

  if (!principal) return null;

  return (
    <div className="mx-auto max-w-[1600px] px-4 py-6 sm:px-6">
      {/* ---- Header ---- */}
      <header className="animate-rise">
        <div className="flex flex-wrap items-end justify-between gap-4">
          <div className="min-w-0">
            <SectionLabel>Intelligence · Movement</SectionLabel>
            <h1 className="mt-1 font-display text-2xl font-extrabold tracking-tight text-fg">
              Movement Intelligence
            </h1>
          </div>
          <div className="flex items-center gap-3">
            <Button onClick={() => void recompute()} disabled={recomputing}>
              {recomputing ? (
                <>
                  <Spinner size={14} />
                  Recomputing…
                </>
              ) : (
                "Recompute"
              )}
            </Button>
            <div className="flex flex-col items-end leading-none">
              <span className="font-mono text-[12px] font-semibold text-fg">{principal.name}</span>
              <span className="mt-1 font-mono text-[9px] uppercase tracking-micro text-accent">
                {principal.role}
                {principal.kind === "system" && <span className="text-fg-muted"> · auth off</span>}
              </span>
            </div>
            {principal.kind === "user" && (
              <Button size="sm" onClick={() => void signOut()}>
                Sign out
              </Button>
            )}
          </div>
        </div>

        {/* Tab bar */}
        <div className="mt-5 flex flex-wrap gap-1 overflow-x-auto border-b border-line">
          {tabs.map((t) => (
            <TabButton key={t.key} active={tab === t.key} onClick={() => setTab(t.key)}>
              {t.label}
            </TabButton>
          ))}
        </div>
      </header>

      {/* ---- Console-wide framing note ---- */}
      <div className="mt-5 animate-rise">
        <HumanLoopNote>
          <span className="text-fg">Probabilistic, human-in-the-loop.</span> Movement intelligence
          correlates anonymous tracks and plate reads across cameras to surface plausible transits and
          red-zone breaches. Nothing here is a confirmed identity — correlations are{" "}
          <span className="text-fg">candidates an operator confirms or rejects</span>, breach reviews
          are gated by role, and every decision and search is audited.
        </HumanLoopNote>
      </div>

      {recomputeError && (
        <div className="mt-3 animate-rise">
          <ErrorNote>{recomputeError}</ErrorNote>
        </div>
      )}

      <div className="mt-5">
        {tab === "breaches" && <BreachesTab reloadKey={reloadKey} nameFor={nameFor} />}
        {tab === "candidates" && <CandidatesTab reloadKey={reloadKey} nameFor={nameFor} />}
        {tab === "topology" && <TopologyTab cameras={cameraList} nameFor={nameFor} />}
        {tab === "search" && <SearchTab nameFor={nameFor} />}
      </div>
    </div>
  );
}

export default Movement;
