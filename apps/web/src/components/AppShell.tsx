// Heldar Core — application shell: left nav rail + top telemetry status bar.
// The telemetry bar polls GET /api/v1/system (~5s) and renders live operations data.

import { useEffect, useState } from "react";
import type { ReactNode } from "react";
import { NavLink } from "react-router-dom";
import { api } from "../lib/api";
import { usePoll } from "../lib/usePoll";
import { formatBytes, formatUptime } from "../lib/format";
import { moduleIcon, useModules } from "../modules";
import { BrandMark, cx, Spinner, StatusLed } from "./ui";

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

function PluginsIcon({ className }: IconProps) {
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
      <path d="M7 3v2.5M13 3v2.5" />
      <path d="M5 5.5h10v4a5 5 0 0 1-10 0z" />
      <path d="M10 14.5V17" />
    </svg>
  );
}

function PlaybackIcon({ className }: IconProps) {
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
      <rect x="2.5" y="4" width="15" height="12" rx="1.5" />
      <path d="M8.5 8l3.5 2-3.5 2z" fill="currentColor" />
    </svg>
  );
}

// Platform chrome — the kernel console pages, always present. Domain modules are NOT listed here;
// they come from GET /api/v1/modules (see the Modules section below), so only loaded modules appear.
const NAV_ITEMS: { to: string; label: string; end?: boolean; Icon: (p: IconProps) => ReactNode }[] = [
  { to: "/", label: "Wall", end: true, Icon: WallIcon },
  { to: "/playback", label: "Playback", Icon: PlaybackIcon },
  { to: "/discover", label: "Discover", Icon: DiscoverIcon },
  { to: "/cameras/new", label: "Add Camera", Icon: AddCameraIcon },
  { to: "/ai", label: "AI", Icon: AiIcon },
  { to: "/incidents", label: "Incidents", Icon: IncidentsIcon },
  { to: "/backup", label: "Backup", Icon: BackupIcon },
  { to: "/plugins", label: "Plugins", Icon: PluginsIcon },
  { to: "/system", label: "System", Icon: SystemIcon },
];

/** A single nav destination, shared by the Operations + Modules sections. */
function NavRow({
  to,
  label,
  end,
  Icon,
  onClick,
}: {
  to: string;
  label: string;
  end?: boolean;
  Icon: (p: IconProps) => ReactNode;
  onClick?: () => void;
}) {
  return (
    <NavLink
      to={to}
      end={end}
      onClick={onClick}
      className={({ isActive }) =>
        cx(
          "group relative flex items-center gap-3 rounded-md px-3 py-2 text-sm font-medium transition-[background-color,color] duration-150",
          isActive
            ? "bg-raised text-fg shadow-[inset_0_1px_0_rgba(255,255,255,0.04)]"
            : "text-fg-secondary hover:bg-raised/60 hover:text-fg",
        )
      }
    >
      {({ isActive }) => (
        <>
          <span
            className={cx(
              "absolute left-0 top-1/2 w-[3px] -translate-y-1/2 rounded-r-full bg-accent transition-all duration-200",
              isActive
                ? "h-5 opacity-100 shadow-[0_0_8px_0_rgba(245,158,11,0.6)]"
                : "h-3 opacity-0 group-hover:opacity-40",
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
  );
}

function SectionLabel({ children }: { children: ReactNode }) {
  return (
    <div className="px-2 pb-2 font-mono text-[9px] uppercase tracking-micro text-fg-muted">
      {children}
    </div>
  );
}

/** The Operations + dynamic Modules nav sections, shared by the desktop rail and the mobile menu.
 *  `onNavigate` fires when a destination is tapped (the mobile menu closes itself on navigation). */
function NavSections({ onNavigate }: { onNavigate?: () => void }) {
  // Loaded modules drive the dynamic Modules section — only what this Core binary links is shown.
  const { modules } = useModules();
  return (
    <nav className="stagger flex flex-1 flex-col gap-0.5 overflow-y-auto px-3 py-4">
      <SectionLabel>Operations</SectionLabel>
      {NAV_ITEMS.map(({ to, label, end, Icon }) => (
        <NavRow key={to} to={to} label={label} end={end} Icon={Icon} onClick={onNavigate} />
      ))}

      {/* Modules — dynamic from GET /api/v1/modules; renders only when at least one is loaded. */}
      {modules.length > 0 && (
        <>
          <SectionLabel>
            <span className="mt-4 block">Modules</span>
          </SectionLabel>
          {modules.flatMap((m) =>
            m.nav.map((n) => (
              <NavRow
                key={n.path}
                to={n.path}
                label={n.label}
                Icon={moduleIcon(n.icon)}
                onClick={onNavigate}
              />
            )),
          )}
        </>
      )}
    </nav>
  );
}

function BrandHeader() {
  return (
    <div className="relative flex animate-fade-in items-center gap-3 px-5 py-5">
      <span className="relative flex h-10 w-10 items-center justify-center rounded-lg border border-accent/35 bg-canvas shadow-[inset_0_1px_0_rgba(255,255,255,0.05),0_0_18px_-6px_rgba(245,158,11,0.5)]">
        <span className="pointer-events-none absolute inset-0 rounded-lg bg-bifrost-soft opacity-50" />
        <BrandMark size={24} className="relative" />
      </span>
      <div className="leading-none">
        <div className="font-display text-[15px] font-extrabold tracking-wider text-fg">HELDAR</div>
        <div className="mt-1.5 font-mono text-[9px] uppercase tracking-micro text-accent">Core</div>
      </div>
      <span aria-hidden="true" className="absolute inset-x-0 bottom-0 h-px bg-bifrost-line opacity-70" />
    </div>
  );
}

function BuildFooter({ version }: { version?: string }) {
  return (
    <div className="border-t border-line px-5 py-4">
      <div className="font-mono text-[9px] uppercase tracking-micro text-fg-muted">Build</div>
      <div className="mt-1 font-mono text-[11px] text-fg-secondary">{version ? `v${version}` : "—"}</div>
    </div>
  );
}

/** Desktop nav rail (hidden below sm; the mobile menu covers that breakpoint). */
function NavRail({ version }: { version?: string }) {
  return (
    <aside className="sticky top-0 z-30 hidden h-screen w-[232px] shrink-0 flex-col border-r border-line bg-panel sm:flex">
      <BrandHeader />
      <NavSections />
      <BuildFooter version={version} />
    </aside>
  );
}

/** Mobile slide-over nav (below sm), opened by the telemetry-bar hamburger. */
function MobileNav({ open, onClose, version }: { open: boolean; onClose: () => void; version?: string }) {
  useEffect(() => {
    if (!open) return;
    const prev = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    document.addEventListener("keydown", onKey);
    return () => {
      document.body.style.overflow = prev;
      document.removeEventListener("keydown", onKey);
    };
  }, [open, onClose]);
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50 flex sm:hidden">
      <div className="absolute inset-0 animate-fade-in bg-black/60 backdrop-blur-sm" onClick={onClose} aria-hidden="true" />
      <aside
        role="dialog"
        aria-modal="true"
        aria-label="Navigation"
        className="relative flex h-full w-[232px] flex-col border-r border-line bg-panel animate-fade-in"
      >
        <BrandHeader />
        <NavSections onNavigate={onClose} />
        <BuildFooter version={version} />
      </aside>
    </div>
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

function TelemetryBar({ onMenu }: { onMenu: () => void }) {
  const { data, error } = usePoll(() => api.system(), 5000);
  const clock = useClock();
  const recording = data?.cameras_recording ?? 0;
  const online = !!data && !error;

  return (
    <header className="sticky top-0 z-20 animate-fade-in border-b border-line bg-panel/85 shadow-[inset_0_1px_0_rgba(255,255,255,0.03)] backdrop-blur supports-[backdrop-filter]:bg-panel/70">
      <div className="flex h-14 items-center gap-5 overflow-x-auto px-4 sm:px-6">
        {/* Mobile menu toggle (the nav rail is hidden below sm) */}
        <button
          onClick={onMenu}
          aria-label="Open navigation"
          className="-ml-1 shrink-0 rounded-md p-1.5 text-fg-secondary hover:bg-raised hover:text-fg sm:hidden"
        >
          <svg viewBox="0 0 20 20" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round">
            <path d="M3 6h14M3 10h14M3 14h14" />
          </svg>
        </button>
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
  const [menuOpen, setMenuOpen] = useState(false);
  return (
    <div className="relative min-h-screen">
      <div className="app-atmosphere" aria-hidden="true" />
      <div className="relative z-10 flex min-h-screen">
        <NavRail version={data?.version} />
        <MobileNav open={menuOpen} onClose={() => setMenuOpen(false)} version={data?.version} />
        <div className="flex min-h-screen min-w-0 flex-1 flex-col">
          <TelemetryBar onMenu={() => setMenuOpen(true)} />
          <main className="flex-1">{children}</main>
        </div>
      </div>
    </div>
  );
}

export default AppShell;
