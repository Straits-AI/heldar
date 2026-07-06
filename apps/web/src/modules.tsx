// Module registry + live module context.
//
// Phase A of the plugin platform: the dashboard renders its module nav + routes from
// GET /api/v1/modules (the manifests the Core binary links) instead of a hardcoded list, so only
// LOADED modules appear. Module page components are NOT compiled into the shell — every module UI is
// loaded at runtime (each crate serves its own bundle at /api/v1/modules/{id}/ui, mounted by
// `ModuleHost`; sidecars are iframe-mounted via `ModuleFrame`). This file holds only the nav glyphs
// keyed by the manifest `icon` field; unknown keys fall back to GenericModuleIcon.

import { createContext, useContext } from "react";
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

// Bespoke nav glyphs for the built-in OPEN modules, keyed by the manifest `icon`. A proprietary module
// (e.g. bakery) is intentionally NOT listed here: its glyph must not live in the shell, or the string
// would leak into the OPEN dashboard source (the open-repo gate greps apps/web/src for it). Such modules
// fall back to the generic glyph today — same as an imported third-party plugin. TODO: let a module ship
// its own bespoke nav glyph via a manifest-carried inline SVG so bakery/third-party plugins aren't
// limited to the generic fallback.
const MODULE_ICONS: Record<string, (p: IconProps) => ReactNode> = {
  entry: EntryIcon,
  movement: MovementIcon,
  search: SearchIcon,
};

/** Resolve a manifest nav `icon` key to a glyph; unknown/proprietary keys get the generic module glyph. */
export function moduleIcon(key: string): (p: IconProps) => ReactNode {
  return MODULE_ICONS[key] ?? GenericModuleIcon;
}

/**
 * Micro-frontend mount for an imported sidecar plugin: a full-bleed iframe to `/m/{id}/`, which the
 * kernel reverse-proxies to the sidecar's own UI (single-origin with the console). The plugin's UI
 * calls back to `/m/{id}/api/...` (same proxy) and to the kernel API with its minted key.
 */
export function ModuleFrame({ id, title }: { id: string; title: string }) {
  return (
    <iframe
      src={`/m/${encodeURIComponent(id)}/`}
      title={title}
      className="h-[calc(100vh-3.5rem)] w-full border-0 bg-canvas"
      // Sandboxed: the plugin runs scripts + same-origin (so its own cookies/storage work) but cannot
      // navigate the top frame or trigger downloads on the console's behalf.
      sandbox="allow-scripts allow-same-origin allow-forms allow-popups"
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
