// Heldar Core — Incidents (evidence cases) console.
// Segments tagged with a free-form `incident_id` are grouped into a case. The left rail rolls up
// every case (segment count, footprint, span); selecting one lists its segments oldest-first with
// the option to play, evidence-lock/unlock (pins footage against retention), or retag/move the
// segment to another case. Reading is open to any authenticated principal; lock/tag mutations are
// manager+ (the API enforces this — the UI mirrors it by gating the controls).

import { useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { api, ApiError } from "../lib/api";
import { usePoll } from "../lib/usePoll";
import {
  Button,
  EmptyState,
  Panel,
  SectionLabel,
  Spinner,
  Stat,
  cx,
} from "../components/ui";
import {
  formatBytes,
  formatClock,
  formatDuration,
  formatTimeShort,
  timeAgo,
} from "../lib/format";
import type { IncidentSummary, Principal, SegmentView } from "../lib/types";

// Anchor styled like a default <Button size="sm"> (anchors can't be Buttons).
const ANCHOR_BTN =
  "inline-flex items-center justify-center gap-1.5 rounded-md border border-line bg-raised px-2.5 py-1 font-mono text-[11px] font-medium text-fg-secondary transition-colors duration-150 hover:border-[#34373e] hover:bg-[#23262c] hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-canvas";

/* ------------------------------ small bits ------------------------------ */

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

function LockIcon({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" className={className}>
      <rect x="3.5" y="7" width="9" height="6.5" rx="1.2" />
      <path d="M5.5 7V5a2.5 2.5 0 0 1 5 0v2" strokeLinecap="round" />
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

/* ------------------------------ incident list ------------------------------ */

function IncidentCard({
  incident,
  active,
  onSelect,
}: {
  incident: IncidentSummary;
  active: boolean;
  onSelect: () => void;
}) {
  const spanS =
    (new Date(incident.newest_end).getTime() - new Date(incident.oldest_start).getTime()) / 1000;
  return (
    <button
      type="button"
      onClick={onSelect}
      className={cx(
        "w-full rounded-md border bg-canvas px-3 py-2.5 text-left transition-colors duration-150",
        active
          ? "border-accent/60 bg-accent/[0.06]"
          : "border-line hover:border-[#34373e] hover:bg-raised/40",
      )}
    >
      <div className="flex items-center justify-between gap-2">
        <span className="min-w-0 truncate font-mono text-sm font-semibold text-fg">
          {incident.incident_id}
        </span>
        <span className="shrink-0 font-mono text-[11px] tabular-nums text-fg-muted">
          {incident.segment_count} seg
        </span>
      </div>
      <div className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-0.5 font-mono text-[10px] text-fg-muted">
        <span className="text-fg-secondary">{formatBytes(incident.total_bytes)}</span>
        <span className="text-fg-muted/60">·</span>
        <span>{formatDuration(spanS)} span</span>
      </div>
      <div className="mt-1 font-mono text-[10px] text-fg-muted">
        {formatClock(incident.oldest_start)} → {formatClock(incident.newest_end)}
      </div>
    </button>
  );
}

/* ------------------------------ segment row ------------------------------ */

function SegmentRow({
  seg,
  canManage,
  busy,
  onPlay,
  onToggleLock,
  onRetag,
}: {
  seg: SegmentView;
  canManage: boolean;
  busy: boolean;
  onPlay: () => void;
  onToggleLock: () => void;
  onRetag: () => void;
}) {
  return (
    <li className="flex flex-wrap items-center justify-between gap-3 rounded-md border border-line bg-canvas px-3 py-2.5 transition-colors duration-150 hover:border-[#34373e]">
      <div className="min-w-0">
        <div className="flex items-center gap-1.5 font-mono text-xs text-fg-secondary">
          {seg.evidence_locked && <LockIcon className="h-3.5 w-3.5 text-accent" />}
          <span className="tabular-nums">
            {formatTimeShort(seg.start_time)} → {formatTimeShort(seg.end_time)}
          </span>
        </div>
        <div className="mt-0.5 flex flex-wrap items-center gap-x-2 font-mono text-[10px] text-fg-muted">
          <span className="text-fg-secondary">{seg.camera_id}</span>
          <span className="text-fg-muted/60">·</span>
          <span>{formatDuration(seg.duration_s)}</span>
          <span className="text-fg-muted/60">·</span>
          <span>{formatBytes(seg.size_bytes)}</span>
          <span className="text-fg-muted/60">·</span>
          <span>{formatClock(seg.start_time)}</span>
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
          onClick={onToggleLock}
        >
          {seg.evidence_locked ? "Unlock" : "Lock"}
        </Button>
        <Button size="sm" disabled={!canManage || busy} onClick={onRetag}>
          Retag
        </Button>
      </div>
    </li>
  );
}

/* -------------------------------- page ---------------------------------- */

export function Incidents() {
  const [principal, setPrincipal] = useState<Principal | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [segBusy, setSegBusy] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [playback, setPlayback] = useState<{ src: string; label: string } | null>(null);

  // Mutations (lock/unlock/retag) are manager+. When auth is disabled the server returns the
  // `system` admin principal; when unauthenticated the controls stay gated off and listings
  // surface their own errors. Read-only view is never blocked behind a login screen.
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

  const incidents = usePoll(() => api.listIncidents(), 15000);
  const segments = usePoll(
    () => (selected ? api.incidentSegments(selected) : Promise.resolve<SegmentView[]>([])),
    selected ? 15000 : 0,
    [selected],
  );

  const list = incidents.data ?? [];
  const segList = segments.data ?? [];

  // Keep the selection valid as the roll-up changes; default to the first case.
  useEffect(() => {
    if (list.length === 0) {
      if (selected !== null) setSelected(null);
      return;
    }
    if (selected === null || !list.some((i) => i.incident_id === selected)) {
      setSelected(list[0].incident_id);
    }
  }, [list, selected]);

  const activeIncident = useMemo(
    () => list.find((i) => i.incident_id === selected) ?? null,
    [list, selected],
  );

  async function refreshBoth() {
    await Promise.all([incidents.refresh(), segments.refresh()]);
  }

  async function toggleLock(seg: SegmentView) {
    setSegBusy(seg.id);
    setActionError(null);
    try {
      if (seg.evidence_locked) await api.unlockSegmentEvidence(seg.id);
      else await api.lockSegmentEvidence(seg.id, seg.incident_id ?? selected);
      await refreshBoth();
    } catch (err) {
      setActionError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setSegBusy(null);
    }
  }

  async function retag(seg: SegmentView) {
    const input = window.prompt(
      "Retag this segment — enter an incident id (blank clears the tag):",
      seg.incident_id ?? "",
    );
    if (input === null) return; // cancelled
    const next = input.trim() ? input.trim() : null;
    setSegBusy(seg.id);
    setActionError(null);
    try {
      await api.tagSegmentIncident(seg.id, next);
      await refreshBoth();
    } catch (err) {
      setActionError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setSegBusy(null);
    }
  }

  const totalSegments = list.reduce((sum, i) => sum + i.segment_count, 0);
  const totalBytes = list.reduce((sum, i) => sum + i.total_bytes, 0);

  return (
    <div className="mx-auto max-w-[1600px] px-4 py-6 sm:px-6">
      <header className="animate-rise">
        <div className="flex flex-wrap items-end justify-between gap-4">
          <div className="min-w-0">
            <SectionLabel>Operations · Incidents</SectionLabel>
            <h1 className="mt-1 font-display text-2xl font-extrabold tracking-tight text-fg">
              Incident Cases
            </h1>
          </div>
          <div className="flex items-center gap-3">
            {principal && (
              <div className="flex flex-col items-end leading-none">
                <span className="font-mono text-[12px] font-semibold text-fg">
                  {principal.name}
                </span>
                <span className="mt-1 font-mono text-[9px] uppercase tracking-micro text-accent">
                  {principal.role}
                  {!canManage && <span className="text-fg-muted"> · read-only</span>}
                </span>
              </div>
            )}
            <Button onClick={() => void refreshBoth()} disabled={incidents.loading}>
              {incidents.loading ? <Spinner size={14} /> : "Refresh"}
            </Button>
          </div>
        </div>
      </header>

      <div className="mt-5 grid grid-cols-2 gap-px overflow-hidden rounded-panel border border-line bg-line sm:grid-cols-3">
        <div className="bg-panel px-4 py-3">
          <Stat label="Cases" value={list.length} />
        </div>
        <div className="bg-panel px-4 py-3">
          <Stat label="Tagged segments" value={totalSegments} />
        </div>
        <div className="bg-panel px-4 py-3">
          <Stat label="Footprint" value={formatBytes(totalBytes)} />
        </div>
      </div>

      <div className="mt-4 grid grid-cols-1 gap-4 lg:grid-cols-3">
        {/* Left rail: incident roll-up */}
        <div className="stagger space-y-4 lg:col-span-1">
          <Panel
            title="Cases"
            subtitle="Tagged evidence groups"
            actions={
              list.length > 0 ? (
                <span className="font-mono text-[11px] tabular-nums text-fg-muted">
                  {list.length}
                </span>
              ) : undefined
            }
          >
            {incidents.error && !incidents.data ? (
              <ErrorNote>Failed to load incidents: {incidents.error}</ErrorNote>
            ) : list.length === 0 ? (
              incidents.loading ? (
                <Loading label="incidents" />
              ) : (
                <EmptyState
                  title="No incident cases"
                  hint="Tag a recorded segment with an incident id (from a camera's segment list) to open a case here."
                />
              )
            ) : (
              <ul className="-mr-1 max-h-[640px] space-y-2 overflow-y-auto pr-1">
                {list.map((i) => (
                  <li key={i.incident_id}>
                    <IncidentCard
                      incident={i}
                      active={i.incident_id === selected}
                      onSelect={() => {
                        setSelected(i.incident_id);
                        setPlayback(null);
                        setActionError(null);
                      }}
                    />
                  </li>
                ))}
              </ul>
            )}
          </Panel>
        </div>

        {/* Right: segments for the selected case */}
        <div className="stagger space-y-4 lg:col-span-2">
          {playback && (
            <Panel
              title="Evidence Playback"
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

          <Panel
            title={activeIncident ? `Case ${activeIncident.incident_id}` : "Segments"}
            subtitle={
              activeIncident
                ? `${formatClock(activeIncident.oldest_start)} → ${formatClock(activeIncident.newest_end)}`
                : "Select a case to view its evidence"
            }
            actions={
              segList.length > 0 ? (
                <span className="font-mono text-[11px] tabular-nums text-fg-muted">
                  {segList.length}
                </span>
              ) : undefined
            }
          >
            {actionError && (
              <div className="mb-3">
                <ErrorNote>{actionError}</ErrorNote>
              </div>
            )}
            {!selected ? (
              <p className="font-mono text-xs text-fg-muted">No case selected.</p>
            ) : segments.error && !segments.data ? (
              <ErrorNote>Failed to load segments: {segments.error}</ErrorNote>
            ) : segList.length === 0 ? (
              segments.loading ? (
                <Loading label="segments" />
              ) : (
                <EmptyState
                  title="No segments in this case"
                  hint="Segments may have been retagged or pruned. Tag footage to this incident id to populate it."
                />
              )
            ) : (
              <ul className="-mr-1 max-h-[640px] space-y-2 overflow-y-auto pr-1">
                {segList.map((seg) => (
                  <SegmentRow
                    key={seg.id}
                    seg={seg}
                    canManage={canManage}
                    busy={segBusy === seg.id}
                    onPlay={() =>
                      setPlayback({
                        src: seg.url,
                        label: `${seg.camera_id} · ${formatClock(seg.start_time)}`,
                      })
                    }
                    onToggleLock={() => void toggleLock(seg)}
                    onRetag={() => void retag(seg)}
                  />
                ))}
              </ul>
            )}
            {activeIncident && segList.length > 0 && (
              <div className="mt-3 flex flex-wrap items-center gap-x-4 gap-y-1 border-t border-line pt-3 font-mono text-[10px] uppercase tracking-micro text-fg-muted">
                <span>
                  Segments:&nbsp;
                  <span className="text-fg-secondary normal-case">{activeIncident.segment_count}</span>
                </span>
                <span>
                  Footprint:&nbsp;
                  <span className="text-fg-secondary normal-case">
                    {formatBytes(activeIncident.total_bytes)}
                  </span>
                </span>
                <span>
                  Newest:&nbsp;
                  <span className="text-fg-secondary normal-case">
                    {timeAgo(activeIncident.newest_end)}
                  </span>
                </span>
              </div>
            )}
          </Panel>
        </div>
      </div>
    </div>
  );
}

export default Incidents;
