// The Heldar module SDK — the shell surface a runtime-loaded module UI imports as `@heldar/shell`.
//
// A module bundle does NOT bundle its own copy of this code. At runtime its `@heldar/shell` import
// resolves (via the shell's import map → `public/modules/shell-shim.js`) to the SHELL's single
// instances that `main.tsx` publishes on `window.__HELDAR_SHELL__`. So modules share the shell's API
// client, auth/session, and design system — no duplication, one source of truth.
//
// This file IS that single source of truth for the SDK surface. Keep it in sync with
// `public/modules/shell-shim.js` (the shim enumerates the same RUNTIME names). Types below are erased
// at build time and exist only so a module's `tsc` can typecheck its `@heldar/shell` imports (the
// module tsconfig maps `@heldar/shell` → this file).

// --- API client + auth ---
export { api, ApiError, setAuthToken } from "./lib/api";

// --- data-fetching hook ---
export { usePoll } from "./lib/usePoll";

// --- design system (the ui kit) ---
export {
  cx,
  BrandMark,
  Panel,
  Button,
  Input,
  Textarea,
  Select,
  Field,
  StatusLed,
  StatusPill,
  Stat,
  Spinner,
  EmptyState,
  SectionLabel,
  Drawer,
} from "./components/ui";

// --- shared auth surface ---
export { Login } from "./components/Login";

// --- formatting helpers ---
export {
  formatBytes,
  formatDuration,
  formatUptime,
  formatClock,
  formatTimeShort,
  timeAgo,
  isoToLocalInput,
  localInputToIso,
} from "./lib/format";

// --- types (erased at build; for module typechecking) ---
export type { PollState } from "./lib/usePoll";
export type { CameraState } from "./components/ui";
export type * from "./lib/types";
