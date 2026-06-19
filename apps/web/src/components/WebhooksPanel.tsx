// Heldar Core — webhook subscriptions (the generic event-delivery substrate).
//
// SUPERSEDES the single-URL alerting panel: operators register any number of webhook
// subscriptions, each with an event-type filter, a minimum severity, an optional HMAC
// signing secret, and an enable toggle. Every subscription is an independent at-least-once
// deliverer (the body is signed with `X-Heldar-Signature: sha256=…` when a secret is set).
//
// Reads (the list + delivery log) are open to any principal; create / edit / delete / test are
// manager+ (the API enforces this — the controls mirror it by gating on `canManage`, derived from
// api.me() in System.tsx). Shares ui.tsx primitives and follows the AlertingPanel /
// RecordingPanels / CameraConfigPanel settings-panel patterns (Switch / Field / Button / Select).

import { useState } from "react";
import type { ReactNode } from "react";
import { api, ApiError } from "../lib/api";
import { usePoll } from "../lib/usePoll";
import type {
  EventTypeInfo,
  Severity,
  WebhookDelivery,
  WebhookSubscription,
  WebhookSubscriptionCreate,
  WebhookSubscriptionUpdate,
  WebhookTestResult,
} from "../lib/types";
import { Button, Field, Input, Panel, Select, Spinner, cx } from "./ui";
import { timeAgo } from "../lib/format";

/** The event-type sentinel that matches every type. */
const ALL_TYPES = "*";

function errMsg(e: unknown): string {
  return e instanceof ApiError || e instanceof Error ? e.message : String(e);
}

const SEVERITY_LABEL: Record<Severity, string> = {
  info: "Info+",
  warning: "Warning+",
  critical: "Critical only",
};

const SEVERITY_COLOR: Record<Severity, string> = {
  info: "#71717a",
  warning: "#fbbf24",
  critical: "#ef4444",
};

/* ---------------------------------------------------------------- */
/* Small shared bits (mirror AlertingPanel's local primitives)       */
/* ---------------------------------------------------------------- */

/** A small on/off switch matching the dark/accent design system (mirrors AlertingPanel). */
function Switch({
  checked,
  onChange,
  disabled,
  id,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  id?: string;
}) {
  return (
    <button
      id={id}
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      onClick={() => !disabled && onChange(!checked)}
      className={cx(
        "relative inline-flex h-5 w-9 shrink-0 items-center rounded-full border transition-colors duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-canvas disabled:cursor-not-allowed disabled:opacity-50",
        checked ? "border-transparent bg-accent" : "border-line bg-raised",
      )}
    >
      <span
        className={cx(
          "inline-block h-3.5 w-3.5 rounded-full bg-fg shadow transition-transform duration-150",
          checked ? "translate-x-4" : "translate-x-0.5",
        )}
      />
    </button>
  );
}

/** Labelled switch row used inside the editor. */
function ToggleField({
  label,
  hint,
  checked,
  onChange,
  disabled,
}: {
  label: ReactNode;
  hint?: ReactNode;
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <div className="flex items-center justify-between gap-3">
      <div className="min-w-0">
        <div className="font-mono text-[10px] font-medium uppercase tracking-micro text-fg-secondary">
          {label}
        </div>
        {hint != null && <div className="mt-0.5 text-[11px] leading-snug text-fg-muted">{hint}</div>}
      </div>
      <Switch checked={checked} onChange={onChange} disabled={disabled} />
    </div>
  );
}

function ErrorNote({ children }: { children: ReactNode }) {
  return <p className="font-mono text-xs text-danger">{children}</p>;
}

/** A small coloured pill (enabled / signed / severity). */
function Badge({ color, children }: { color: string; children: ReactNode }) {
  return (
    <span
      className="inline-flex items-center gap-1 rounded border px-1.5 py-0.5 font-mono text-[9px] font-semibold uppercase tracking-micro"
      style={{ color, borderColor: `${color}55`, backgroundColor: `${color}1a` }}
    >
      {children}
    </span>
  );
}

/** A selectable chip used for the event-type filter and the all/specific toggle. */
function Chip({
  selected,
  onClick,
  disabled,
  title,
  children,
}: {
  selected: boolean;
  onClick: () => void;
  disabled?: boolean;
  title?: string;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      title={title}
      className={cx(
        "rounded border px-2 py-1 font-mono text-[10px] uppercase tracking-micro transition-colors duration-150 disabled:cursor-not-allowed disabled:opacity-50",
        selected
          ? "border-accent/60 bg-accent/15 text-accent-soft"
          : "border-line bg-raised text-fg-secondary hover:border-[#34373e] hover:text-fg",
      )}
    >
      {children}
    </button>
  );
}

/* ---------------------------------------------------------------- */
/* Create / edit editor                                             */
/* ---------------------------------------------------------------- */

function WebhookEditor({
  initial,
  eventTypes,
  onClose,
  onSaved,
}: {
  /** The subscription being edited; undefined = create a new one. */
  initial?: WebhookSubscription;
  eventTypes: EventTypeInfo[];
  onClose: () => void;
  onSaved: () => void;
}) {
  const isEdit = initial != null;
  const initialAll = !initial || initial.event_types.includes(ALL_TYPES);

  const [name, setName] = useState(initial?.name ?? "");
  const [url, setUrl] = useState(initial?.url ?? "");
  const [minSeverity, setMinSeverity] = useState<Severity>(initial?.min_severity ?? "info");
  const [enabled, setEnabled] = useState(initial?.enabled ?? true);
  const [allTypes, setAllTypes] = useState(initialAll);
  const [selected, setSelected] = useState<Set<string>>(
    () => new Set(initial && !initialAll ? initial.event_types : []),
  );
  // Secret is never returned by the API; blank = keep current (edit) or none (create). The
  // "remove" toggle clears it via an explicit null on update.
  const [secret, setSecret] = useState("");
  const [clearSecret, setClearSecret] = useState(false);

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  function toggleType(t: string) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(t)) next.delete(t);
      else next.add(t);
      return next;
    });
  }

  async function save() {
    setError(null);
    const trimmedName = name.trim();
    const trimmedUrl = url.trim();
    if (!trimmedName) {
      setError("Name is required.");
      return;
    }
    if (!/^https?:\/\//i.test(trimmedUrl)) {
      setError("URL must be an http(s) URL.");
      return;
    }
    const eventTypesValue = allTypes ? [ALL_TYPES] : Array.from(selected);
    if (!allTypes && eventTypesValue.length === 0) {
      setError("Select at least one event type, or choose “All event types”.");
      return;
    }

    setBusy(true);
    try {
      if (isEdit && initial) {
        const body: WebhookSubscriptionUpdate = {
          name: trimmedName,
          url: trimmedUrl,
          event_types: eventTypesValue,
          min_severity: minSeverity,
          enabled,
        };
        // Three-state secret: a "remove" wins; else a typed value replaces; else leave unchanged.
        if (clearSecret) body.secret = null;
        else if (secret.trim()) body.secret = secret.trim();
        await api.updateWebhook(initial.id, body);
      } else {
        const body: WebhookSubscriptionCreate = {
          name: trimmedName,
          url: trimmedUrl,
          event_types: eventTypesValue,
          min_severity: minSeverity,
          enabled,
        };
        const s = secret.trim();
        if (s) body.secret = s;
        await api.createWebhook(body);
      }
      onSaved();
      onClose();
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="space-y-4 rounded-md border border-line bg-canvas/40 p-4">
      <div className="flex items-center justify-between gap-2">
        <h3 className="font-display text-sm font-bold text-fg">
          {isEdit ? "Edit webhook" : "New webhook"}
        </h3>
        <div className="flex items-center gap-2">
          <Button size="sm" variant="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button size="sm" variant="primary" onClick={() => void save()} disabled={busy}>
            {busy ? "Saving…" : isEdit ? "Save changes" : "Create"}
          </Button>
        </div>
      </div>

      <Field label="Name" htmlFor="wh-name">
        <Input
          id="wh-name"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="Ops Slack"
          disabled={busy}
        />
      </Field>

      <Field label="Endpoint URL" htmlFor="wh-url" hint="Matching events are POSTed here as signed JSON.">
        <Input
          id="wh-url"
          type="url"
          inputMode="url"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          placeholder="https://example.com/heldar/webhook"
          disabled={busy}
        />
      </Field>

      <Field
        label="Event types"
        hint={allTypes ? "Delivering every event type." : "Delivering only the selected event types."}
      >
        <>
          <div className="flex flex-wrap gap-1.5">
            <Chip selected={allTypes} onClick={() => setAllTypes(true)} disabled={busy}>
              All event types (*)
            </Chip>
            <Chip selected={!allTypes} onClick={() => setAllTypes(false)} disabled={busy}>
              Specific types
            </Chip>
          </div>
          {!allTypes && (
            <div className="mt-2 flex flex-wrap gap-1.5">
              {eventTypes.length === 0 ? (
                <span className="font-mono text-[11px] text-fg-muted">No event types available.</span>
              ) : (
                eventTypes.map((t) => (
                  <Chip
                    key={t.event_type}
                    selected={selected.has(t.event_type)}
                    onClick={() => toggleType(t.event_type)}
                    disabled={busy}
                    title={t.description}
                  >
                    {t.event_type}
                  </Chip>
                ))
              )}
            </div>
          )}
        </>
      </Field>

      <Field
        label="Minimum severity"
        htmlFor="wh-sev"
        hint="Only events at or above this severity are delivered."
      >
        <Select
          id="wh-sev"
          value={minSeverity}
          onChange={(e) => setMinSeverity(e.target.value as Severity)}
          disabled={busy}
        >
          <option value="info">All severities (info and above)</option>
          <option value="warning">Warning and above</option>
          <option value="critical">Critical only</option>
        </Select>
      </Field>

      <Field
        label="Signing secret"
        htmlFor="wh-secret"
        hint={
          isEdit && initial?.has_secret
            ? "A secret is configured. Type a new value to replace it, or leave blank to keep it."
            : "Optional. Sets the HMAC-SHA256 key sent as X-Heldar-Signature."
        }
      >
        <Input
          id="wh-secret"
          type="password"
          autoComplete="new-password"
          value={secret}
          onChange={(e) => setSecret(e.target.value)}
          placeholder={isEdit && initial?.has_secret ? "•••••••• (unchanged)" : "whsec_…"}
          disabled={busy || clearSecret}
        />
      </Field>

      {isEdit && initial?.has_secret && (
        <ToggleField
          label="Remove signing secret"
          hint="Deliver unsigned (no X-Heldar-Signature header)."
          checked={clearSecret}
          onChange={setClearSecret}
          disabled={busy}
        />
      )}

      <div className="border-t border-line pt-3">
        <ToggleField
          label="Enabled"
          hint="Pause delivery without deleting the subscription."
          checked={enabled}
          onChange={setEnabled}
          disabled={busy}
        />
      </div>

      {error && <ErrorNote>{error}</ErrorNote>}
    </div>
  );
}

/* ---------------------------------------------------------------- */
/* One subscription row (test + delivery log)                        */
/* ---------------------------------------------------------------- */

function SubscriptionCard({
  sub,
  canManage,
  onEdit,
  onChanged,
}: {
  sub: WebhookSubscription;
  canManage: boolean;
  onEdit: () => void;
  onChanged: () => void;
}) {
  const [testBusy, setTestBusy] = useState(false);
  const [testResult, setTestResult] = useState<WebhookTestResult | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [showDeliveries, setShowDeliveries] = useState(false);
  const [deliveries, setDeliveries] = useState<WebhookDelivery[] | null>(null);
  const [deliveriesLoading, setDeliveriesLoading] = useState(false);
  const [deliveriesError, setDeliveriesError] = useState<string | null>(null);

  async function sendTest() {
    setTestResult(null);
    setTestBusy(true);
    try {
      setTestResult(await api.testWebhook(sub.id));
    } catch (e) {
      setTestResult({ ok: false, status: null, error: errMsg(e) });
    } finally {
      setTestBusy(false);
    }
  }

  async function loadDeliveries() {
    setDeliveriesLoading(true);
    setDeliveriesError(null);
    try {
      setDeliveries(await api.webhookDeliveries(sub.id, 20));
    } catch (e) {
      setDeliveriesError(errMsg(e));
    } finally {
      setDeliveriesLoading(false);
    }
  }

  function toggleDeliveries() {
    const next = !showDeliveries;
    setShowDeliveries(next);
    if (next) void loadDeliveries();
  }

  async function remove() {
    setDeleting(true);
    try {
      await api.deleteWebhook(sub.id);
      onChanged(); // card unmounts on the refreshed list
    } catch (e) {
      setTestResult({ ok: false, status: null, error: errMsg(e) });
      setDeleting(false);
      setConfirmingDelete(false);
    }
  }

  return (
    <div className="rounded-md border border-line bg-canvas/40 p-3">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <span className="truncate text-sm font-semibold text-fg">{sub.name}</span>
            <Badge color={sub.enabled ? "#10b981" : "#52525b"}>{sub.enabled ? "Enabled" : "Paused"}</Badge>
            {sub.has_secret && <Badge color="#a78bfa">Signed</Badge>}
            <Badge color={SEVERITY_COLOR[sub.min_severity]}>{SEVERITY_LABEL[sub.min_severity]}</Badge>
          </div>
          <div className="mt-1 truncate font-mono text-[11px] text-fg-muted" title={sub.url}>
            {sub.url}
          </div>
          <div className="mt-1.5 flex flex-wrap gap-1">
            {sub.event_types.map((t) => (
              <span
                key={t}
                className="rounded border border-line bg-raised px-1.5 py-0.5 font-mono text-[9px] uppercase tracking-micro text-fg-secondary"
              >
                {t === ALL_TYPES ? "all events (*)" : t}
              </span>
            ))}
          </div>
        </div>

        <div className="flex shrink-0 flex-wrap items-center gap-1.5">
          <Button size="sm" variant="ghost" onClick={toggleDeliveries}>
            {showDeliveries ? "Hide log" : "Deliveries"}
          </Button>
          {canManage && (
            <>
              <Button size="sm" onClick={() => void sendTest()} disabled={testBusy}>
                {testBusy ? "Testing…" : "Send test"}
              </Button>
              <Button size="sm" variant="ghost" onClick={onEdit}>
                Edit
              </Button>
              {confirmingDelete ? (
                <>
                  <Button size="sm" variant="danger" onClick={() => void remove()} disabled={deleting}>
                    {deleting ? "Deleting…" : "Confirm"}
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => setConfirmingDelete(false)}
                    disabled={deleting}
                  >
                    Cancel
                  </Button>
                </>
              ) : (
                <Button size="sm" variant="danger" onClick={() => setConfirmingDelete(true)}>
                  Delete
                </Button>
              )}
            </>
          )}
        </div>
      </div>

      {testResult && (
        <p className={cx("mt-2 font-mono text-[11px]", testResult.ok ? "text-rec" : "text-danger")}>
          {testResult.ok
            ? `Test delivered${testResult.status != null ? ` · HTTP ${testResult.status}` : ""}.`
            : `Test failed${testResult.status != null ? ` · HTTP ${testResult.status}` : ""}${
                testResult.error ? ` · ${testResult.error}` : ""
              }`}
        </p>
      )}

      {showDeliveries && (
        <div className="mt-3 border-t border-line pt-3">
          {deliveriesLoading && !deliveries ? (
            <div className="flex items-center gap-2 font-mono text-[11px] text-fg-muted">
              <Spinner size={12} /> Loading deliveries…
            </div>
          ) : deliveriesError ? (
            <ErrorNote>Failed to load deliveries: {deliveriesError}</ErrorNote>
          ) : deliveries && deliveries.length > 0 ? (
            <ul className="space-y-1">
              {deliveries.map((d) => (
                <li key={d.id} className="flex items-center justify-between gap-2 font-mono text-[10px]">
                  <span className="flex min-w-0 items-center gap-2">
                    <span
                      className={cx(
                        "inline-flex h-1.5 w-1.5 shrink-0 rounded-full",
                        d.status === "delivered" ? "bg-rec" : "bg-danger",
                      )}
                    />
                    <span className="truncate text-fg-secondary">{d.event_type ?? "—"}</span>
                  </span>
                  <span className="flex shrink-0 items-center gap-2 text-fg-muted">
                    <span className={d.status === "delivered" ? "text-rec" : "text-danger"}>
                      {d.response_code != null ? `HTTP ${d.response_code}` : d.status}
                    </span>
                    <span className="tabular-nums">{timeAgo(d.created_at)}</span>
                  </span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="font-mono text-[11px] text-fg-muted">No deliveries yet.</p>
          )}
        </div>
      )}
    </div>
  );
}

/* ---------------------------------------------------------------- */
/* Panel                                                            */
/* ---------------------------------------------------------------- */

export function WebhooksPanel({ canManage }: { canManage: boolean }) {
  // Load once; refresh after a mutation (no background polling for a settings surface).
  const subs = usePoll(() => api.listWebhooks(), 0, []);
  const evTypes = usePoll(() => api.eventTypes(), 0, []);

  type Editing = { mode: "new" } | { mode: "edit"; sub: WebhookSubscription } | null;
  const [editing, setEditing] = useState<Editing>(null);

  const list = subs.data ?? [];

  return (
    <Panel
      title="Webhooks"
      subtitle="Event-delivery subscriptions"
      actions={
        <div className="flex items-center gap-2">
          <span className="font-mono text-[11px] tabular-nums text-fg-muted">{list.length}</span>
          {canManage && editing == null && (
            <Button size="sm" variant="primary" onClick={() => setEditing({ mode: "new" })}>
              New webhook
            </Button>
          )}
        </div>
      }
    >
      <div className="space-y-4">
        <p className="text-xs leading-relaxed text-fg-secondary">
          Forward Heldar events to any external system. Each subscription POSTs matching events as
          JSON, filtered by event type and minimum severity, signed with HMAC-SHA256 in{" "}
          <code className="font-mono text-fg-muted">X-Heldar-Signature</code> when a secret is set,
          with at-least-once retry.
        </p>

        {editing != null && (
          <WebhookEditor
            key={editing.mode === "edit" ? editing.sub.id : "new"}
            initial={editing.mode === "edit" ? editing.sub : undefined}
            eventTypes={evTypes.data ?? []}
            onClose={() => setEditing(null)}
            onSaved={() => void subs.refresh()}
          />
        )}

        {subs.error && !subs.data ? (
          <ErrorNote>Failed to load webhooks: {subs.error}</ErrorNote>
        ) : subs.loading && !subs.data ? (
          <div className="flex items-center gap-2 font-mono text-xs text-fg-muted">
            <Spinner size={14} /> Loading webhooks…
          </div>
        ) : list.length === 0 ? (
          editing == null && (
            <p className="font-mono text-xs text-fg-muted">
              No webhook subscriptions.
              {canManage ? " Create one to forward events to an external system." : ""}
            </p>
          )
        ) : (
          <div className="space-y-2">
            {list.map((sub) => (
              <SubscriptionCard
                key={sub.id}
                sub={sub}
                canManage={canManage}
                onEdit={() => setEditing({ mode: "edit", sub })}
                onChanged={() => void subs.refresh()}
              />
            ))}
          </div>
        )}

        {!canManage && (
          <p className="font-mono text-[11px] text-fg-muted">
            Manager role required to create, edit, or test webhooks.
          </p>
        )}
      </div>
    </Panel>
  );
}

export default WebhooksPanel;
