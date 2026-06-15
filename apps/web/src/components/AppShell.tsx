// Heldar Core — application shell: left nav rail + top telemetry status bar.
// The telemetry bar polls GET /api/v1/system (~5s) and renders live operations data.

import { useEffect, useState } from "react";
import type { ReactNode } from "react";
import { NavLink } from "react-router-dom";
import { api } from "../lib/api";
import { usePoll } from "../lib/usePoll";
import { formatBytes, formatUptime } from "../lib/format";
import { cx, Spinner, StatusLed } from "./ui";

/* ---------------------------------------------------------------- */
/* Nav rail                                                         */
/* ---------------------------------------------------------------- */

type IconProps = { className?: string };

function WallIcon({ className }: IconProps) {
  return (
    <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" className={className}>
      <rect x="2.5" y="2.5" width="6" height="6" rx="1" />
      <rect x="11.5" y="2.5" width="6" height="6" rx="1" />
      <rect x="2.5" y="11.5" width="6" height="6" rx="1" />
      <rect x="11.5" y="11.5" width="6" height="6" rx="1" />
    </svg>
  );
}

function DiscoverIcon({ className }: IconProps) {
  return (
    <svg
      viewBox="0 0 20 20"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      className={className}
    >
      <circle cx="10" cy="10" r="2" />
      <path d="M10 10 13.5 6.5" />
      <path d="M5.4 14.6a6.5 6.5 0 0 1 0-9.2" opacity="0.85" />
      <path d="M14.6 5.4a6.5 6.5 0 0 1 0 9.2" opacity="0.85" />
      <path d="M3.3 16.7a9.5 9.5 0 0 1 0-13.4" opacity="0.5" />
      <path d="M16.7 3.3a9.5 9.5 0 0 1 0 13.4" opacity="0.5" />
    </svg>
  );
}

function AddCameraIcon({ className }: IconProps) {
  return (
    <svg
      viewBox="0 0 20 20"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
    >
      <path d="M3 7.5a1.5 1.5 0 0 1 1.5-1.5h2l1.2-1.6h2.1" />
      <path d="M14.5 6H16a1.5 1.5 0 0 1 1.5 1.5V14a1.5 1.5 0 0 1-1.5 1.5H4.5A1.5 1.5 0 0 1 3 14v-4" />
      <circle cx="10.25" cy="11" r="2.6" />
      <path d="M15 4.2v3.6M13.2 6h3.6" />
    </svg>
  );
}

function AiIcon({ className }: IconProps) {
  return (
    <svg
      viewBox="0 0 20 20"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
    >
      <rect x="5.5" y="5.5" width="9" height="9" rx="1.5" />
      <path d="M8 5.5V3M12 5.5V3M8 17v-2.5M12 17v-2.5M5.5 8H3M5.5 12H3M17 8h-2.5M17 12h-2.5" />
      <circle cx="10" cy="10" r="1.6" />
    </svg>
  );
}

function EntryIcon({ className }: IconProps) {
  return (
    <svg
      viewBox="0 0 20 20"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
    >
      <path d="M3 16.5V6l5-2.5V16.5" />
      <path d="M2 16.5h6" />
      <path d="M8 8h9a1 1 0 0 1 1 1v6.5" />
      <path d="M11 16.5V8" />
      <path d="M14.5 16.5V8" />
      <path d="M5.4 9.6h.01" />
    </svg>
  );
}


function MovementIcon({ className }: IconProps) {
  return (
    <svg
      viewBox="0 0 20 20"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
    >
      <circle cx="4.5" cy="5.5" r="2" />
      <circle cx="15.5" cy="14.5" r="2" />
      <path d="M6.4 6.6l7.2 6.8" />
      <path d="M13.5 5l3 1-1 3" />
    </svg>
  );
}

function SearchIcon({ className }: IconProps) {
  return (
    <svg
      viewBox="0 0 20 20"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
    >
      <circle cx="8.5" cy="8.5" r="5" />
      <path d="M12.5 12.5L17 17" />
    </svg>
  );
}

function IncidentsIcon({ className }: IconProps) {
  return (
    <svg
      viewBox="0 0 20 20"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
    >
      <path d="M10 2.5 17 15H3z" />
      <path d="M10 8v3.5" />
      <path d="M10 13.6v.4" />
    </svg>
  );
}

function BackupIcon({ className }: IconProps) {
  return (
    <svg
      viewBox="0 0 20 20"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
    >
      <ellipse cx="10" cy="5" rx="6" ry="2.5" />
      <path d="M4 5v10c0 1.38 2.69 2.5 6 2.5s6-1.12 6-2.5V5" />
      <path d="M4 10c0 1.38 2.69 2.5 6 2.5s6-1.12 6-2.5" />
    </svg>
  );
}

function SystemIcon({ className }: IconProps) {
  return (
    <svg
      viewBox="0 0 20 20"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
    >
      <path d="M2.5 12.5h3l1.5-5 2.5 9 2-7 1.5 3h4" />
    </svg>
  );
}

const NAV_ITEMS: { to: string; label: string; end?: boolean; Icon: (p: IconProps) => ReactNode }[] = [
  { to: "/", label: "Wall", end: true, Icon: WallIcon },
  { to: "/discover", label: "Discover", Icon: DiscoverIcon },
  { to: "/cameras/new", label: "Add Camera", Icon: AddCameraIcon },
  { to: "/ai", label: "AI", Icon: AiIcon },
  { to: "/entry", label: "Entry", Icon: EntryIcon },
  { to: "/movement", label: "Movement", Icon: MovementIcon },
  { to: "/search", label: "Search", Icon: SearchIcon },
  { to: "/incidents", label: "Incidents", Icon: IncidentsIcon },
  { to: "/backup", label: "Backup", Icon: BackupIcon },
  { to: "/system", label: "System", Icon: SystemIcon },
];

function NavRail({ version }: { version?: string }) {
  return (
    <aside className="sticky top-0 z-30 hidden h-screen w-[232px] shrink-0 flex-col border-r border-line bg-panel sm:flex">
      {/* Wordmark */}
      <div className="flex items-center gap-3 border-b border-line px-5 py-5">
        <span className="relative flex h-9 w-9 items-center justify-center rounded-md border border-accent/40 bg-canvas">
          <svg viewBox="0 0 24 24" className="h-5 w-5" fill="none">
            <circle cx="12" cy="12" r="8" stroke="#f59e0b" strokeWidth="1.8" />
            <circle cx="12" cy="12" r="2.4" fill="#f59e0b" />
          </svg>
        </span>
        <div className="leading-none">
          <div className="font-display text-[15px] font-extrabold tracking-wider text-fg">
            HELDAR
          </div>
          <div className="mt-1 font-mono text-[9px] uppercase tracking-micro text-accent">
            Core
          </div>
        </div>
      </div>

      {/* Nav */}
      <nav className="flex flex-1 flex-col gap-1 px-3 py-4">
        <div className="px-2 pb-2 font-mono text-[9px] uppercase tracking-micro text-fg-muted">
          Operations
        </div>
        {NAV_ITEMS.map(({ to, label, end, Icon }) => (
          <NavLink
            key={to}
            to={to}
            end={end}
            className={({ isActive }) =>
              cx(
                "group relative flex items-center gap-3 rounded-md px-3 py-2 text-sm font-medium transition-colors duration-150",
                isActive
                  ? "bg-raised text-fg"
                  : "text-fg-secondary hover:bg-raised/60 hover:text-fg",
              )
            }
          >
            {({ isActive }) => (
              <>
                <span
                  className={cx(
                    "absolute left-0 top-1/2 h-4 w-[3px] -translate-y-1/2 rounded-r-full bg-accent transition-opacity duration-150",
                    isActive ? "opacity-100" : "opacity-0",
                  )}
                />
                <Icon
                  className={cx(
                    "h-[18px] w-[18px] transition-colors",
                    isActive ? "text-accent" : "text-fg-muted group-hover:text-fg-secondary",
                  )}
                />
                <span>{label}</span>
              </>
            )}
          </NavLink>
        ))}
      </nav>

      {/* Footer */}
      <div className="border-t border-line px-5 py-4">
        <div className="font-mono text-[9px] uppercase tracking-micro text-fg-muted">
          Build
        </div>
        <div className="mt-1 font-mono text-[11px] text-fg-secondary">
          {version ? `v${version}` : "—"}
        </div>
      </div>
    </aside>
  );
}

/* ---------------------------------------------------------------- */
/* Telemetry bar                                                    */
/* ---------------------------------------------------------------- */

function Metric({
  label,
  children,
}: {
  label: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-0.5 leading-none">
      <span className="font-mono text-[9px] uppercase tracking-micro text-fg-muted">{label}</span>
      <span className="font-mono text-[13px] font-semibold text-fg">{children}</span>
    </div>
  );
}

function Divider() {
  return <span className="hidden h-7 w-px bg-line md:block" />;
}

function useClock(): Date {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const t = setInterval(() => setNow(new Date()), 1000);
    return () => clearInterval(t);
  }, []);
  return now;
}

function StorageGauge({ usedGb, maxGb }: { usedGb: number; maxGb: number }) {
  const pct = maxGb > 0 ? Math.min(100, (usedGb / maxGb) * 100) : 0;
  const fill = pct > 90 ? "#ef4444" : pct > 75 ? "#fbbf24" : "#f59e0b";
  return (
    <div className="flex flex-col gap-1 leading-none">
      <span className="font-mono text-[9px] uppercase tracking-micro text-fg-muted">Storage</span>
      <div className="flex items-center gap-2">
        <div className="h-1.5 w-24 overflow-hidden rounded-full bg-line">
          <div
            className="h-full rounded-full transition-[width] duration-500"
            style={{ width: `${pct}%`, backgroundColor: fill }}
          />
        </div>
        <span className="whitespace-nowrap font-mono text-[12px] font-semibold text-fg">
          {formatBytes(usedGb * 1024 ** 3)}
          <span className="text-fg-muted"> / {maxGb.toFixed(0)}G</span>
        </span>
      </div>
    </div>
  );
}

function TelemetryBar() {
  const { data, error } = usePoll(() => api.system(), 5000);
  const clock = useClock();
  const recording = data?.cameras_recording ?? 0;
  const online = !!data && !error;

  return (
    <header className="sticky top-0 z-20 border-b border-line bg-panel/85 backdrop-blur supports-[backdrop-filter]:bg-panel/70">
      <div className="flex h-14 items-center gap-5 overflow-x-auto px-4 sm:px-6">
        {/* Link status */}
        <div className="flex items-center gap-2">
          <StatusLed
            state={online ? "recording" : error ? "error" : "connecting"}
            pulse={online}
          />
          <span className="font-mono text-[10px] uppercase tracking-micro text-fg-secondary">
            {online ? "Core Online" : error ? "Core Unreachable" : "Linking"}
          </span>
        </div>

        <Divider />

        {error && !data ? (
          <span className="truncate font-mono text-[12px] text-danger">{error}</span>
        ) : !data ? (
          <span className="flex items-center gap-2 font-mono text-[12px] text-fg-muted">
            <Spinner size={13} /> Reading telemetry…
          </span>
        ) : (
          <>
            <div className="flex flex-col gap-0.5 leading-none">
              <span className="font-mono text-[9px] uppercase tracking-micro text-fg-muted">
                Recording
              </span>
              <span className="font-mono text-[13px] font-semibold tabular-nums text-rec">
                {recording}
                <span className="text-fg-muted"> / {data.cameras_total}</span>
              </span>
            </div>

            <Divider />
            <Metric label="Cameras">
              <span className="tabular-nums">{data.cameras_total}</span>
            </Metric>

            <Divider />
            <Metric label="Recorders">
              <span className="tabular-nums">{data.active_recorders}</span>
            </Metric>

            <Divider />
            <Metric label="Segments">
              <span className="tabular-nums">{data.segments_total.toLocaleString()}</span>
            </Metric>

            <Divider />
            <StorageGauge usedGb={data.recordings_gb} maxGb={data.max_recordings_gb} />

            <Divider />
            <Metric label="Uptime">{formatUptime(data.uptime_seconds)}</Metric>
          </>
        )}

        {/* Clock — pinned right */}
        <div className="ml-auto flex shrink-0 flex-col items-end gap-0.5 leading-none">
          <span className="font-mono text-[9px] uppercase tracking-micro text-fg-muted">
            Local
          </span>
          <span className="font-mono text-[13px] font-semibold tabular-nums text-fg">
            {clock.toLocaleTimeString([], {
              hour: "2-digit",
              minute: "2-digit",
              second: "2-digit",
              hour12: false,
            })}
          </span>
        </div>
      </div>
    </header>
  );
}

/* ---------------------------------------------------------------- */
/* AppShell                                                         */
/* ---------------------------------------------------------------- */

export function AppShell({ children }: { children: ReactNode }) {
  const { data } = usePoll(() => api.system(), 30000);
  return (
    <div className="relative min-h-screen">
      <div className="app-atmosphere" aria-hidden="true" />
      <div className="relative z-10 flex min-h-screen">
        <NavRail version={data?.version} />
        <div className="flex min-h-screen min-w-0 flex-1 flex-col">
          <TelemetryBar />
          <main className="flex-1">{children}</main>
        </div>
      </div>
    </div>
  );
}

export default AppShell;
