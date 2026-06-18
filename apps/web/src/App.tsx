import { lazy, Suspense } from "react";
import { Link, Route, Routes } from "react-router-dom";
import { AppShell } from "./components/AppShell";
import { Button, Spinner } from "./components/ui";
import { MODULE_PAGES, ModuleFrame, ModulesProvider, useModules } from "./modules";

// Route-level code-splitting: each page is its own chunk, loaded on demand instead of
// being bundled into the entry chunk. Pages are mapped via their named export because
// several (Dashboard, CameraDetail, AddCamera, Discover) have no default export, and
// React.lazy requires a module whose `default` is the component.
const Dashboard = lazy(() => import("./pages/Dashboard").then((m) => ({ default: m.Dashboard })));
const CameraDetail = lazy(() =>
  import("./pages/CameraDetail").then((m) => ({ default: m.CameraDetail })),
);
const AddCamera = lazy(() => import("./pages/AddCamera").then((m) => ({ default: m.AddCamera })));
const Discover = lazy(() => import("./pages/Discover").then((m) => ({ default: m.Discover })));
const System = lazy(() => import("./pages/System").then((m) => ({ default: m.System })));
const Ai = lazy(() => import("./pages/Ai").then((m) => ({ default: m.Ai })));
const Backup = lazy(() => import("./pages/Backup").then((m) => ({ default: m.Backup })));
const Incidents = lazy(() => import("./pages/Incidents").then((m) => ({ default: m.Incidents })));
const Plugins = lazy(() => import("./pages/Plugins").then((m) => ({ default: m.Plugins })));

function NotFound() {
  return (
    <div className="mx-auto max-w-xl px-4 py-24 text-center">
      <div className="font-mono text-5xl font-semibold tracking-tight text-fg-muted">404</div>
      <h1 className="mt-4 font-display text-xl font-bold text-fg">Signal lost</h1>
      <p className="mt-2 text-sm text-fg-secondary">That route does not exist on this console.</p>
      <Link to="/" className="mt-6 inline-block">
        <Button variant="primary">Return to Wall</Button>
      </Link>
    </div>
  );
}

function RouteLoading() {
  return (
    <div className="flex items-center justify-center py-24 text-fg-muted">
      <Spinner size={18} />
    </div>
  );
}

/** Platform routes are static; module routes come from the loaded manifests (only loaded modules
 *  with a bundled page are routed). While modules are still loading, an unmatched path shows a spinner
 *  rather than flashing 404 on a module deep-link. */
function AppRoutes() {
  const { modules, loading } = useModules();
  return (
    <Suspense fallback={<RouteLoading />}>
      <Routes>
        {/* Platform — the kernel console, always present */}
        <Route path="/" element={<Dashboard />} />
        <Route path="/cameras/new" element={<AddCamera />} />
        <Route path="/discover" element={<Discover />} />
        <Route path="/ai" element={<Ai />} />
        <Route path="/incidents" element={<Incidents />} />
        <Route path="/backup" element={<Backup />} />
        <Route path="/plugins" element={<Plugins />} />
        <Route path="/system" element={<System />} />
        <Route path="/cameras/:id" element={<CameraDetail />} />

        {/* Modules — dynamic from GET /api/v1/modules. Compiled modules render their bundled page;
            imported sidecars (mount=iframe) render a micro-frontend proxied at /m/{id}/. */}
        {modules.flatMap((m) =>
          m.nav.map((n) => {
            const Page = MODULE_PAGES[m.id];
            const element = Page ? (
              <Page />
            ) : m.mount === "iframe" ? (
              <ModuleFrame id={m.id} title={m.name} />
            ) : null;
            return element ? <Route key={n.path} path={n.path} element={element} /> : null;
          }),
        )}

        <Route path="*" element={loading ? <RouteLoading /> : <NotFound />} />
      </Routes>
    </Suspense>
  );
}

export default function App() {
  return (
    <ModulesProvider>
      <AppShell>
        <AppRoutes />
      </AppShell>
    </ModulesProvider>
  );
}
