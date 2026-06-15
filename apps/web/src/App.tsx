import { Link, Route, Routes } from "react-router-dom";
import { AppShell } from "./components/AppShell";
import { Button, Spinner } from "./components/ui";
import { MODULE_PAGES, ModulesProvider, useModules } from "./modules";
import { Dashboard } from "./pages/Dashboard";
import { CameraDetail } from "./pages/CameraDetail";
import { AddCamera } from "./pages/AddCamera";
import { Discover } from "./pages/Discover";
import { System } from "./pages/System";
import { Ai } from "./pages/Ai";
import { Backup } from "./pages/Backup";
import { Incidents } from "./pages/Incidents";

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
    <Routes>
      {/* Platform — the kernel console, always present */}
      <Route path="/" element={<Dashboard />} />
      <Route path="/cameras/new" element={<AddCamera />} />
      <Route path="/discover" element={<Discover />} />
      <Route path="/ai" element={<Ai />} />
      <Route path="/incidents" element={<Incidents />} />
      <Route path="/backup" element={<Backup />} />
      <Route path="/system" element={<System />} />
      <Route path="/cameras/:id" element={<CameraDetail />} />

      {/* Modules — dynamic from GET /api/v1/modules */}
      {modules.flatMap((m) =>
        m.nav.map((n) => {
          const Page = MODULE_PAGES[m.id];
          return Page ? <Route key={n.path} path={n.path} element={<Page />} /> : null;
        }),
      )}

      <Route path="*" element={loading ? <RouteLoading /> : <NotFound />} />
    </Routes>
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
