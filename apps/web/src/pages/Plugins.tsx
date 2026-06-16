// Heldar Core — Plugins: the module registry surface (Phase B of the plugin platform).
//
// Lists every loaded module (compiled-in core/proprietary + runtime-registered sidecars) and lets an
// admin install a sidecar plugin or uninstall one. Installing mints the sidecar a scoped API key + a
// webhook subscription; those credentials are returned ONCE and shown here for the operator to copy
// into the sidecar. (The browsable Core/Proprietary/self-made store is the next phase; this is the
// install/uninstall + health surface it grows from.)

import { useEffect, useState } from "react";
import { api, ApiError } from "../lib/api";
import { usePoll } from "../lib/usePoll";
import type { ModuleManifest, ModuleRegistered, Principal } from "../lib/types";
import {
  Button,
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
  imported: { label: "Imported", cls: "border-teal-400/40 bg-teal-400/10 text-teal-300" },
};

function KindBadge({ kind }: { kind: string }) {
  const m = KIND_META[kind] ?? KIND_META.imported;
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

function HealthPill({ health }: { health?: string }) {
  if (!health) return null;
  const state = health === "healthy" ? "recording" : health === "unreachable" ? "error" : "connecting";
  return <StatusPill state={state} label={health.toUpperCase()} />;
}

/** One-line copy field for the once-only credentials. */
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
/* Installed list                                                   */
/* ---------------------------------------------------------------- */

function ModuleRow({
  m,
  canAdmin,
  onUninstall,
}: {
  m: ModuleManifest;
  canAdmin: boolean;
  onUninstall: (m: ModuleManifest) => void;
}) {
  const imported = m.kind === "imported";
  return (
    <div className="flex flex-wrap items-center gap-3 rounded-lg border border-line bg-raised/40 px-4 py-3">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="truncate font-display text-sm font-bold text-fg">{m.name}</span>
          <KindBadge kind={m.kind} />
          {m.version && (
            <span className="font-mono text-[11px] text-fg-muted">v{m.version}</span>
          )}
        </div>
        {m.description && (
          <p className="mt-0.5 truncate text-xs text-fg-secondary">{m.description}</p>
        )}
        <div className="mt-1 flex items-center gap-3 font-mono text-[10px] uppercase tracking-micro text-fg-muted">
          <span>{m.id}</span>
          {m.publisher && <span>· {m.publisher}</span>}
          {m.nav[0] && <span>· {m.nav[0].path}</span>}
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-3">
        <HealthPill health={m.health} />
        {imported && canAdmin && (
          <Button variant="danger" size="sm" onClick={() => onUninstall(m)}>
            Uninstall
          </Button>
        )}
        {!imported && (
          <span className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
            Built-in
          </span>
        )}
      </div>
    </div>
  );
}

/* ---------------------------------------------------------------- */
/* Install form                                                     */
/* ---------------------------------------------------------------- */

const EMPTY_FORM = {
  id: "",
  name: "",
  base_url: "",
  version: "",
  publisher: "",
  description: "",
  subscribes: "",
  role: "integration" as "integration" | "viewer",
};

function InstallForm({ onInstalled }: { onInstalled: (r: ModuleRegistered) => void }) {
  const [form, setForm] = useState({ ...EMPTY_FORM });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const set = (k: keyof typeof EMPTY_FORM) => (v: string) => setForm((f) => ({ ...f, [k]: v }));

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      const subscribes = form.subscribes
        .split(/[\s,]+/)
        .map((s) => s.trim())
        .filter(Boolean);
      const r = await api.registerModule({
        id: form.id.trim(),
        name: form.name.trim(),
        base_url: form.base_url.trim(),
        version: form.version.trim() || undefined,
        publisher: form.publisher.trim() || undefined,
        description: form.description.trim() || undefined,
        subscribes: subscribes.length ? subscribes : undefined,
        role: form.role,
      });
      setForm({ ...EMPTY_FORM });
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
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
        <Field label="Module id" hint="Slug used for /m/{id}/ + nav. Must be unique.">
          <Input
            value={form.id}
            onChange={(e) => set("id")(e.target.value)}
            placeholder="visitor-portal"
          />
        </Field>
        <Field label="Name">
          <Input
            value={form.name}
            onChange={(e) => set("name")(e.target.value)}
            placeholder="Visitor Portal"
          />
        </Field>
        <Field label="Base URL" hint="The sidecar origin Heldar reverse-proxies to.">
          <Input
            value={form.base_url}
            onChange={(e) => set("base_url")(e.target.value)}
            placeholder="http://127.0.0.1:9123"
          />
        </Field>
        <Field label="Subscribes" hint="Event types to receive (space/comma separated). Blank = all.">
          <Input
            value={form.subscribes}
            onChange={(e) => set("subscribes")(e.target.value)}
            placeholder="zone_enter zone_dwell entry_matched"
          />
        </Field>
        <Field label="Version">
          <Input value={form.version} onChange={(e) => set("version")(e.target.value)} placeholder="1.0.0" />
        </Field>
        <Field label="Publisher">
          <Input
            value={form.publisher}
            onChange={(e) => set("publisher")(e.target.value)}
            placeholder="ACME Corp"
          />
        </Field>
        <Field
          label="Key role"
          hint="Least-privilege grant for the minted key: viewer (read) or integration (read + ingest)."
        >
          <Select value={form.role} onChange={(e) => set("role")(e.target.value)}>
            <option value="integration">integration — read + ingest</option>
            <option value="viewer">viewer — read only</option>
          </Select>
        </Field>
        <Field label="Description">
          <Input
            value={form.description}
            onChange={(e) => set("description")(e.target.value)}
            placeholder="What this plugin does"
          />
        </Field>
      </div>
      {error && <p className="text-sm text-danger">{error}</p>}
      <div className="flex items-center gap-3">
        <Button variant="primary" onClick={submit} disabled={!ready || busy}>
          {busy ? <Spinner size={14} /> : null}
          Install plugin
        </Button>
        <span className="text-xs text-fg-muted">
          Heldar mints a scoped API key + a webhook subscription and proxies the sidecar at{" "}
          <code className="font-mono text-fg-secondary">/m/{form.id.trim() || "{id}"}/</code>.
        </span>
      </div>
    </div>
  );
}

/* ---------------------------------------------------------------- */
/* Credentials reveal (once-only)                                   */
/* ---------------------------------------------------------------- */

function CredentialsPanel({ reg, onDone }: { reg: ModuleRegistered; onDone: () => void }) {
  return (
    <Panel
      title={`${reg.module.name} installed`}
      subtitle="Copy these into the sidecar now — they are shown only once."
      className="border-accent/40"
    >
      <div className="space-y-3">
        <CopyRow label="API key (HELDAR_API_KEY)" value={reg.api_key} />
        <CopyRow label="Webhook secret (HELDAR_WEBHOOK_SECRET)" value={reg.webhook_secret} />
        <CopyRow label="Events delivered to" value={`${reg.module.base_url}/heldar/events`} />
        <div className="flex justify-end pt-1">
          <Button variant="primary" onClick={onDone}>
            Done
          </Button>
        </div>
      </div>
    </Panel>
  );
}

/* ---------------------------------------------------------------- */
/* Page                                                             */
/* ---------------------------------------------------------------- */

export function Plugins() {
  const { data, loading, error, refresh } = usePoll(() => api.modules(), 15000);
  const [principal, setPrincipal] = useState<Principal | null>(null);
  const [registered, setRegistered] = useState<ModuleRegistered | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    api
      .me()
      .then((p) => alive && setPrincipal(p))
      .catch(() => {
        /* unauthenticated / auth off — leave null (controls gated off, but API may still allow) */
      });
    return () => {
      alive = false;
    };
  }, []);
  // With auth OFF the API treats every caller as admin, so enable controls when we have no principal.
  const canAdmin = principal == null || principal.role === "admin";

  const modules = data ?? [];
  const imported = modules.filter((m) => m.kind === "imported");
  const builtin = modules.filter((m) => m.kind !== "imported");

  const uninstall = async (m: ModuleManifest) => {
    if (!window.confirm(`Uninstall "${m.name}"? This revokes its API key and webhook subscription.`))
      return;
    setBusyId(m.id);
    try {
      await api.unregisterModule(m.id);
      await refresh();
    } catch (e) {
      window.alert(errMsg(e));
    } finally {
      setBusyId(null);
    }
  };

  return (
    <div className="mx-auto max-w-[1200px] px-4 py-6 sm:px-6">
      <header className="animate-rise">
        <SectionLabel>Operations · Plugins</SectionLabel>
        <h1 className="mt-1 font-display text-2xl font-extrabold tracking-tight text-fg">
          Plugin Modules
        </h1>
        <p className="mt-1 max-w-2xl text-sm text-fg-secondary">
          Compiled-in modules ship with this build; sidecar plugins are out-of-process services Heldar
          reverse-proxies and feeds events to. Installing one mints it a scoped key + webhook.
        </p>
      </header>

      <div className="mt-5 space-y-4">
        {registered && (
          <CredentialsPanel
            reg={registered}
            onDone={() => {
              setRegistered(null);
              void refresh();
            }}
          />
        )}

        <Panel
          title="Installed modules"
          subtitle={`${modules.length} loaded · ${imported.length} sidecar`}
          actions={
            <Button size="sm" onClick={() => void refresh()} disabled={loading}>
              {loading ? <Spinner size={13} /> : "Refresh"}
            </Button>
          }
        >
          {error && !data ? (
            <p className="text-sm text-danger">{error}</p>
          ) : modules.length === 0 ? (
            <EmptyState title="No modules loaded" hint="This build links no app modules." />
          ) : (
            <div className="space-y-4">
              {imported.length > 0 && (
                <div className="space-y-2">
                  <SectionLabel>Sidecar plugins</SectionLabel>
                  {imported.map((m) => (
                    <div key={m.id} className={cx(busyId === m.id && "pointer-events-none opacity-50")}>
                      <ModuleRow m={m} canAdmin={canAdmin} onUninstall={uninstall} />
                    </div>
                  ))}
                </div>
              )}
              <div className="space-y-2">
                <SectionLabel>Built-in</SectionLabel>
                {builtin.map((m) => (
                  <ModuleRow key={m.id} m={m} canAdmin={canAdmin} onUninstall={uninstall} />
                ))}
              </div>
            </div>
          )}
        </Panel>

        {canAdmin && (
          <Panel
            title="Install a sidecar plugin"
            subtitle="Register an out-of-process service as a module"
          >
            <InstallForm onInstalled={setRegistered} />
          </Panel>
        )}
      </div>
    </div>
  );
}

export default Plugins;
