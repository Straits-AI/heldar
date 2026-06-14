// Heldar Core — Stage 7 "Semantic Search" console.
// One natural-language box over the kernel's stored facts (entry / zone / breach events). The planner
// (rules, or an optional LLM) ONLY decides *how to query* — it never answers. The answer is always the
// executed query's rows. So the trust surface here is two-fold: the interpreted plan (the single
// inference) is always shown back, and a proof ladder spells out that the results are facts and the
// interpretation is the only uncertain step. Auth-gated via /auth/me (reads need can_view).

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
import { formatClock, localInputToIso } from "../lib/format";
import type {
  Principal,
  QueryPlan,
  SearchHit,
  SearchPlanResponse,
  SearchResponse,
} from "../lib/types";

/* ====================================================================== */
/* Palettes — map domain enums onto the SOC signal colors.                */
/* ====================================================================== */

// auth_status -> camera-state palette consumed by StatusPill (mirrors Entry.tsx).
const AUTH_TO_STATE: Record<string, string> = {
  matched: "recording",
  exception: "connecting",
  blocked: "error",
  unmatched: "offline",
};
const AUTH_COLOR: Record<string, string> = {
  matched: "#10b981",
  exception: "#fbbf24",
  blocked: "#ef4444",
  unmatched: "#52525b",
};

// Fact source the hit was drawn from.
const SOURCE_COLOR: Record<string, string> = {
  entry: "#38bdf8",
  zone: "#a78bfa",
  breach: "#ef4444",
};

// Which engine produced the plan.
const PLANNER_COLOR: Record<string, string> = {
  llm: "#f59e0b",
  rules: "#38bdf8",
  structured: "#a78bfa",
};

// Proof ladder: inference (interpretive) → aggregate → event (fact).
const CLAIM_META: Record<string, { color: string; blurb: string }> = {
  inference: { color: "#fbbf24", blurb: "Interpretation — the only inference" },
  aggregate: { color: "#38bdf8", blurb: "Deterministic query over stored facts" },
  event: { color: "#10b981", blurb: "Event-level facts, each with provenance" },
  track: { color: "#10b981", blurb: "Track-level provenance" },
  observation: { color: "#10b981", blurb: "Raw observation" },
};
const CLAIM_RANK: Record<string, number> = {
  inference: 0,
  aggregate: 1,
  event: 2,
  track: 3,
  observation: 4,
};

const EXAMPLES = [
  "white cars entering after 6pm last week",
  "unauthorized vehicles today",
  "red zone breaches yesterday",
];

type NameFor = (id?: string | null) => string;

/* ====================================================================== */
/* Small shared bits (mirrors Entry.tsx / Movement.tsx).                  */
/* ====================================================================== */

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

/** Facts-not-inference banner reused at the top of the console. */
function IntelNote({ children }: { children: ReactNode }) {
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
/* Interpreted plan — the single inference, always shown back.            */
/* ====================================================================== */

function fmtHour(h: number): string {
  return `${String(h).padStart(2, "0")}:00`;
}

function PlanChip({ label, value }: { label: string; value: ReactNode }) {
  return (
    <span className="inline-flex items-center gap-1.5 rounded-md border border-line bg-canvas px-2 py-1 leading-none">
      <span className="font-mono text-[9px] uppercase tracking-micro text-fg-muted">{label}</span>
      <span className="font-mono text-[11px] font-semibold text-fg">{value}</span>
    </span>
  );
}

function planEntries(plan: QueryPlan, nameFor: NameFor): { label: string; value: string }[] {
  const e: { label: string; value: string }[] = [];
  if (plan.from) e.push({ label: "From", value: formatClock(plan.from) });
  if (plan.to) e.push({ label: "To", value: formatClock(plan.to) });
  if (plan.hour_min != null) e.push({ label: "After", value: `${fmtHour(plan.hour_min)} UTC` });
  if (plan.hour_max != null) e.push({ label: "Before", value: `${fmtHour(plan.hour_max)} UTC` });
  if (plan.cameras && plan.cameras.length > 0)
    e.push({ label: "Cameras", value: plan.cameras.map((c) => nameFor(c)).join(", ") });
  if (plan.sources && plan.sources.length > 0)
    e.push({ label: "Sources", value: plan.sources.join(" · ") });
  if (plan.plate) e.push({ label: "Plate", value: plan.plate });
  if (plan.color) e.push({ label: "Color", value: plan.color });
  if (plan.vehicle_type) e.push({ label: "Vehicle", value: plan.vehicle_type });
  if (plan.subject_type) e.push({ label: "Subject", value: plan.subject_type });
  if (plan.auth_status && plan.auth_status.length > 0)
    e.push({ label: "Auth", value: plan.auth_status.join(" · ") });
  if (plan.event_type) e.push({ label: "Event", value: plan.event_type });
  if (plan.zone_kind) e.push({ label: "Zone kind", value: plan.zone_kind });
  if (plan.text) e.push({ label: "Text", value: plan.text });
  if (plan.limit != null) e.push({ label: "Limit", value: String(plan.limit) });
  return e;
}

function PlanView({
  plan,
  planner,
  nameFor,
  dryRun,
}: {
  plan: QueryPlan;
  planner: string;
  nameFor: NameFor;
  dryRun?: boolean;
}) {
  const entries = useMemo(() => planEntries(plan, nameFor), [plan, nameFor]);
  const plannerColor = PLANNER_COLOR[planner] ?? "#71717a";

  return (
    <Panel
      title="Interpreted as"
      subtitle="The structured plan your question was turned into — the only inference in the answer"
      actions={
        <div className="flex items-center gap-2">
          {dryRun && <Pill label="Dry run" color="#fbbf24" />}
          <Pill label={`Planner · ${planner}`} color={plannerColor} />
        </div>
      }
    >
      {entries.length === 0 ? (
        <p className="font-mono text-[11px] leading-relaxed text-fg-secondary">
          No filters were extracted — this defaults to{" "}
          <span className="text-fg">all sources</span> over the last ~7 days. Add detail (color,
          time, camera, authorization) to narrow it.
        </p>
      ) : (
        <div className="flex flex-wrap gap-2">
          {entries.map((it) => (
            <PlanChip key={it.label} label={it.label} value={it.value} />
          ))}
        </div>
      )}
      <p className="mt-3 flex items-start gap-1.5 border-t border-line pt-3 font-mono text-[10px] leading-relaxed text-fg-muted">
        <WarnIcon className="mt-0.5 shrink-0 text-fg-muted/80" />
        <span>
          Verify this reflects your intent — the planner only decides{" "}
          <span className="text-fg-secondary">how to query</span>. The results are exactly what this
          plan selected, nothing more.
        </span>
      </p>
    </Panel>
  );
}

/* ====================================================================== */
/* Proof ladder — observation→track→event→aggregate→inference.            */
/* ====================================================================== */

function lvlStr(obj: Record<string, unknown>, key: string): string | null {
  const v = obj[key];
  return typeof v === "string" && v.trim() ? v : null;
}

function ProofLadder({
  proof,
}: {
  proof: { claim_levels: Array<Record<string, unknown>>; note: string };
}) {
  const levels = useMemo(() => {
    const arr = [...(proof.claim_levels ?? [])];
    arr.sort((a, b) => {
      const ra = CLAIM_RANK[String(a.level ?? "")] ?? 99;
      const rb = CLAIM_RANK[String(b.level ?? "")] ?? 99;
      return ra - rb;
    });
    return arr;
  }, [proof.claim_levels]);

  return (
    <Panel
      title="Proof"
      subtitle="Why this answer can be trusted — facts at the bottom, interpretation at the top"
    >
      <p className="mb-4 rounded-md border border-accent/30 bg-accent/[0.06] px-3 py-2 text-xs leading-relaxed text-fg-secondary">
        <span className="font-semibold text-fg">The answers are facts; the interpretation is the
        only inference.</span>{" "}
        Each rung below states a claim, its confidence, and the caveat that bounds it.
      </p>

      <ol className="relative space-y-3 pl-5">
        <span className="absolute left-[5px] top-2 bottom-2 w-px bg-line" aria-hidden="true" />
        {levels.map((lvl, i) => {
          const level = lvlStr(lvl, "level") ?? "—";
          const meta = CLAIM_META[level] ?? { color: "#71717a", blurb: "" };
          const statement = lvlStr(lvl, "statement");
          const confidence = lvlStr(lvl, "confidence");
          const caveat = lvlStr(lvl, "caveat");
          const basis = lvlStr(lvl, "basis");
          const provenance = lvlStr(lvl, "provenance");
          return (
            <li key={`${level}-${i}`} className="relative">
              <span
                className="absolute -left-5 top-1.5 h-2.5 w-2.5 rounded-full border-2 border-canvas"
                style={{ backgroundColor: meta.color }}
                aria-hidden="true"
              />
              <div
                className="rounded-md border border-line bg-panel2/40 p-3"
                style={{ borderLeftColor: meta.color, borderLeftWidth: 3 }}
              >
                <div className="flex flex-wrap items-center gap-2">
                  <Pill label={level} color={meta.color} />
                  {meta.blurb && (
                    <span className="font-mono text-[10px] text-fg-muted">{meta.blurb}</span>
                  )}
                  {confidence && (
                    <span className="ml-auto whitespace-nowrap font-mono text-[10px] text-fg-secondary">
                      confidence:&nbsp;<span className="text-fg">{confidence}</span>
                    </span>
                  )}
                </div>
                {statement && (
                  <p className="mt-2 text-xs leading-relaxed text-fg-secondary">{statement}</p>
                )}
                {basis && (
                  <p className="mt-1.5 font-mono text-[10px] leading-relaxed text-fg-muted">
                    basis: {basis}
                  </p>
                )}
                {provenance && (
                  <p className="mt-1.5 font-mono text-[10px] leading-relaxed text-fg-muted">
                    provenance: {provenance}
                  </p>
                )}
                {caveat && (
                  <p className="mt-2 flex items-start gap-1.5 rounded border border-connecting/30 bg-connecting/[0.06] px-2 py-1.5 font-mono text-[10px] leading-relaxed text-connecting">
                    <WarnIcon className="mt-0.5 shrink-0" />
                    <span>{caveat}</span>
                  </p>
                )}
              </div>
            </li>
          );
        })}
      </ol>

      {proof.note && (
        <p className="mt-4 border-t border-line pt-3 font-mono text-[10px] leading-relaxed text-fg-muted">
          {proof.note}
        </p>
      )}
    </Panel>
  );
}

/* ====================================================================== */
/* Result hit card.                                                       */
/* ====================================================================== */

function HitCard({ hit, nameFor }: { hit: SearchHit; nameFor: NameFor }) {
  const sourceColor = SOURCE_COLOR[hit.source] ?? "#71717a";
  const authColor = hit.auth_status ? AUTH_COLOR[hit.auth_status] : undefined;
  const edge = authColor ?? sourceColor;

  const color = field(hit.subject, "color");
  const vType = field(hit.subject, "vehicle_type");
  const label = field(hit.subject, "label");
  const subjectType = field(hit.subject, "subject_type") ?? field(hit.subject, "type");
  const severity = field(hit.subject, "severity");

  return (
    <div
      className="flex gap-3 rounded-md border border-line bg-panel2/40 p-3 transition-colors duration-150 hover:border-[#34373e]"
      style={{ borderLeftColor: edge, borderLeftWidth: 3 }}
    >
      {hit.evidence_path && (
        <EvidenceThumb path={hit.evidence_path} alt={`${hit.source} ${hit.plate ?? hit.id}`} />
      )}
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <Pill label={hit.source} color={sourceColor} />
          <span className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
            {hit.kind}
          </span>
          {hit.claim_level && <Pill label={hit.claim_level} color="#52525b" />}
          <span className="ml-auto whitespace-nowrap font-mono text-[10px] text-fg-muted">
            {formatClock(hit.timestamp)}
          </span>
        </div>

        <div className="mt-2 flex flex-wrap items-center gap-2">
          {hit.plate ? (
            <span className="font-mono text-base font-semibold tracking-wide text-fg">
              {hit.plate}
            </span>
          ) : (
            <span className="font-mono text-sm text-fg-secondary">
              {label ?? subjectType ?? "—"}
            </span>
          )}
          {hit.auth_status && (
            <StatusPill state={AUTH_TO_STATE[hit.auth_status] ?? "unknown"} label={hit.auth_status} />
          )}
        </div>

        <div className="mt-1.5 flex flex-wrap gap-x-3 gap-y-0.5 font-mono text-[10px] text-fg-secondary">
          <span className="text-fg-muted">
            camera:&nbsp;<span className="text-fg-secondary">{nameFor(hit.camera_id)}</span>
          </span>
          {hit.zone && (
            <span className="text-fg-muted">
              zone:&nbsp;<span className="text-fg-secondary">{hit.zone}</span>
              {hit.zone_kind ? <span className="text-fg-muted"> ({hit.zone_kind})</span> : null}
            </span>
          )}
          {subjectType && hit.plate && (
            <span className="text-fg-muted">
              subject:&nbsp;<span className="text-fg-secondary">{subjectType}</span>
            </span>
          )}
          {vType && (
            <span className="text-fg-muted">
              type:&nbsp;<span className="text-fg-secondary">{vType}</span>
            </span>
          )}
          {color && (
            <span className="text-fg-muted">
              color:&nbsp;<span className="text-fg-secondary">{color}</span>
            </span>
          )}
          {severity && (
            <span className="text-fg-muted">
              severity:&nbsp;<span className="text-fg-secondary">{severity}</span>
            </span>
          )}
        </div>
      </div>
    </div>
  );
}

/* ====================================================================== */
/* Results — count + hit list.                                            */
/* ====================================================================== */

function Results({ result, nameFor }: { result: SearchResponse; nameFor: NameFor }) {
  const counts = useMemo(() => {
    let entry = 0;
    let zone = 0;
    let breach = 0;
    for (const h of result.hits) {
      if (h.source === "entry") entry += 1;
      else if (h.source === "zone") zone += 1;
      else if (h.source === "breach") breach += 1;
    }
    return { entry, zone, breach };
  }, [result.hits]);

  return (
    <>
      <div className="grid grid-cols-2 gap-px overflow-hidden rounded-panel border border-line bg-line sm:grid-cols-4">
        <div className="bg-panel px-4 py-3">
          <Stat label="Matches" value={result.count} />
        </div>
        <div className="bg-panel px-4 py-3">
          <Stat label="Entry" value={counts.entry} />
        </div>
        <div className="bg-panel px-4 py-3">
          <Stat label="Zone" value={counts.zone} />
        </div>
        <div className="bg-panel px-4 py-3">
          <Stat label="Breach" value={counts.breach} tone={counts.breach > 0 ? "bad" : "default"} />
        </div>
      </div>

      <Panel
        title="Results"
        subtitle="Stored events matching the executed plan — newest first"
        actions={
          <span className="font-mono text-[11px] tabular-nums text-fg-muted">{result.count}</span>
        }
      >
        {result.hits.length === 0 ? (
          <EmptyState
            title="No matching events"
            hint="The plan ran cleanly but no stored events matched. Loosen the filters above, widen the time window, or check the interpreted plan."
          />
        ) : (
          <div className="space-y-2.5">
            {result.hits.map((h) => (
              <HitCard key={`${h.source}-${h.id}`} hit={h} nameFor={nameFor} />
            ))}
          </div>
        )}
      </Panel>
    </>
  );
}

/* ====================================================================== */
/* Structured filter form (collapsible) → /search/events.                */
/* ====================================================================== */

function StructuredForm({
  busy,
  onRun,
}: {
  busy: boolean;
  onRun: (plan: QueryPlan) => void;
}) {
  const [source, setSource] = useState("");
  const [auth, setAuth] = useState("");
  const [color, setColor] = useState("");
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");

  function submit(e: FormEvent) {
    e.preventDefault();
    const plan: QueryPlan = {};
    if (source) plan.sources = [source];
    if (auth) plan.auth_status = [auth];
    if (color.trim()) plan.color = color.trim();
    const f = localInputToIso(from);
    if (f) plan.from = f;
    const t = localInputToIso(to);
    if (t) plan.to = t;
    onRun(plan);
  }

  return (
    <form onSubmit={submit} className="space-y-4">
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
        <Field label="Source" htmlFor="sf-source">
          <Select id="sf-source" value={source} onChange={(e) => setSource(e.target.value)}>
            <option value="">Any source</option>
            <option value="entry">Entry</option>
            <option value="zone">Zone</option>
            <option value="breach">Breach</option>
          </Select>
        </Field>
        <Field label="Authorization" htmlFor="sf-auth">
          <Select id="sf-auth" value={auth} onChange={(e) => setAuth(e.target.value)}>
            <option value="">Any status</option>
            <option value="matched">Matched</option>
            <option value="exception">Exception</option>
            <option value="unmatched">Unmatched</option>
            <option value="blocked">Blocked</option>
          </Select>
        </Field>
        <Field label="Color" htmlFor="sf-color">
          <Input
            id="sf-color"
            value={color}
            onChange={(e) => setColor(e.target.value)}
            placeholder="white"
            autoComplete="off"
          />
        </Field>
        <Field label="From" htmlFor="sf-from">
          <Input
            id="sf-from"
            type="datetime-local"
            step={1}
            value={from}
            onChange={(e) => setFrom(e.target.value)}
          />
        </Field>
        <Field label="To" htmlFor="sf-to">
          <Input
            id="sf-to"
            type="datetime-local"
            step={1}
            value={to}
            onChange={(e) => setTo(e.target.value)}
          />
        </Field>
      </div>
      <div className="flex justify-end">
        <Button type="submit" variant="primary" disabled={busy}>
          {busy ? (
            <>
              <Spinner size={14} />
              Running…
            </>
          ) : (
            "Run structured query"
          )}
        </Button>
      </div>
    </form>
  );
}

/* ====================================================================== */
/* Page shell: auth gate + search console.                                */
/* ====================================================================== */

type Busy = "nl" | "plan" | "structured" | null;

function SearchConsole({ nameFor }: { nameFor: NameFor }) {
  const [query, setQuery] = useState("");
  const [result, setResult] = useState<SearchResponse | null>(null);
  const [planOnly, setPlanOnly] = useState<SearchPlanResponse | null>(null);
  const [busy, setBusy] = useState<Busy>(null);
  const [error, setError] = useState<string | null>(null);
  const [searched, setSearched] = useState(false);
  const [showFilters, setShowFilters] = useState(false);

  const runNl = useCallback(async (text: string) => {
    const q = text.trim();
    if (!q) return;
    setQuery(q);
    setBusy("nl");
    setError(null);
    setPlanOnly(null);
    try {
      const r = await api.searchNl(q);
      setResult(r);
      setSearched(true);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : String(e));
      setResult(null);
      setSearched(true);
    } finally {
      setBusy(null);
    }
  }, []);

  async function runPlan() {
    const q = query.trim();
    if (!q) return;
    setBusy("plan");
    setError(null);
    setResult(null);
    try {
      const r = await api.searchPlan(q);
      setPlanOnly(r);
      setSearched(true);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : String(e));
      setPlanOnly(null);
      setSearched(true);
    } finally {
      setBusy(null);
    }
  }

  const runStructured = useCallback(async (plan: QueryPlan) => {
    setBusy("structured");
    setError(null);
    setPlanOnly(null);
    try {
      const r = await api.searchEvents(plan);
      setResult(r);
      setSearched(true);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : String(e));
      setResult(null);
      setSearched(true);
    } finally {
      setBusy(null);
    }
  }, []);

  function onSubmit(e: FormEvent) {
    e.preventDefault();
    void runNl(query);
  }

  const displayPlan = planOnly?.plan ?? result?.plan ?? null;
  const displayPlanner = planOnly?.planner ?? result?.planner ?? "rules";

  return (
    <div className="stagger space-y-4">
      <IntelNote>
        <span className="text-fg">Ask in plain language; the answer is the data.</span> A planner
        (transparent rules, or an optional LLM) translates your question into a structured query —
        that interpretation is the only inference. The plan then runs deterministically over the
        kernel's stored events, so{" "}
        <span className="text-fg">the answers are facts, the interpretation is the only inference</span>
        . Every search is logged; plate-targeted queries are audited.
      </IntelNote>

      {/* Query box */}
      <Panel title="Ask" subtitle="Natural-language search over entry, zone & breach events">
        <form onSubmit={onSubmit} className="flex flex-col gap-3 sm:flex-row sm:items-end">
          <div className="min-w-0 flex-1">
            <Field label="Query" htmlFor="nl-query">
              <Input
                id="nl-query"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="white cars entering after 6pm last week"
                autoComplete="off"
              />
            </Field>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <Button type="submit" variant="primary" disabled={busy !== null || !query.trim()}>
              {busy === "nl" ? (
                <>
                  <Spinner size={14} />
                  Searching…
                </>
              ) : (
                "Search"
              )}
            </Button>
            <Button
              type="button"
              onClick={() => void runPlan()}
              disabled={busy !== null || !query.trim()}
            >
              {busy === "plan" ? (
                <>
                  <Spinner size={14} />
                  Planning…
                </>
              ) : (
                "Plan only (dry-run)"
              )}
            </Button>
          </div>
        </form>

        <div className="mt-3 flex flex-wrap items-center gap-2">
          <span className="font-mono text-[9px] uppercase tracking-micro text-fg-muted">Try</span>
          {EXAMPLES.map((ex) => (
            <button
              key={ex}
              type="button"
              disabled={busy !== null}
              onClick={() => void runNl(ex)}
              className="rounded-full border border-line bg-canvas px-2.5 py-1 font-mono text-[10px] text-fg-secondary transition-colors duration-150 hover:border-accent/50 hover:text-fg disabled:cursor-not-allowed disabled:opacity-50"
            >
              {ex}
            </button>
          ))}
        </div>

        <p className="mt-3 flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-micro text-fg-muted">
          <svg
            viewBox="0 0 16 16"
            width="12"
            height="12"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            aria-hidden="true"
          >
            <rect x="3" y="7" width="10" height="7" rx="1.5" />
            <path d="M5.5 7V5a2.5 2.5 0 0 1 5 0v2" strokeLinecap="round" />
          </svg>
          Searches are logged · plate-targeted queries are audited.
        </p>

        {error && (
          <div className="mt-3">
            <ErrorNote>{error}</ErrorNote>
          </div>
        )}

        {/* Collapsible structured filter form */}
        <div className="mt-4 border-t border-line pt-3">
          <button
            type="button"
            onClick={() => setShowFilters((v) => !v)}
            className="flex items-center gap-1.5 font-mono text-[10px] font-semibold uppercase tracking-micro text-fg-secondary transition-colors duration-150 hover:text-fg"
          >
            <svg
              viewBox="0 0 16 16"
              width="12"
              height="12"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.6"
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden="true"
              className={cx("transition-transform duration-150", showFilters && "rotate-90")}
            >
              <path d="M6 4l4 4-4 4" />
            </svg>
            Structured filters
          </button>
          {showFilters && (
            <div className="mt-3">
              <StructuredForm busy={busy === "structured"} onRun={(p) => void runStructured(p)} />
            </div>
          )}
        </div>
      </Panel>

      {/* Interpreted plan — always shown once we have one */}
      {displayPlan && (
        <PlanView
          plan={displayPlan}
          planner={displayPlanner}
          nameFor={nameFor}
          dryRun={planOnly != null}
        />
      )}

      {/* Dry-run: plan shown but not executed */}
      {planOnly && (
        <Panel
          title="Dry run — not executed"
          subtitle="The plan above was generated but no query was run"
        >
          <p className="text-xs leading-relaxed text-fg-secondary">
            Nothing was read from the fact tables. Review the interpreted plan, then execute it
            exactly as shown.
          </p>
          <div className="mt-3 flex items-center gap-2">
            <Button
              variant="primary"
              disabled={busy !== null}
              onClick={() => void runStructured(planOnly.plan)}
            >
              {busy === "structured" ? (
                <>
                  <Spinner size={14} />
                  Running…
                </>
              ) : (
                "Run this plan"
              )}
            </Button>
            <Button disabled={busy !== null} onClick={() => void runNl(query)}>
              Re-run as search
            </Button>
          </div>
        </Panel>
      )}

      {/* Results + proof, for an executed search */}
      {result && (
        <>
          <Results result={result} nameFor={nameFor} />
          <ProofLadder proof={result.proof} />
        </>
      )}

      {/* First-load empty state */}
      {!searched && !displayPlan && (
        <EmptyState
          title="Ask a question to begin"
          hint="Search in plain language across entry, zone and breach events. The interpreted plan and a proof ladder are shown with every result, so you can always see how the question was read and why the answer holds."
        />
      )}
    </div>
  );
}

export function Search() {
  const [principal, setPrincipal] = useState<Principal | null>(null);
  const [authLoading, setAuthLoading] = useState(true);
  const [needsLogin, setNeedsLogin] = useState(false);
  const [authError, setAuthError] = useState<string | null>(null);

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

  // Camera roster for id -> name resolution.
  const cameras = usePoll(() => api.listCameras(), 0);
  const cameraList = cameras.data ?? [];
  const nameById = useMemo(() => {
    const m = new Map<string, string>();
    for (const c of cameraList) m.set(c.id, c.name);
    return m;
  }, [cameras.data]); // eslint-disable-line react-hooks/exhaustive-deps
  const nameFor = useCallback<NameFor>((id) => (id ? (nameById.get(id) ?? id) : "—"), [nameById]);

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
      <header className="animate-rise">
        <div className="flex flex-wrap items-end justify-between gap-4">
          <div className="min-w-0">
            <SectionLabel>Intelligence · Search</SectionLabel>
            <h1 className="mt-1 font-display text-2xl font-extrabold tracking-tight text-fg">
              Semantic Search
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
              <Button size="sm" onClick={() => void signOut()}>
                Sign out
              </Button>
            )}
          </div>
        </div>
      </header>

      <div className="mt-5">
        <SearchConsole nameFor={nameFor} />
      </div>
    </div>
  );
}

export default Search;
