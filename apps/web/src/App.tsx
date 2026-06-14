import { Link, Route, Routes } from "react-router-dom";
import { AppShell } from "./components/AppShell";
import { Button } from "./components/ui";
import { Dashboard } from "./pages/Dashboard";
import { CameraDetail } from "./pages/CameraDetail";
import { AddCamera } from "./pages/AddCamera";
import { Discover } from "./pages/Discover";
import { System } from "./pages/System";
import { Ai } from "./pages/Ai";
import { Entry } from "./pages/Entry";
import { Movement } from "./pages/Movement";
import { Search } from "./pages/Search";

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

export default function App() {
  return (
    <AppShell>
      <Routes>
        <Route path="/" element={<Dashboard />} />
        <Route path="/cameras/new" element={<AddCamera />} />
        <Route path="/discover" element={<Discover />} />
        <Route path="/ai" element={<Ai />} />
        <Route path="/entry" element={<Entry />} />
        <Route path="/movement" element={<Movement />} />
        <Route path="/search" element={<Search />} />
        <Route path="/system" element={<System />} />
        <Route path="/cameras/:id" element={<CameraDetail />} />
        <Route path="*" element={<NotFound />} />
      </Routes>
    </AppShell>
  );
}
