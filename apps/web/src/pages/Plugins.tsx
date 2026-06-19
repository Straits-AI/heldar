// Heldar Core — Plugin Store (Phase C of the plugin platform).
//
// Browses the merged registry catalog (GET /api/v1/registry): the bundled first-party catalog plus
// any signature-verified remote registries, cross-referenced against what is actually loaded/installed.
// Four shelves — Core, Proprietary, Community, Import (bring-your-own). Installing a sidecar entry funnels
// through the existing Phase B register flow (the catalog entry pre-fills the form); the kernel mints a
// scoped key + signed webhook + reverse-proxy mount and returns the credentials ONCE. Compiled modules
// are shown as "Included" / "Contact" (build-time, not runtime-installable). Verification is server-side;
// the UI only renders the `verified` boolean — a forged catalog can never paint a fake VERIFIED badge.

import { useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { api, ApiError } from "../lib/api";
import { usePoll } from "../lib/usePoll";
import type {
  EntryState,
  ModuleRegisterRequest,
  ModuleRegistered,
  Principal,
  RegistryEntry,
  RegistrySource,
  Shelf,
} from "../lib/types";
import { moduleIcon } from "../modules";
import {
  Button,
  Drawer,
  EmptyState,
  Field,
  Input,
  Panel,
  SectionLabel,
  Select,
  Spinner,
  StatusPill,
  cx,
} from "../components/ui";

function errMsg(e: unknown): string {
  return e instanceof ApiError || e instanceof Error ? e.message : String(e);
}

const KIND_META: Record<string, { label: string; cls: string }> = {
  core: { label: "Core", cls: "border-accent/40 bg-accent/10 text-accent" },
  proprietary: { label: "Proprietary", cls: "border-violet-400/40 bg-violet-400/10 text-violet-300" },
  community: { label: "Community", cls: "border-teal-400/40 bg-teal-400/10 text-teal-300" },
  imported: { label: "Self-made", cls: "border-sky-400/40 bg-sky-400/10 text-sky-300" },
};

const SHELVES: { key: Shelf; label: string; blurb: string }[] = [
  { key: "core", label: "Core", blurb: "First-party open modules." },
  { key: "proprietary", label: "Proprietary", blurb: "First-party commercial add-ons." },
  { key: "community", label: "Community", blurb: "Third-party plugins from the registry." },
  { key: "import", label: "Import", blurb: "Run your own sidecar plugin." },
];

function KindBadge({ kind }: { kind: string }) {
  const m = KIND_META[kind] ?? KIND_META.community;
  return (
    <span
      className={cx(
        "rounded-md border px-2 py-0.5 font-mono text-[10px] font-semibold uppercase tracking-micro",
        m.cls,
      )}
    >
      {m.label}
    </span>
  );
}

function VerifiedBadge() {
  return (
    <span
      className="inline-flex items-center gap-1 rounded-md border border-emerald-400/40 bg-emerald-400/10 px-1.5 py-0.5 font-mono text-[10px] font-semibold uppercase tracking-micro text-emerald-300"
      title="This catalog listing was signed by a pinned publisher key"
    >
      <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" strokeWidth="1.8">
        <path d="M8 1.5l5 2v3.5c0 3-2.1 5.4-5 6.5-2.9-1.1-5-3.5-5-6.5V3.5z" />
        <path d="M5.8 8l1.6 1.6L10.4 6.6" strokeLinecap="round" strokeLinejoin="round" />
      </svg>
      Verified
    </span>
  );
}

const STATE_LABEL: Record<EntryState, string> = {
  available: "Available",
  installed: "Installed",
  included: "Included",
  not_in_build: "Not in build",
  unreachable: "Unreachable",
  loaded: "Loaded",
};

function StatePill({ state }: { state: EntryState }) {
  const tone =
    state === "installed" || state === "included" || state === "loaded"
      ? "recording"
      : state === "unreachable"
        ? "error"
        : state === "not_in_build"
          ? "disabled"
          : "connecting";
  return <StatusPill state={tone} label={STATE_LABEL[state]} />;
}

/* ---------------------------------------------------------------- */
/* Copyable credential row                                          */
/* ---------------------------------------------------------------- */

function CopyRow({ label, value }: { label: string; value: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <Field label={label}>
      <div className="flex items-center gap-2">
        <code className="min-w-0 flex-1 truncate rounded-md border border-line bg-canvas px-2.5 py-1.5 font-mono text-xs text-fg">
          {value}
        </code>
        <Button
          size="sm"
          onClick={() => {
            void navigator.clipboard?.writeText(value);
            setCopied(true);
            setTimeout(() => setCopied(false), 1200);
          }}
        >
          {copied ? "Copied" : "Copy"}
        </Button>
      </div>
    </Field>
  );
}

/* ---------------------------------------------------------------- */
/* Store card                                                       */
/* ---------------------------------------------------------------- */

function StoreCard({
  entry,
  canAdmin,
  busy,
  onDetail,
  onInstall,
  onUninstall,
}: {
  entry: RegistryEntry;
  canAdmin: boolean;
  busy: boolean;
  onDetail: (e: RegistryEntry) => void;
  onInstall: (e: RegistryEntry) => void;
  onUninstall: (e: RegistryEntry) => void;
}) {
  const Icon = moduleIcon(entry.icon ?? entry.id);
  const sidecar = entry.install.type === "sidecar";
  const navPath =
    entry.install.type === "sidecar" && entry.install.nav?.[0]?.path
      ? entry.install.nav[0].path
      : `/${entry.id}`;

  return (
    <div
      className={cx(
        "group flex flex-col rounded-xl border border-line bg-raised/40 p-4 transition-colors hover:border-[#34373e]",
        busy && "pointer-events-none opacity-50",
      )}
    >
      <div className="flex items-start gap-3">
        <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-line bg-canvas text-fg-secondary">
          <Icon className="h-[18px] w-[18px]" />
        </span>
        <div className="min-w-0 flex-1">
          <button
            onClick={() => onDetail(entry)}
            className="block truncate text-left font-display text-sm font-bold text-fg hover:text-accent"
          >
            {entry.name}
          </button>
          <div className="mt-0.5 truncate font-mono text-[10px] uppercase tracking-micro text-fg-muted">
            {entry.publisher}
            {entry.version ? ` · v${entry.version}` : ""}
          </div>
        </div>
      </div>

      <p className="mt-2.5 line-clamp-2 min-h-[2.4em] text-xs leading-relaxed text-fg-secondary">
        {entry.summary}
      </p>

      <div className="mt-3 flex flex-wrap items-center gap-1.5">
        <KindBadge kind={entry.kind} />
        {entry.verified && <VerifiedBadge />}
        <StatePill state={entry.state} />
      </div>

      <div className="mt-3 flex items-center gap-2 border-t border-line/70 pt-3">
        {/* Action by state */}
        {entry.mount === "headless" ? (
          <span className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
            Headless · sandboxed compute
          </span>
        ) : null}
        {entry.state === "available" && sidecar && canAdmin && (
          <Button variant="primary" size="sm" onClick={() => onInstall(entry)}>
            Install
          </Button>
        )}
        {(entry.state === "installed" || entry.state === "unreachable") && (
          <>
            <Link to={navPath}>
              <Button size="sm">Open</Button>
            </Link>
            {canAdmin && (
              <Button variant="danger" size="sm" onClick={() => onUninstall(entry)}>
                Uninstall
              </Button>
            )}
          </>
        )}
        {entry.state === "included" && (
          <span className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
            Built into this deployment
          </span>
        )}
        {entry.state === "not_in_build" &&
          (entry.install.type === "builtin" && entry.install.contact ? (
            <a href={`mailto:${entry.install.contact}`}>
              <Button size="sm">Contact</Button>
            </a>
          ) : entry.homepage ? (
            <a href={entry.homepage} target="_blank" rel="noreferrer">
              <Button size="sm">Learn more</Button>
            </a>
          ) : (
            <span className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
              Commercial add-on
            </span>
          ))}
        <button
          onClick={() => onDetail(entry)}
          className="ml-auto font-mono text-[10px] uppercase tracking-micro text-fg-muted hover:text-fg-secondary"
        >
          Details →
        </button>
      </div>
    </div>
  );
}

/* ---------------------------------------------------------------- */
/* Install form (pre-filled from a catalog entry, or blank import)  */
/* ---------------------------------------------------------------- */

interface InstallFields {
  id: string;
  name: string;
  publisher: string;
  version: string;
  base_url: string;
  subscribes: string;
  role: "integration" | "viewer";
}

function seedFields(entry?: RegistryEntry): InstallFields {
  if (entry && entry.install.type === "sidecar") {
    const s = entry.install;
    return {
      id: entry.id,
      name: entry.name,
      publisher: entry.publisher,
      version: entry.version ?? "",
      base_url: s.default_base_url ?? "",
      subscribes: (s.subscribes ?? []).join(" "),
      role: s.role === "viewer" ? "viewer" : "integration",
    };
  }
  return { id: "", name: "", publisher: "", version: "", base_url: "", subscribes: "", role: "integration" };
}

function InstallForm({
  entry,
  onInstalled,
}: {
  entry?: RegistryEntry;
  onInstalled: (r: ModuleRegistered) => void;
}) {
  const [form, setForm] = useState<InstallFields>(() => seedFields(entry));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const set = (k: keyof InstallFields) => (v: string) => setForm((f) => ({ ...f, [k]: v }));
  // Installing from a verified catalog listing locks the identity so you get exactly what's advertised;
  // only the base URL (and event scope) are operator-specific. Manual import leaves everything editable.
  const locked = !!entry && entry.verified;

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      const subscribes = form.subscribes
        .split(/[\s,]+/)
        .map((s) => s.trim())
        .filter(Boolean);
      // Carry the catalog entry's nav + description through so the registered module's route matches
      // the card's "Open" link (the kernel would otherwise default nav to /{id}).
      const nav = entry && entry.install.type === "sidecar" ? entry.install.nav : undefined;
      const body: ModuleRegisterRequest = {
        id: form.id.trim(),
        name: form.name.trim(),
        base_url: form.base_url.trim(),
        version: form.version.trim() || undefined,
        publisher: form.publisher.trim() || undefined,
        description: entry?.description ?? undefined,
        nav: nav && nav.length ? nav : undefined,
        subscribes: subscribes.length ? subscribes : undefined,
        role: form.role,
      };
      const r = await api.registerModule(body);
      onInstalled(r);
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setBusy(false);
    }
  };

  const ready = form.id.trim() && form.name.trim() && /^https?:\/\//.test(form.base_url.trim());

  return (
    <div className="space-y-4">
      {entry && entry.install.type === "sidecar" && entry.install.image && (
        <div className="rounded-lg border border-line bg-canvas/60 p-3 text-xs text-fg-secondary">
          <span className="font-mono uppercase tracking-micro text-fg-muted">Deploy hint</span>
          <div className="mt-1 font-mono text-[11px] text-fg">{entry.install.image}</div>
          <p className="mt-1.5 leading-relaxed text-fg-muted">
            Run the sidecar yourself, then enter the URL Heldar should reach it at. Heldar mints its key
            + webhook and reverse-proxies it; it does not start the process for you.
          </p>
        </div>
      )}
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <Field label="Module id">
          <Input value={form.id} onChange={(e) => set("id")(e.target.value)} disabled={locked} placeholder="visitor-portal" />
        </Field>
        <Field label="Name">
          <Input value={form.name} onChange={(e) => set("name")(e.target.value)} disabled={locked} placeholder="Visitor Portal" />
        </Field>
        <Field label="Base URL" hint="The sidecar origin Heldar reverse-proxies to.">
          <Input value={form.base_url} onChange={(e) => set("base_url")(e.target.value)} placeholder="http://127.0.0.1:9123" />
        </Field>
        <Field label="Subscribes" hint="Event types to receive (space separated). Blank = all.">
          <Input value={form.subscribes} onChange={(e) => set("subscribes")(e.target.value)} placeholder="zone_enter entry_matched" />
        </Field>
        <Field label="Publisher">
          <Input value={form.publisher} onChange={(e) => set("publisher")(e.target.value)} disabled={locked} placeholder="ACME Corp" />
        </Field>
        <Field label="Key role" hint="viewer (read) or integration (read + ingest).">
          <Select value={form.role} onChange={(e) => set("role")(e.target.value)}>
            <option value="integration">integration — read + ingest</option>
            <option value="viewer">viewer — read only</option>
          </Select>
        </Field>
      </div>
      {error && <p className="text-sm text-danger">{error}</p>}
      <Button variant="primary" onClick={submit} disabled={!ready || busy} className="w-full justify-center">
        {busy ? <Spinner size={14} /> : null}
        Install plugin
      </Button>
    </div>
  );
}

/* ---------------------------------------------------------------- */
/* Drawer bodies                                                    */
/* ---------------------------------------------------------------- */

function DetailBody({ entry, onInstall, canAdmin }: { entry: RegistryEntry; canAdmin: boolean; onInstall: () => void }) {
  const sidecar = entry.install.type === "sidecar";
  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-1.5">
        <KindBadge kind={entry.kind} />
        {entry.verified && <VerifiedBadge />}
        <StatePill state={entry.state} />
      </div>
      <p className="text-sm leading-relaxed text-fg-secondary">{entry.description ?? entry.summary}</p>
      <dl className="grid grid-cols-2 gap-2 text-xs">
        <div>
          <dt className="font-mono uppercase tracking-micro text-fg-muted">Publisher</dt>
          <dd className="mt-0.5 text-fg">{entry.publisher}</dd>
        </div>
        {entry.version && (
          <div>
            <dt className="font-mono uppercase tracking-micro text-fg-muted">Version</dt>
            <dd className="mt-0.5 text-fg">v{entry.version}</dd>
          </div>
        )}
        <div>
          <dt className="font-mono uppercase tracking-micro text-fg-muted">Source</dt>
          <dd className="mt-0.5 truncate text-fg">{entry.source}</dd>
        </div>
        {entry.categories && entry.categories.length > 0 && (
          <div>
            <dt className="font-mono uppercase tracking-micro text-fg-muted">Categories</dt>
            <dd className="mt-0.5 text-fg">{entry.categories.join(", ")}</dd>
          </div>
        )}
      </dl>
      {entry.homepage && (
        <a href={entry.homepage} target="_blank" rel="noreferrer" className="inline-block text-xs text-accent hover:underline">
          {entry.homepage} →
        </a>
      )}
      {entry.state === "available" && sidecar && canAdmin && (
        <Button variant="primary" onClick={onInstall} className="w-full justify-center">
          Install
        </Button>
      )}
    </div>
  );
}

function CredentialsBody({ reg }: { reg: ModuleRegistered }) {
  return (
    <div className="space-y-3">
      <p className="rounded-lg border border-accent/40 bg-accent/5 px-3 py-2 text-xs text-fg-secondary">
        Copy these into the sidecar now — they are shown only once.
      </p>
      <CopyRow label="API key (HELDAR_API_KEY)" value={reg.api_key} />
      <CopyRow label="Webhook secret (HELDAR_WEBHOOK_SECRET)" value={reg.webhook_secret} />
      <CopyRow label="Events delivered to" value={`${reg.module.base_url}/heldar/events`} />
    </div>
  );
}

/* ---------------------------------------------------------------- */
/* Registry status strip                                            */
/* ---------------------------------------------------------------- */

function SourceStatus({ sources }: { sources: RegistrySource[] }) {
  const remote = sources.filter((s) => s.source !== "bundled" && s.source !== "local");
  if (remote.length === 0) {
    return (
      <span className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
        Bundled catalog · no remote registry configured
      </span>
    );
  }
  return (
    <div className="flex flex-wrap items-center gap-2">
      {remote.map((s) => (
        <span
          key={s.source}
          title={s.error ?? s.source}
          className={cx(
            "inline-flex items-center gap-1.5 rounded-md border px-2 py-0.5 font-mono text-[10px] uppercase tracking-micro",
            s.verified
              ? "border-emerald-400/40 bg-emerald-400/10 text-emerald-300"
              : "border-danger/40 bg-danger/10 text-red-300",
          )}
        >
          {s.verified ? "✓" : "✕"} {s.name}
        </span>
      ))}
    </div>
  );
}

/* ---------------------------------------------------------------- */
/* Store page                                                       */
/* ---------------------------------------------------------------- */

type DrawerState =
  | { mode: "detail"; entry: RegistryEntry }
  | { mode: "install"; entry?: RegistryEntry }
  | { mode: "credentials"; reg: ModuleRegistered }
  | null;

export function Plugins() {
  const { data, loading, error, refresh } = usePoll(() => api.registry(), 60000);
  const [principal, setPrincipal] = useState<Principal | null>(null);
  const [shelf, setShelf] = useState<Shelf>("core");
  const [query, setQuery] = useState("");
  const [verifiedOnly, setVerifiedOnly] = useState(false);
  const [drawer, setDrawer] = useState<DrawerState>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  useEffect(() => {
    let alive = true;
    api
      .me()
      .then((p) => alive && setPrincipal(p))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);
  // Default NON-admin until /auth/me resolves: with auth OFF it returns a system admin principal
  // (controls show); with auth ON, an unauthenticated/failed lookup stays null (controls hidden,
  // matching what the API enforces) instead of flashing admin controls.
  const canAdmin = principal?.role === "admin";

  const entries = useMemo(() => data?.entries ?? [], [data]);
  const counts = useMemo(() => {
    const c: Record<Shelf, number> = { core: 0, proprietary: 0, community: 0, import: 0 };
    for (const e of entries) c[e.shelf] += 1;
    return c;
  }, [entries]);

  const shelfEntries = useMemo(() => {
    const q = query.trim().toLowerCase();
    return entries
      .filter((e) => e.shelf === shelf)
      .filter((e) => !verifiedOnly || e.verified)
      .filter(
        (e) =>
          !q ||
          e.name.toLowerCase().includes(q) ||
          e.publisher.toLowerCase().includes(q) ||
          e.summary.toLowerCase().includes(q) ||
          (e.categories ?? []).some((c) => c.toLowerCase().includes(q)),
      );
  }, [entries, shelf, query, verifiedOnly]);

  const doRefresh = async () => {
    setRefreshing(true);
    try {
      await api.refreshRegistry();
      await refresh();
    } catch (e) {
      window.alert(errMsg(e));
    } finally {
      setRefreshing(false);
    }
  };

  const uninstall = async (e: RegistryEntry) => {
    if (!window.confirm(`Uninstall "${e.name}"? This revokes its API key and webhook subscription.`)) return;
    setBusyId(e.id);
    try {
      await api.unregisterModule(e.id);
      await refresh();
    } catch (err) {
      window.alert(errMsg(err));
    } finally {
      setBusyId(null);
    }
  };

  const drawerTitle =
    drawer?.mode === "credentials"
      ? `${drawer.reg.module.name} installed`
      : drawer?.mode === "install"
        ? drawer.entry
          ? `Install ${drawer.entry.name}`
          : "Import a sidecar plugin"
        : drawer?.mode === "detail"
          ? drawer.entry.name
          : "";

  return (
    <div className="mx-auto max-w-[1300px] px-4 py-6 sm:px-6">
      <header className="animate-rise">
        <div className="flex flex-wrap items-end justify-between gap-4">
          <div>
            <SectionLabel>Operations · Plugins</SectionLabel>
            <h1 className="mt-1 font-display text-2xl font-extrabold tracking-tight text-fg">
              Plugin Store
            </h1>
            <p className="mt-1 max-w-2xl text-sm text-fg-secondary">
              Browse modules and install out-of-process plugins. Listings from the official registry are
              signature-verified; installing a sidecar mints it a scoped key + webhook and proxies it.
            </p>
          </div>
          {canAdmin && (
            <div className="flex items-center gap-2">
              <Button onClick={doRefresh} disabled={refreshing}>
                {refreshing ? <Spinner size={14} /> : null}
                Refresh registry
              </Button>
            </div>
          )}
        </div>
        <div className="mt-3">
          {data && <SourceStatus sources={data.sources} />}
        </div>
      </header>

      {/* Shelf tabs */}
      <div className="mt-5 flex flex-wrap gap-1 border-b border-line">
        {SHELVES.map((s) => (
          <button
            key={s.key}
            onClick={() => setShelf(s.key)}
            className={cx(
              "relative -mb-px flex items-center gap-2 border-b-2 px-4 py-2.5 text-sm font-medium transition-colors",
              shelf === s.key
                ? "border-accent text-fg"
                : "border-transparent text-fg-secondary hover:text-fg",
            )}
          >
            {s.label}
            <span className="rounded-full bg-raised px-1.5 py-0.5 font-mono text-[10px] text-fg-muted">
              {counts[s.key]}
            </span>
          </button>
        ))}
      </div>

      {/* Controls */}
      <div className="mt-4 flex flex-wrap items-center gap-3">
        <div className="min-w-[200px] flex-1">
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={`Search ${SHELVES.find((s) => s.key === shelf)?.label}…`}
            className="!font-sans"
          />
        </div>
        <label className="flex cursor-pointer items-center gap-2 text-xs text-fg-secondary">
          <input
            type="checkbox"
            checked={verifiedOnly}
            onChange={(e) => setVerifiedOnly(e.target.checked)}
            className="h-3.5 w-3.5 accent-accent"
          />
          Verified only
        </label>
        {shelf === "import" && canAdmin && (
          <Button variant="primary" size="sm" onClick={() => setDrawer({ mode: "install" })}>
            Register a sidecar
          </Button>
        )}
      </div>

      <p className="mt-2 text-xs text-fg-muted">{SHELVES.find((s) => s.key === shelf)?.blurb}</p>

      {/* Grid */}
      <div className="mt-4">
        {error && !data ? (
          <Panel>
            <p className="text-sm text-danger">{error}</p>
          </Panel>
        ) : loading && !data ? (
          <div className="flex items-center justify-center py-20 text-fg-muted">
            <Spinner size={18} />
          </div>
        ) : shelfEntries.length === 0 ? (
          <EmptyState
            title={shelf === "import" ? "No self-made plugins yet" : "Nothing here yet"}
            hint={
              shelf === "import"
                ? "Register your own out-of-process sidecar to extend Heldar."
                : shelf === "proprietary" || shelf === "community"
                  ? "Configure a signed remote registry (HELDAR_REGISTRY_URLS) to populate this shelf."
                  : "No modules match your filter."
            }
          />
        ) : (
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
            {shelfEntries.map((e) => (
              <StoreCard
                key={e.id}
                entry={e}
                canAdmin={canAdmin}
                busy={busyId === e.id}
                onDetail={(en) => setDrawer({ mode: "detail", entry: en })}
                onInstall={(en) => setDrawer({ mode: "install", entry: en })}
                onUninstall={uninstall}
              />
            ))}
          </div>
        )}
      </div>

      {/* Drawer */}
      <Drawer
        open={drawer != null}
        onClose={() => setDrawer(null)}
        title={drawerTitle}
        subtitle={
          drawer?.mode === "install" && drawer.entry
            ? "Heldar mints a scoped key + webhook and proxies the sidecar."
            : undefined
        }
      >
        {drawer?.mode === "detail" && (
          <DetailBody
            entry={drawer.entry}
            canAdmin={canAdmin}
            onInstall={() => setDrawer({ mode: "install", entry: drawer.entry })}
          />
        )}
        {drawer?.mode === "install" && (
          <InstallForm
            entry={drawer.entry}
            onInstalled={(r) => {
              setDrawer({ mode: "credentials", reg: r });
              void refresh();
            }}
          />
        )}
        {drawer?.mode === "credentials" && <CredentialsBody reg={drawer.reg} />}
      </Drawer>
    </div>
  );
}

export default Plugins;
