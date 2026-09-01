// Module registry + live module context.
//
// Phase A of the plugin platform: the dashboard renders its module nav + routes from
// GET /api/v1/modules (the manifests the Core binary links) instead of a hardcoded list, so only
// LOADED modules appear. Module page components are NOT compiled into the shell — every module UI is
// loaded at runtime (each crate serves its own bundle at /api/v1/modules/{id}/ui, mounted by
// `ModuleHost`; sidecars are iframe-mounted via `ModuleFrame`). This file holds only the nav glyphs
// keyed by the manifest `icon` field; unknown keys fall back to GenericModuleIcon.

import { createContext, useContext, useEffect, useRef } from "react";
import type { ReactNode } from "react";
import { api } from "./lib/api";
import { usePoll } from "./lib/usePoll";
import type { ModuleManifest } from "./lib/types";
// All module pages are runtime-loaded — each crate serves its own bundle at
// /api/v1/modules/{id}/ui, mounted by `ModuleHost` from the manifest `ui_url`. Nav glyphs
// (MODULE_ICONS below) stay in the shell for the rail; unknown keys fall back to a generic glyph.

type IconProps = { className?: string };

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

/** Fallback glyph for modules with no bundled icon (e.g. third-party/imported plugins). */
function GenericModuleIcon({ className }: IconProps) {
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
      <rect x="3" y="3" width="6" height="6" rx="1.2" />
      <rect x="11" y="3" width="6" height="6" rx="1.2" />
      <rect x="3" y="11" width="6" height="6" rx="1.2" />
      <path d="M11 14h6M14 11v6" />
    </svg>
  );
}

// Bespoke nav glyphs for the built-in OPEN modules, keyed by the manifest `icon`. A PROPRIETARY module's
// glyph is intentionally NOT listed here: its name must not appear in the shell source, or the open-repo
// generator's fail-closed gate (which greps apps/web/src for proprietary identifiers) would flag it. Such
// modules fall back to the generic glyph today — same as an imported third-party plugin. Future: a
// module could ship its own bespoke nav glyph via a manifest-carried inline SVG, so proprietary/
// third-party plugins aren't limited to the generic fallback.
const MODULE_ICONS: Record<string, (p: IconProps) => ReactNode> = {
  entry: EntryIcon,
  movement: MovementIcon,
  search: SearchIcon,
};

/** Resolve a manifest nav `icon` key to a glyph; unknown/proprietary keys get the generic module glyph. */
export function moduleIcon(key: string): (p: IconProps) => ReactNode {
  return MODULE_ICONS[key] ?? GenericModuleIcon;
}

/* ---------------------------------------------------------------- */
/* Sidecar plugin host (ISOLATED)                                   */
/* ---------------------------------------------------------------- */

/** How long the host waits for a proxied plugin request before giving up. */
const BRIDGE_TIMEOUT_MS = 20_000;

/** Methods a plugin may ask the host to perform on its behalf. */
const BRIDGE_METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE"] as const;

/** A request from a sandboxed plugin, asking the host to call the plugin's OWN proxy path. */
interface BridgeRequest {
  heldar: "request";
  /** Correlates the reply; echoed back untouched. */
  id: string;
  method: string;
  /** Path RELATIVE to the plugin's own `/m/{id}/` root. Never an absolute URL. */
  path: string;
  body?: string;
  headers?: Record<string, string>;
}

/**
 * Resolve a plugin-supplied path against its own proxy root, or reject it.
 *
 * This is the containment boundary. The plugin names a path relative to `/m/{id}/`, and anything that
 * resolves outside that prefix — `../`, a leading `/`, an absolute URL, a protocol-relative `//host` —
 * is refused. Without it the bridge would be a confused deputy: the plugin would be asking the HOST,
 * which holds the operator's session, to call arbitrary kernel endpoints on its behalf, which is
 * exactly the privilege the sandbox just took away.
 */
export function resolveBridgePath(moduleId: string, path: string): string | null {
  const root = `/m/${encodeURIComponent(moduleId)}/`;
  // A base is required to resolve relatives; the origin is discarded below, only the pathname is used.
  let url: URL;
  try {
    url = new URL(path, `${window.location.origin}${root}`);
  } catch {
    return null;
  }
  // An absolute or protocol-relative URL resolves to a different origin — refuse rather than follow.
  if (url.origin !== window.location.origin) return null;
  if (!url.pathname.startsWith(root)) return null;
  return url.pathname + url.search;
}

/**
 * Micro-frontend mount for an imported sidecar plugin.
 *
 * ISOLATED BY CONSTRUCTION: the sandbox deliberately omits `allow-same-origin`, so the frame gets an
 * OPAQUE origin. It therefore cannot touch the parent DOM, cannot read storage, and — the part that
 * matters — its own requests carry no session cookie, because a sandboxed frame has a null
 * site-for-cookies. A hostile or compromised plugin can no longer act with the operator's authority
 * simply by virtue of being served from the console's origin.
 *
 * That isolation costs the plugin its ability to call anything directly, so the host mediates: a
 * plugin posts a request, the host validates it is confined to that plugin's OWN `/m/{id}/` proxy
 * path, performs it with the session, and posts the result back. The plugin's reach is therefore
 * exactly its own sidecar — not the kernel API.
 *
 * Message identity is checked by SOURCE, not origin: an opaque-origin frame posts with origin
 * `"null"`, which is unforgeable-by-string but also unusable as an allowlist, so the host compares
 * `event.source` against this frame's `contentWindow`.
 */
export function ModuleFrame({ id, title }: { id: string; title: string }) {
  const frameRef = useRef<HTMLIFrameElement | null>(null);

  useEffect(() => {
    async function onMessage(event: MessageEvent) {
      const frame = frameRef.current;
      // Only this frame. `event.origin` is "null" for a sandboxed frame, so identity comes from the
      // window reference — any other sender (another frame, the opener, an extension) is ignored.
      if (!frame || event.source !== frame.contentWindow) return;

      const msg = event.data as BridgeRequest | undefined;
      if (!msg || msg.heldar !== "request" || typeof msg.id !== "string") return;

      const reply = (payload: Record<string, unknown>) =>
        // targetOrigin "*" because an opaque origin cannot be named. Safe here: the message goes to
        // one specific window we already identified, and carries only that window's own response.
        frame.contentWindow?.postMessage({ heldar: "response", id: msg.id, ...payload }, "*");

      const method = String(msg.method ?? "GET").toUpperCase();
      if (!(BRIDGE_METHODS as readonly string[]).includes(method)) {
        reply({ ok: false, error: `method ${method} is not permitted` });
        return;
      }
      const path = resolveBridgePath(id, String(msg.path ?? ""));
      if (!path) {
        reply({ ok: false, error: "path escapes this plugin's proxy root" });
        return;
      }

      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(), BRIDGE_TIMEOUT_MS);
      try {
        const res = await fetch(path, {
          method,
          // Only content-type is forwarded. Letting a plugin set arbitrary headers would let it forge
          // authorization or identity headers on a request the host makes with the operator's session.
          headers: msg.headers?.["content-type"]
            ? { "content-type": msg.headers["content-type"] }
            : undefined,
          body: method === "GET" || method === "DELETE" ? undefined : msg.body,
          credentials: "include",
          signal: controller.signal,
        });
        reply({ ok: true, status: res.status, body: await res.text() });
      } catch (e) {
        reply({ ok: false, error: e instanceof Error ? e.message : String(e) });
      } finally {
        clearTimeout(timer);
      }
    }

    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, [id]);

  return (
    <iframe
      ref={frameRef}
      src={`/m/${encodeURIComponent(id)}/`}
      title={title}
      className="h-[calc(100vh-3.5rem)] w-full border-0 bg-canvas"
      // No `allow-same-origin`: the frame runs in an OPAQUE origin, so it cannot reach the parent DOM
      // or send the console's session cookie. `allow-popups` is gone too — a plugin does not need to
      // open windows, and a popup is an escape from the sandbox's containment.
      sandbox="allow-scripts allow-forms"
    />
  );
}

/* ---------------------------------------------------------------- */
/* Live module context                                              */
/* ---------------------------------------------------------------- */

interface ModulesState {
  modules: ModuleManifest[];
  loading: boolean;
  error: string | null;
}

const ModulesContext = createContext<ModulesState>({
  modules: [],
  loading: true,
  error: null,
});

/** Loaded modules from GET /api/v1/modules, shared by the nav rail and the router. */
export function useModules(): ModulesState {
  return useContext(ModulesContext);
}

/**
 * Fetches the loaded modules once (then re-polls every 30s so an install/uninstall in a later phase
 * reflects without a reload) and provides them to the shell + routes.
 */
export function ModulesProvider({ children }: { children: ReactNode }) {
  const { data, loading, error } = usePoll(() => api.modules(), 30000);
  return (
    <ModulesContext.Provider value={{ modules: data ?? [], loading, error }}>
      {children}
    </ModulesContext.Provider>
  );
}
