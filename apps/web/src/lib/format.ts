// Small formatting helpers shared across the dashboard.

import { ApiError } from "./api";

export function formatBytes(bytes: number): string {
  if (!bytes || bytes < 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let n = bytes;
  let i = 0;
  while (n >= 1024 && i < units.length - 1) {
    n /= 1024;
    i += 1;
  }
  const digits = i === 0 || n >= 100 ? 0 : n >= 10 ? 1 : 2;
  return `${n.toFixed(digits)} ${units[i]}`;
}

export function formatDuration(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${sec}s`;
  return `${sec}s`;
}

/** Compact uptime like "3d 4h", "12m". */
export function formatUptime(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m`;
}

export function formatClock(iso?: string | null): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "—";
  return d.toLocaleString();
}

export function formatTimeShort(iso?: string | null): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "—";
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

export function timeAgo(iso?: string | null): string {
  if (!iso) return "never";
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "never";
  const diffMs = Date.now() - then;
  const s = Math.round(diffMs / 1000);
  if (s < 0) return "soon";
  if (s < 60) return `${s}s ago`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  return `${d}d ago`;
}

/** RFC3339/ISO string -> value for an <input type="datetime-local" step="1">. */
export function isoToLocalInput(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const local = new Date(d.getTime() - d.getTimezoneOffset() * 60000);
  return local.toISOString().slice(0, 19);
}

/** datetime-local value (local wall time) -> RFC3339 UTC string, or null if blank/invalid. */
export function localInputToIso(value: string): string | null {
  if (!value) return null;
  const d = new Date(value);
  if (Number.isNaN(d.getTime())) return null;
  return d.toISOString();
}

/** Map an error (usually an ApiError) to operator-readable copy. The raw status/message stays
 *  available on the error object for the console; this is what the UI should show as primary copy.
 *  401 keeps the server's message (the kernel's auth errors are already human, e.g. login). */
export function friendlyError(e: unknown): string {
  if (e instanceof ApiError) {
    switch (e.status) {
      case 502:
      case 504:
        return "The box is unreachable right now — it may be offline. Try again shortly.";
      case 503:
        return "That service is temporarily unavailable — retry shortly.";
      case 429:
        return "Too many attempts — wait a minute and try again.";
      case 403:
        return "You don't have permission for that.";
      case 0:
        return "Network problem — check your connection and retry.";
      default:
        return e.message;
    }
  }
  return e instanceof Error ? e.message : String(e);
}

/**
 * How to label a bare "HH:MM" schedule window with the clock it is actually read in (#125).
 *
 * A recording window is a wall-clock rule evaluated in the camera's site timezone, or — when no zone
 * is configured anywhere — the server's own clock. Rendering "18:00" unlabelled is how an operator
 * in Kuala Lumpur comes to believe they scheduled 6pm local on a box running UTC, and the recorder
 * is then eight hours out every day with nothing on screen to say so.
 *
 * Returns `null` only while the setting is still loading, so the caller renders the panel unchanged
 * rather than flashing a wrong label.
 */
export function scheduleClockLabel(
  tz: { configured: string | null; server_local_offset: string } | null | undefined,
  /** The camera's own site zone, when it has one — it OVERRIDES the box-wide setting. */
  siteZone?: string | null,
): string | null {
  if (!tz) return null;
  // Resolution order must match `services/tz.rs`: the camera's site, then the box-wide setting,
  // then the server's own clock. The first version of this took only the box-wide value, so a
  // camera on a site with a different zone was labelled with a clock its recorder does not use —
  // an 8-hour lie stated with authority, which is WORSE than the unlabelled window it replaced.
  // An unlabelled "18:00" sends someone to go and check; "18:00 — UTC" invites them to "fix" a
  // schedule that was already correct.
  if (siteZone) return siteZone;
  // Not "UTC": say WHOSE clock it is. "the server's clock" is the honest description, and it is
  // the thing an operator can go and check.
  return tz.configured ?? `server clock (${tz.server_local_offset})`;
}

/**
 * The zone a camera's timestamps should be RENDERED in, and where that zone came from (#148).
 *
 * `scheduleClockLabel` above answers "what do we call the clock"; this answers "what do we format
 * in", and the two are not the same question. That function can return `server clock (+08:00)`,
 * which is a true and useful sentence and NOT something `Intl.DateTimeFormat` will accept as a
 * `timeZone` — offset strings are not IANA identifiers. Passing one through would throw a
 * RangeError inside a render.
 *
 * So when no IANA zone is known, this resolves to the VIEWER's zone and says so. Rendering a
 * Malaysian camera's footage in London time is defensible; rendering it in London time while
 * claiming it is the site's clock is not, and an unlabelled clock is what #148 exists to fix.
 */
export type ResolvedZone =
  /** The camera's own site names a zone. It overrides the box-wide setting, as in `services/tz.rs`. */
  | { kind: "site"; zone: string }
  /** No site zone; the box has one configured. */
  | { kind: "box"; zone: string }
  /**
   * Neither names an IANA zone, so there is nothing to convert INTO. The box reads schedules in the
   * server's own clock, of which the API gives us only an offset — not enough for `Intl`.
   */
  | { kind: "viewer"; zone: string };

export function resolveCameraZone(
  tz:
    | { configured: string | null; server_local_offset: string }
    | null
    | undefined,
  siteZone?: string | null,
): ResolvedZone {
  // Same order as `services/tz.rs`: the camera's site, then the box-wide setting. A camera on a site
  // with its own zone must not be rendered in the box's — that is the 8-hour lie #147 fixed for
  // schedule labels, reached here through a different door.
  if (siteZone) return { kind: "site", zone: siteZone };
  if (tz?.configured) return { kind: "box", zone: tz.configured };
  return { kind: "viewer", zone: viewerZone() };
}

/** The browser's IANA zone, or `UTC` where the runtime will not say. */
export function viewerZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
}

/** What to put next to a converted clock. Short, because it sits in a panel header. */
export function zoneLabel(r: ResolvedZone): string {
  return r.kind === "viewer" ? `${r.zone} (your zone)` : r.zone;
}

/**
 * Render an instant in a named zone.
 *
 * Falls back to the viewer's zone rather than throwing: an unknown or malformed IANA name reaches
 * this from a site row an operator typed, and one bad row must not blank out every timestamp on the
 * page. The caller's LABEL is derived from `ResolvedZone`, so a fallback here would silently
 * mislabel — which is why `isRenderableZone` exists and callers resolve the label from the same
 * value they format with.
 */
export function formatClockIn(
  iso: string | null | undefined,
  zone: string,
): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "—";
  try {
    return d.toLocaleString([], { timeZone: zone });
  } catch {
    return d.toLocaleString();
  }
}

export function formatTimeShortIn(
  iso: string | null | undefined,
  zone: string,
): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "—";
  try {
    return d.toLocaleTimeString([], {
      timeZone: zone,
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
  } catch {
    return d.toLocaleTimeString([], {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
  }
}

/** Whether `Intl` will accept this as a `timeZone`. A site row carries operator-typed text. */
export function isRenderableZone(zone: string): boolean {
  try {
    new Intl.DateTimeFormat([], { timeZone: zone });
    return true;
  } catch {
    return false;
  }
}

/** The zone a camera's schedule is actually read in, or `null` when its site names none. */
export function cameraSiteZone(
  siteId: string | null | undefined,
  sites: { id: string; timezone: string | null }[] | undefined,
): string | null {
  if (!siteId || !sites) return null;
  return sites.find((s) => s.id === siteId)?.timezone ?? null;
}
