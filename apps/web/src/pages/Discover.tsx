import { useEffect, useState } from "react";
import type { ChangeEventHandler, FormEvent, ReactNode } from "react";
import { Link, useNavigate } from "react-router-dom";
import { api, ApiError } from "../lib/api";
import type {
  DiscoverOptions,
  DiscoverResponse,
  DiscoveredDevice,
  DiscoveredOnvifDevice,
  OnvifDiscoverResponse,
  Principal,
} from "../lib/types";
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

function RadarIcon({ className }: { className?: string }) {
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
      <circle cx="8" cy="8" r="6.5" />
      <path d="M8 8 12.6 4.4" />
      <path d="M8 4.2A3.8 3.8 0 0 1 11.8 8" />
    </svg>
  );
}

const TH = "px-4 py-2.5 font-mono text-[10px] font-medium uppercase tracking-micro text-fg-muted";

/** Pull name / hardware / location hints out of ONVIF scope URIs
 * (e.g. `onvif://www.onvif.org/name/Cam`, `.../hardware/DS-2CD`). */
function parseOnvifScopes(scopes: string[]): {
  name?: string;
  hardware?: string;
  location?: string;
} {
  const out: { name?: string; hardware?: string; location?: string } = {};
  for (const s of scopes) {
    const m = s.match(/onvif:\/\/www\.onvif\.org\/(name|hardware|location)\/(.+)$/i);
    if (!m) continue;
    const key = m[1].toLowerCase() as "name" | "hardware" | "location";
    let val: string;
    try {
      val = decodeURIComponent(m[2]);
    } catch {
      val = m[2];
    }
    val = val.replace(/_/g, " ").trim();
    if (val && !out[key]) out[key] = val;
  }
  return out;
}

/** Build the Add-Camera prefill link for a discovered ONVIF device. ONVIF gives no RTSP path, so the
 * vendor is left generic — the operator finishes the connection details in the form. */
function onvifPrefillHref(device: DiscoveredOnvifDevice): string {
  const parsed = parseOnvifScopes(device.scopes);
  const params = new URLSearchParams();
  if (device.address) params.set("address", device.address);
  const name = parsed.name ?? parsed.hardware ?? device.address ?? "";
  if (name) params.set("name", name);
  if (parsed.hardware) params.set("model", parsed.hardware);
  params.set("vendor", "generic");
  return `/cameras/new?${params.toString()}`;
}

export function Discover() {
  const [targets, setTargets] = useState("192.168.0.0/24");
  const [username, setUsername] = useState("admin");
  const [password, setPassword] = useState("");
  const [verify, setVerify] = useState(true);

  const [scanning, setScanning] = useState(false);
  const [addingAddr, setAddingAddr] = useState<string | null>(null);
  const [result, setResult] = useState<DiscoverResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const navigate = useNavigate();

  // ONVIF WS-Discovery is a manager+ action (it touches devices). When auth is off the server returns
  // the `system` admin principal; unauthenticated leaves the control gated off. Reads are never blocked.
  const [principal, setPrincipal] = useState<Principal | null>(null);
  useEffect(() => {
    let alive = true;
    api
      .me()
      .then((p) => {
        if (alive) setPrincipal(p);
      })
      .catch(() => {
        /* unauthenticated / auth off — leave principal null (ONVIF scan gated off) */
      });
    return () => {
      alive = false;
    };
  }, []);
  const canManage = principal?.role === "admin" || principal?.role === "manager";

  // ---- ONVIF (WS-Discovery) ----
  const [onvifScanning, setOnvifScanning] = useState(false);
  const [onvifResult, setOnvifResult] = useState<OnvifDiscoverResponse | null>(null);
  const [onvifError, setOnvifError] = useState<string | null>(null);

  async function handleOnvifDiscover() {
    setOnvifError(null);
    setOnvifScanning(true);
    try {
      const resp = await api.onvifDiscover();
      setOnvifResult(resp);
    } catch (err) {
      setOnvifError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setOnvifScanning(false);
    }
  }

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

        {/* ONVIF WS-Discovery (multicast) — separate from the RTSP port scan above. */}
        <Panel
          title="ONVIF Discovery"
          subtitle="Multicast WS-Discovery probe for ONVIF (Profile S) devices on the local segment."
          actions={
            <div className="flex items-center gap-3">
              {onvifScanning && (
                <span className="hidden items-center gap-2 font-mono text-[10px] uppercase tracking-micro text-fg-muted sm:flex">
                  <span className="relative flex h-1.5 w-1.5">
                    <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-accent opacity-70" />
                    <span className="relative inline-flex h-1.5 w-1.5 rounded-full bg-accent" />
                  </span>
                  Probing
                </span>
              )}
              <Button
                variant="primary"
                size="sm"
                disabled={onvifScanning || !canManage}
                onClick={() => void handleOnvifDiscover()}
              >
                {onvifScanning ? (
                  <>
                    <Spinner size={12} />
                    Probing…
                  </>
                ) : (
                  <>
                    <RadarIcon className="h-3.5 w-3.5" />
                    Discover ONVIF
                  </>
                )}
              </Button>
            </div>
          }
        >
          <p className="text-xs leading-relaxed text-fg-secondary">
            WS-Discovery finds ONVIF devices that advertise themselves on the network — no IP range
            required. It returns each device's service URLs and identity hints; ONVIF does not expose
            an RTSP path, so a discovered device pre-fills the Add Camera form (vendor{" "}
            <span className="font-mono text-fg">generic</span>) where you finish the connection
            details.
          </p>
          {!canManage && (
            <p className="mt-3 font-mono text-[11px] text-fg-muted">
              Manager role required to run ONVIF discovery.
            </p>
          )}

          {onvifError && (
            <div
              role="alert"
              className="mt-4 overflow-hidden rounded-md border border-danger/40 bg-danger/[0.07]"
            >
              <div className="flex items-start gap-3 px-4 py-3">
                <AlertIcon className="mt-0.5 h-4 w-4 shrink-0 text-danger" />
                <div className="min-w-0">
                  <div className="font-mono text-[10px] font-semibold uppercase tracking-micro text-danger">
                    ONVIF Discovery Error
                  </div>
                  <p className="mt-1 break-words font-mono text-xs leading-relaxed text-red-200">
                    {onvifError}
                  </p>
                </div>
              </div>
            </div>
          )}

          {onvifResult &&
            (onvifResult.devices.length === 0 ? (
              <p className="mt-4 rounded-md border border-dashed border-line bg-panel/40 px-4 py-6 text-center text-xs text-fg-secondary">
                No ONVIF devices answered the WS-Discovery probe.
              </p>
            ) : (
              <div className="mt-4 space-y-2.5">
                {onvifResult.devices.map((d, i) => {
                  const parsed = parseOnvifScopes(d.scopes);
                  const title = parsed.name ?? parsed.hardware ?? d.address ?? "ONVIF device";
                  const key = d.endpoint_reference ?? d.device_url ?? `${d.address ?? "dev"}-${i}`;
                  return (
                    <div
                      key={key}
                      className="flex items-start justify-between gap-4 rounded-md border border-line bg-panel2/40 px-3.5 py-3"
                    >
                      <div className="min-w-0 space-y-1.5">
                        <div className="flex flex-wrap items-baseline gap-x-2.5 gap-y-1">
                          <span className="font-display text-sm font-bold text-fg">{title}</span>
                          {d.address && (
                            <span className="font-mono text-[11px] text-fg-muted">{d.address}</span>
                          )}
                        </div>
                        <div className="flex flex-wrap gap-x-4 gap-y-1 font-mono text-[10px] uppercase tracking-micro text-fg-muted">
                          {parsed.hardware && (
                            <span>
                              Model{" "}
                              <span className="normal-case text-fg-secondary">
                                {parsed.hardware}
                              </span>
                            </span>
                          )}
                          {parsed.location && (
                            <span>
                              Loc{" "}
                              <span className="normal-case text-fg-secondary">
                                {parsed.location}
                              </span>
                            </span>
                          )}
                          {d.types && (
                            <span>
                              Type{" "}
                              <span className="normal-case text-fg-secondary">{d.types}</span>
                            </span>
                          )}
                        </div>
                        <div className="break-all font-mono text-[11px] text-fg-secondary">
                          {d.device_url}
                        </div>
                        {d.xaddrs.length > 1 && (
                          <div className="font-mono text-[10px] text-fg-muted">
                            +{d.xaddrs.length - 1} more transport address
                            {d.xaddrs.length - 1 === 1 ? "" : "es"}
                          </div>
                        )}
                      </div>
                      <div className="shrink-0">
                        <Button
                          size="sm"
                          variant="primary"
                          onClick={() => navigate(onvifPrefillHref(d))}
                        >
                          Use in Add Camera
                        </Button>
                      </div>
                    </div>
                  );
                })}
              </div>
            ))}
        </Panel>
      </div>
    </div>
  );
}
