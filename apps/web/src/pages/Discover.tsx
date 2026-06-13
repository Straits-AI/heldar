import { useState } from "react";
import type { ChangeEventHandler, FormEvent, ReactNode } from "react";
import { Link } from "react-router-dom";
import { api, ApiError } from "../lib/api";
import type { DiscoverOptions, DiscoverResponse, DiscoveredDevice } from "../lib/types";
import {
  Button,
  cx,
  EmptyState,
  Field,
  Input,
  Panel,
  SectionLabel,
  Spinner,
  StatusLed,
} from "../components/ui";

/* ---------------------------------------------------------------- */
/* Small inline, line-based icons                                   */
/* ---------------------------------------------------------------- */
function ScanIcon({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden="true"
    >
      <circle cx="7" cy="7" r="4.5" />
      <path d="M10.4 10.4 14 14" />
    </svg>
  );
}

function AlertIcon({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden="true"
    >
      <path d="M8 1.8 15 14H1L8 1.8Z" />
      <path d="M8 6.4v3.4" />
      <path d="M8 11.8h.01" />
    </svg>
  );
}

/* ---------------------------------------------------------------- */
/* Custom amber checkbox                                            */
/* ---------------------------------------------------------------- */
function Checkbox({
  id,
  checked,
  onChange,
  children,
}: {
  id?: string;
  checked: boolean;
  onChange: ChangeEventHandler<HTMLInputElement>;
  children: ReactNode;
}) {
  return (
    <label className="group inline-flex cursor-pointer select-none items-center gap-2.5">
      <span className="relative flex h-4 w-4 shrink-0 items-center justify-center">
        <input
          id={id}
          type="checkbox"
          checked={checked}
          onChange={onChange}
          className="peer absolute inset-0 h-full w-full cursor-pointer appearance-none rounded border border-line bg-canvas transition-colors duration-150 checked:border-accent checked:bg-accent hover:border-[#34373e] checked:hover:bg-accent-soft"
        />
        <svg
          viewBox="0 0 12 12"
          className="pointer-events-none relative h-2.5 w-2.5 text-accent-ink opacity-0 transition-opacity peer-checked:opacity-100"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          <path d="M2.5 6.5 5 9l4.5-5" />
        </svg>
      </span>
      <span className="font-mono text-[11px] font-medium uppercase tracking-micro text-fg-secondary transition-colors group-hover:text-fg">
        {children}
      </span>
    </label>
  );
}

/* ---------------------------------------------------------------- */
/* Scan summary metric (panel header)                              */
/* ---------------------------------------------------------------- */
function SummaryMetric({
  label,
  value,
  tone = "default",
}: {
  label: ReactNode;
  value: ReactNode;
  tone?: "default" | "good" | "accent";
}) {
  const toneCls =
    tone === "good" ? "text-rec" : tone === "accent" ? "text-accent" : "text-fg";
  return (
    <div className="flex items-baseline gap-1.5">
      <span className={cx("font-mono text-sm font-semibold tabular-nums", toneCls)}>
        {value}
      </span>
      <span className="font-mono text-[10px] uppercase tracking-micro text-fg-muted">
        {label}
      </span>
    </div>
  );
}

const TH = "px-4 py-2.5 font-mono text-[10px] font-medium uppercase tracking-micro text-fg-muted";

export function Discover() {
  const [targets, setTargets] = useState("192.168.0.0/24");
  const [username, setUsername] = useState("admin");
  const [password, setPassword] = useState("");
  const [verify, setVerify] = useState(true);

  const [scanning, setScanning] = useState(false);
  const [addingAddr, setAddingAddr] = useState<string | null>(null);
  const [result, setResult] = useState<DiscoverResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  function baseOpts(): DiscoverOptions {
    const opts: DiscoverOptions = { targets: targets.trim(), verify };
    const u = username.trim();
    if (u) opts.username = u;
    if (password) opts.password = password;
    return opts;
  }

  async function handleScan(e: FormEvent) {
    e.preventDefault();
    setError(null);
    if (!targets.trim()) {
      setError("Enter one or more targets to scan.");
      return;
    }
    setScanning(true);
    try {
      const resp = await api.discover(baseOpts());
      setResult(resp);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setScanning(false);
    }
  }

  async function handleAdd(device: DiscoveredDevice) {
    setError(null);
    setAddingAddr(device.address);
    try {
      const resp = await api.discover({
        ...baseOpts(),
        targets: device.address,
        auto_add: true,
      });
      const updated = resp.devices.find((d) => d.address === device.address);
      setResult((prev) => {
        if (!prev) return prev;
        const devices = updated
          ? prev.devices.map((d) => (d.address === device.address ? updated : d))
          : prev.devices;
        const added = Array.from(new Set([...prev.added, ...resp.added]));
        return {
          ...prev,
          devices,
          added,
          verified: devices.filter((d) => d.verified).length,
        };
      });
      if (resp.added.length === 0 && !updated?.already_registered) {
        setError(
          `Could not register ${device.address} — credentials were not verified. ` +
            `Check the username/password and that "verify credentials" is on.`,
        );
      }
    } catch (err) {
      setError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setAddingAddr(null);
    }
  }

  const devices = result?.devices ?? [];

  return (
    <div className="mx-auto max-w-[1100px] px-4 py-6 sm:px-6">
      <div className="stagger space-y-5">
        {/* Breadcrumb + page header */}
        <div>
          <nav className="flex items-center gap-2 font-mono text-[10px] uppercase tracking-micro">
            <Link to="/" className="text-fg-muted transition-colors hover:text-fg-secondary">
              Wall
            </Link>
            <span className="text-line">/</span>
            <span className="text-accent">Discover</span>
          </nav>
          <div className="mt-3">
            <SectionLabel>Network</SectionLabel>
            <h1 className="mt-1 font-display text-xl font-bold tracking-tight text-fg">
              Discover Cameras
            </h1>
            <p className="mt-1 max-w-2xl text-sm text-fg-secondary">
              Scan a network range for devices with an open RTSP port, identify the vendor, and
              register verified cameras.
            </p>
          </div>
        </div>

        {/* Scan toolbar */}
        <Panel
          title="Network Scan"
          subtitle="Probe a range for open RTSP endpoints, then verify and register."
        >
          <form onSubmit={handleScan} className="space-y-4">
            <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4">
              <div className="lg:col-span-2">
                <Field
                  label="Targets"
                  htmlFor="targets"
                  hint={
                    <>
                      CIDR <code className="text-fg-secondary">192.168.0.0/24</code>, range{" "}
                      <code className="text-fg-secondary">192.168.0.2-192.168.0.12</code>, a single
                      IP, or a comma-separated list.
                    </>
                  }
                >
                  <Input
                    id="targets"
                    value={targets}
                    onChange={(e) => setTargets(e.target.value)}
                    placeholder="192.168.0.0/24"
                    autoComplete="off"
                  />
                </Field>
              </div>
              <Field label="Username" htmlFor="disc-username">
                <Input
                  id="disc-username"
                  value={username}
                  onChange={(e) => setUsername(e.target.value)}
                  autoComplete="off"
                  placeholder="admin"
                />
              </Field>
              <Field label="Password" htmlFor="disc-password">
                <Input
                  id="disc-password"
                  type="password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  autoComplete="new-password"
                  placeholder="••••••••"
                />
              </Field>
            </div>

            <div className="flex flex-wrap items-center justify-between gap-3 border-t border-line pt-4">
              <Checkbox checked={verify} onChange={(e) => setVerify(e.target.checked)}>
                Verify credentials (ffprobe)
              </Checkbox>
              <div className="flex items-center gap-3">
                {scanning && (
                  <span className="flex items-center gap-2 font-mono text-[10px] uppercase tracking-micro text-fg-muted">
                    <span className="relative flex h-1.5 w-1.5">
                      <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-accent opacity-70" />
                      <span className="relative inline-flex h-1.5 w-1.5 rounded-full bg-accent" />
                    </span>
                    Scanning network · ~30s
                  </span>
                )}
                <Button type="submit" variant="primary" disabled={scanning}>
                  {scanning ? (
                    <>
                      <Spinner size={14} />
                      Scanning…
                    </>
                  ) : (
                    <>
                      <ScanIcon className="h-3.5 w-3.5" />
                      Scan network
                    </>
                  )}
                </Button>
              </div>
            </div>
          </form>
        </Panel>

        {/* Console-style error alert */}
        {error && (
          <div
            role="alert"
            className="overflow-hidden rounded-md border border-danger/40 bg-danger/[0.07]"
          >
            <div className="flex items-start gap-3 px-4 py-3">
              <AlertIcon className="mt-0.5 h-4 w-4 shrink-0 text-danger" />
              <div className="min-w-0">
                <div className="font-mono text-[10px] font-semibold uppercase tracking-micro text-danger">
                  Scan Error
                </div>
                <p className="mt-1 break-words font-mono text-xs leading-relaxed text-red-200">
                  {error}
                </p>
              </div>
            </div>
          </div>
        )}

        {/* Results */}
        {result &&
          (devices.length === 0 ? (
            <EmptyState
              title="No cameras found"
              hint={
                <>
                  Nothing with an open RTSP port responded on{" "}
                  <span className="font-mono text-fg-secondary">{result.scanned}</span>. Try a
                  different range or port.
                </>
              }
            />
          ) : (
            <Panel
              padded={false}
              title="Discovered Devices"
              subtitle={
                <>
                  Scanned <span className="font-mono text-fg-secondary">{result.scanned}</span>
                </>
              }
              actions={
                <div className="flex items-center gap-4">
                  <SummaryMetric label="Found" value={result.found} />
                  <span className="h-5 w-px bg-line" />
                  <SummaryMetric label="Verified" value={result.verified} tone="good" />
                  <span className="h-5 w-px bg-line" />
                  <SummaryMetric label="Added" value={result.added.length} tone="accent" />
                </div>
              }
            >
              <div className="overflow-x-auto">
                <table className="w-full border-collapse text-sm">
                  <thead>
                    <tr className="border-b border-line bg-panel2/40 text-left">
                      <th className={TH}>Address</th>
                      <th className={TH}>Vendor</th>
                      <th className={TH}>HTTP</th>
                      <th className={TH}>Codec</th>
                      <th className={TH}>Resolution</th>
                      <th className={TH}>Verified</th>
                      <th className={cx(TH, "text-right")}>Action</th>
                    </tr>
                  </thead>
                  <tbody>
                    {devices.map((d) => {
                      const resolution =
                        d.width != null && d.height != null ? `${d.width}×${d.height}` : "—";
                      const adding = addingAddr === d.address;
                      return (
                        <tr
                          key={d.address}
                          className="border-b border-line/60 transition-colors last:border-0 hover:bg-panel2/50"
                        >
                          <td className="px-4 py-2.5 align-middle">
                            <div className="font-mono text-[13px] text-fg">
                              {d.address}
                              <span className="text-fg-muted">:{d.rtsp_port}</span>
                            </div>
                            <div className="mt-0.5 font-mono text-[10px] text-fg-muted">
                              {d.suggested_id}
                            </div>
                          </td>
                          <td className="px-4 py-2.5">
                            {d.vendor_guess === "unknown" ? (
                              <span className="font-mono text-xs text-fg-muted">unknown</span>
                            ) : (
                              <span className="font-mono text-xs uppercase tracking-wide text-fg-secondary">
                                {d.vendor_guess}
                              </span>
                            )}
                          </td>
                          <td className="px-4 py-2.5 font-mono text-xs">
                            {d.http_open ? (
                              <span className="text-fg-secondary" title={d.http_server ?? undefined}>
                                open{d.http_server ? ` · ${d.http_server}` : ""}
                              </span>
                            ) : (
                              <span className="text-fg-muted">—</span>
                            )}
                          </td>
                          <td className="px-4 py-2.5 font-mono text-xs text-fg-secondary">
                            {d.codec ?? "—"}
                          </td>
                          <td className="px-4 py-2.5 font-mono text-xs text-fg-secondary">
                            {resolution}
                          </td>
                          <td className="px-4 py-2.5">
                            <span className="inline-flex items-center gap-2">
                              <StatusLed
                                state={d.verified ? "recording" : "offline"}
                                pulse={false}
                              />
                              <span
                                className={cx(
                                  "font-mono text-[10px] font-semibold uppercase tracking-micro",
                                  d.verified ? "text-rec" : "text-fg-muted",
                                )}
                              >
                                {d.verified ? "Verified" : "Unverified"}
                              </span>
                            </span>
                          </td>
                          <td className="px-4 py-2.5 text-right">
                            {d.already_registered ? (
                              <span className="inline-flex items-center gap-2 font-mono text-[10px] font-semibold uppercase tracking-micro text-fg-secondary">
                                <StatusLed state="recording" pulse={false} />
                                Registered
                              </span>
                            ) : (
                              <Button
                                size="sm"
                                variant="primary"
                                disabled={adding || addingAddr !== null}
                                onClick={() => void handleAdd(d)}
                              >
                                {adding ? (
                                  <>
                                    <Spinner size={12} />
                                    Adding…
                                  </>
                                ) : (
                                  "Add"
                                )}
                              </Button>
                            )}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            </Panel>
          ))}
      </div>
    </div>
  );
}
