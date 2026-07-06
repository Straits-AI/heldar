import { Component, lazy, Suspense } from "react";
import type { ComponentType, ReactNode } from "react";
import { Spinner } from "../components/ui";
import type { ModuleBundle } from "./contract";

/** A module bundle that fails to load (offline, 404, updating) or throws must render a clear fallback,
 *  never a blank frame or a crashed console. */
class ModuleErrorBoundary extends Component<
  { title: string; children: ReactNode },
  { failed: boolean }
> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  render() {
    if (this.state.failed) {
      return (
        <div className="mx-auto max-w-xl px-4 py-24 text-center">
          <h1 className="font-display text-xl font-bold text-fg">Module unavailable</h1>
          <p className="mt-2 text-sm text-fg-secondary">
            The “{this.props.title}” module UI failed to load. It may be updating or offline.
          </p>
        </div>
      );
    }
    return this.props.children;
  }
}

// Dedupe the lazy() per URL so a re-render (or navigating away and back) doesn't re-import the bundle.
const cache = new Map<string, ComponentType>();

/**
 * Mounts a runtime-loaded module UI: dynamically imports the module's ES bundle from `url` and renders
 * its default export. The bundle shares the shell's React via the window bridge + import-map shims (see
 * `main.tsx` / `contract.ts`). Loading shows a spinner; a load/runtime failure shows the error fallback.
 */
export function ModuleHost({ url, title }: { url: string; title: string }) {
  let Comp = cache.get(url);
  if (!Comp) {
    Comp = lazy(() => import(/* @vite-ignore */ url) as Promise<ModuleBundle>);
    cache.set(url, Comp);
  }
  return (
    <ModuleErrorBoundary title={title}>
      <Suspense
        fallback={
          <div className="flex items-center justify-center py-24 text-fg-muted">
            <Spinner size={18} />
          </div>
        }
      >
        <Comp />
      </Suspense>
    </ModuleErrorBoundary>
  );
}
