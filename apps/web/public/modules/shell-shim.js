/**
 * Shell SDK shim for dynamically-loaded module bundles.
 *
 * A module bundle imports the shell surface as `import { api, Button } from "@heldar/shell"`. The
 * shell's import map points `@heldar/shell` → this file, which re-exports the shell's single instances
 * that `main.tsx` published on `window.__HELDAR_SHELL__` (from src/sdk.ts) before any module loaded. So
 * modules share the shell's API client, auth/session, and design system — they never bundle a copy.
 *
 * Keep the enumerated names below in sync with the RUNTIME exports of src/sdk.ts (types are erased and
 * are not listed here). This shim is served as a static asset; it is NOT bundled by Vite.
 */

const S = window.__HELDAR_SHELL__;
export const {
  // API client + auth
  api,
  ApiError,
  setAuthToken,
  // data-fetching hook
  usePoll,
  // design system (ui kit)
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
  // shared auth surface
  Login,
  // formatting helpers
  formatBytes,
  formatDuration,
  formatUptime,
  formatClock,
  formatTimeShort,
  timeAgo,
  isoToLocalInput,
  localInputToIso,
} = S;

export default S;
