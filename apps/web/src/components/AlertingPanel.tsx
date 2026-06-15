// Heldar Core — UI-configurable alerting (webhook notifications).
//
// One clickable surface for non-technical operators to point alerts at a webhook
// (Slack / Teams / any URL), toggle delivery, pick a severity threshold, and fire a
// test delivery — no terminal, no env vars. Settings persist server-side in app_state;
// the notifier re-reads them every cycle, so changes take effect without a restart.
//
// Reads are open to any principal; the PUT + test POST are manager+ (the API enforces
// this — the controls mirror it by gating on `canManage`). Shares ui.tsx primitives and
// follows the RecordingPanels.tsx / CameraConfigPanel.tsx patterns (Switch row, ErrorNote).

import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { api, ApiError } from "../lib/api";
import { usePoll } from "../lib/usePoll";
import type { AlertingTestResult, AlertingUpdate } from "../lib/types";
import { Button, Field, Input, Panel, Select, cx } from "./ui";

function errMsg(e: unknown): string {
  return e instanceof ApiError || e instanceof Error ? e.message : String(e);
}

/** A small on/off switch matching the dark/accent design system (mirrors RecordingPanels). */
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

/** Labelled switch row used inside the settings editor. */
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

/** A configured / not-configured badge for the panel header (mirrors OnvifBadge). */
function ConfiguredBadge({ configured }: { configured: boolean }) {
  const color = configured ? "#10b981" : "#52525b";
  return (
    <span
      className="inline-flex items-center gap-1.5 rounded border px-1.5 py-0.5 font-mono text-[9px] font-semibold uppercase tracking-micro"
      style={{ color, borderColor: `${color}55`, backgroundColor: `${color}1a` }}
    >
      <span className="inline-flex h-1.5 w-1.5 rounded-full" style={{ backgroundColor: color }} />
      {configured ? "Configured" : "Not set"}
    </span>
  );
}

export function AlertingPanel({ canManage }: { canManage: boolean }) {
  // Load once; refresh manually after a save (no background polling needed for a settings form).
  const cfg = usePoll(() => api.getAlerting(), 0, []);

  // Local edit state. The webhook field starts blank (the server only ever returns a MASKED value),
  // so leaving it blank on save leaves the stored webhook unchanged.
  const [webhook, setWebhook] = useState("");
  const [enabled, setEnabled] = useState(true);
  const [minSeverity, setMinSeverity] = useState<"warning" | "critical">("warning");

  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [testBusy, setTestBusy] = useState(false);
  const [testResult, setTestResult] = useState<AlertingTestResult | null>(null);

  // Seed the toggles from the loaded config exactly once (don't clobber in-flight edits on refresh).
  const seeded = useRef(false);
  useEffect(() => {
    if (cfg.data && !seeded.current) {
      seeded.current = true;
      setEnabled(cfg.data.enabled);
      setMinSeverity(cfg.data.min_severity);
    }
  }, [cfg.data]);

  const configured = cfg.data?.configured ?? false;
  const masked = cfg.data?.webhook_url_masked ?? null;

  async function save() {
    setError(null);
    setSaved(false);
    setTestResult(null);
    const body: AlertingUpdate = { enabled, min_severity: minSeverity };
    // Three-state webhook: only send it when the operator typed a new value (blank = keep current).
    const trimmed = webhook.trim();
    if (trimmed) body.webhook_url = trimmed;
    setBusy(true);
    try {
      await api.putAlerting(body);
      setWebhook(""); // the new value is now stored + masked server-side
      setSaved(true);
      await cfg.refresh();
    } catch (err) {
      setError(errMsg(err));
    } finally {
      setBusy(false);
    }
  }

  async function sendTest() {
    setTestResult(null);
    setError(null);
    setTestBusy(true);
    try {
      const r = await api.testAlerting();
      setTestResult(r);
    } catch (err) {
      setTestResult({ ok: false, status: null, error: errMsg(err) });
    } finally {
      setTestBusy(false);
    }
  }

  return (
    <Panel
      title="Alerting"
      subtitle="Webhook notifications"
      actions={
        <div className="flex items-center gap-2">
          <ConfiguredBadge configured={configured} />
          {canManage && (
            <Button size="sm" variant="primary" disabled={busy || cfg.loading} onClick={() => void save()}>
              {busy ? "Saving…" : "Save"}
            </Button>
          )}
        </div>
      }
    >
      <div className="space-y-4">
        <p className="text-xs leading-relaxed text-fg-secondary">
          Alerts are POSTed to this webhook (Slack, Microsoft Teams, or any URL) whenever a person or
          vehicle triggers a zone — no terminal needed.
        </p>

        <Field
          label="Webhook URL"
          htmlFor="alert-webhook"
          hint={
            configured
              ? `Currently ${masked ?? "configured"} · leave blank to keep it`
              : "Paste a Slack / Teams / custom incoming-webhook URL"
          }
        >
          <Input
            id="alert-webhook"
            type="url"
            inputMode="url"
            value={webhook}
            onChange={(e) => setWebhook(e.target.value)}
            placeholder={configured ? (masked ?? "https://hooks.slack.com/…") : "https://hooks.slack.com/services/…"}
            disabled={!canManage}
          />
        </Field>

        <div className="space-y-3 border-t border-line pt-3">
          <ToggleField
            label="Delivery enabled"
            hint="Pause without clearing the webhook"
            checked={enabled}
            onChange={setEnabled}
            disabled={!canManage}
          />
        </div>

        <Field label="Send alerts for" htmlFor="alert-severity">
          <Select
            id="alert-severity"
            value={minSeverity}
            onChange={(e) => setMinSeverity(e.target.value as "warning" | "critical")}
            disabled={!canManage}
          >
            <option value="warning">Warning and above</option>
            <option value="critical">Critical only</option>
          </Select>
        </Field>

        {!canManage && (
          <p className="font-mono text-[11px] text-fg-muted">
            Manager role required to change alerting configuration.
          </p>
        )}

        {canManage && (
          <div className="flex flex-wrap items-center gap-2 border-t border-line pt-3">
            <Button
              size="sm"
              disabled={testBusy || !configured}
              onClick={() => void sendTest()}
              title={configured ? undefined : "Save a webhook first"}
            >
              {testBusy ? "Sending…" : "Send test alert"}
            </Button>
            {!configured && (
              <span className="font-mono text-[11px] text-fg-muted">
                Save a webhook URL to enable the test.
              </span>
            )}
            {testResult &&
              (testResult.ok ? (
                <span className="font-mono text-[11px] text-rec">
                  Test delivered{testResult.status != null ? ` · HTTP ${testResult.status}` : ""}.
                </span>
              ) : (
                <span className="font-mono text-[11px] text-danger">
                  Test failed
                  {testResult.status != null ? ` · HTTP ${testResult.status}` : ""}
                  {testResult.error ? ` · ${testResult.error}` : ""}
                </span>
              ))}
          </div>
        )}

        {error && <ErrorNote>{error}</ErrorNote>}
        {saved && !error && <p className="font-mono text-[11px] text-rec">Alerting settings saved.</p>}
        {cfg.error && !cfg.data && (
          <ErrorNote>Failed to load alerting configuration: {cfg.error}</ErrorNote>
        )}
      </div>
    </Panel>
  );
}

export default AlertingPanel;
