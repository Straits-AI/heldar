import { Link, NavLink, Route, Routes } from "react-router-dom";
import { api } from "./lib/api";
import { usePoll } from "./lib/usePoll";
import { SystemBar } from "./components/SystemBar";
import { Dashboard } from "./pages/Dashboard";
import { CameraDetail } from "./pages/CameraDetail";
import { AddCamera } from "./pages/AddCamera";

function NotFound() {
  return (
    <div className="mx-auto max-w-xl px-4 py-20 text-center">
      <h1 className="text-2xl font-semibold text-slate-200">404</h1>
      <p className="mt-2 text-sm text-slate-500">That page does not exist.</p>
      <Link to="/" className="btn mt-4">
        ← Back to cameras
      </Link>
    </div>
  );
}

export default function App() {
  const system = usePoll(() => api.system(), 5000);

  return (
    <div className="flex min-h-full flex-col bg-ink">
      <header className="flex items-center gap-4 border-b border-line bg-panel px-4 py-2.5">
        <Link to="/" className="flex items-center gap-2">
          <span className="flex h-6 w-6 items-center justify-center rounded-md border border-accent/40">
            <span className="h-2 w-2 rounded-full bg-accent" />
          </span>
          <span className="text-sm font-semibold tracking-tight text-slate-100">
            VisionOps <span className="text-accent">Core</span>
          </span>
        </Link>
        <nav className="flex items-center gap-1 text-sm">
          <NavLink
            to="/"
            end
            className={({ isActive }) =>
              `rounded-md px-2.5 py-1 ${isActive ? "bg-panel2 text-white" : "text-slate-400 hover:text-slate-200"}`
            }
          >
            Cameras
          </NavLink>
        </nav>
        <div className="ml-auto">
          <Link to="/cameras/new" className="btn btn-primary btn-sm">
            + Add camera
          </Link>
        </div>
      </header>

      <SystemBar info={system.data} error={system.error} />

      <main className="flex-1">
        <Routes>
          <Route path="/" element={<Dashboard />} />
          <Route path="/cameras/new" element={<AddCamera />} />
          <Route path="/cameras/:id" element={<CameraDetail />} />
          <Route path="*" element={<NotFound />} />
        </Routes>
      </main>
    </div>
  );
}
