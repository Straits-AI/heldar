import { useMemo, useState } from "react";
import type { ChangeEventHandler, FormEvent, ReactNode } from "react";
import { Link, useNavigate } from "react-router-dom";
import { api, ApiError } from "../lib/api";
import type { CameraCreate, RecordStream } from "../lib/types";
import {
  Button,
  Field,
  Input,
  Panel,
  SectionLabel,
  Select,
  Spinner,
} from "../components/ui";

type Vendor = "hikvision" | "dahua" | "generic";

/** Mirror of crates/heldar-kernel/src/camera_url.rs path templates, for the live preview only. */
function buildPreviewUrl(
  vendor: Vendor,
  address: string,
  port: number,
  username: string,
  hasPassword: boolean,
  stream: RecordStream,
): string | null {
  const host = address.trim();
  if (!host) return null;
  const creds = username.trim()
    ? `${username.trim()}:${hasPassword ? "••••" : ""}@`
    : "";
  if (vendor === "hikvision") {
    return `rtsp://${creds}${host}:${port}/Streaming/Channels/${stream === "sub" ? "102" : "101"}`;
  }
  if (vendor === "dahua") {
    return `rtsp://${creds}${host}:${port}/cam/realmonitor?channel=1&subtype=${stream === "sub" ? "1" : "0"}`;
  }
  return null;
}

/* ---------------------------------------------------------------- */
/* Small inline, line-based icons                                   */
/* ---------------------------------------------------------------- */
function InfoIcon({ className }: { className?: string }) {
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
      <circle cx="8" cy="8" r="6.5" />
      <path d="M8 7.4v3.4" />
      <path d="M8 5.2h.01" />
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

export function AddCamera() {
  const navigate = useNavigate();

  const [name, setName] = useState("");
  const [id, setId] = useState("");
  const [siteId, setSiteId] = useState("");
  const [vendor, setVendor] = useState<Vendor>("hikvision");
  const [model, setModel] = useState("");

  const [address, setAddress] = useState("");
  const [rtspPort, setRtspPort] = useState(554);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");

  const [mainStreamUrl, setMainStreamUrl] = useState("");
  const [subStreamUrl, setSubStreamUrl] = useState("");

  const [recordStream, setRecordStream] = useState<RecordStream>("main");
  const [segmentSeconds, setSegmentSeconds] = useState(60);
  const [retentionHours, setRetentionHours] = useState(24);
  const [recordEnabled, setRecordEnabled] = useState(true);
  const [enabled, setEnabled] = useState(true);

  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const autoBuilds = vendor === "hikvision" || vendor === "dahua";
  const preview = useMemo(
    () =>
      autoBuilds
        ? buildPreviewUrl(vendor, address, rtspPort, username, password.length > 0, recordStream)
        : null,
    [autoBuilds, vendor, address, rtspPort, username, password, recordStream],
  );

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    setError(null);

    if (!name.trim()) {
      setError("Name is required.");
      return;
    }

    const body: CameraCreate = {
      name: name.trim(),
      vendor,
      record_stream: recordStream,
      record_enabled: recordEnabled,
      enabled,
      segment_seconds: segmentSeconds,
      retention_hours: retentionHours,
    };
    if (id.trim()) body.id = id.trim();
    if (siteId.trim()) body.site_id = siteId.trim();
    if (model.trim()) body.model = model.trim();
    if (address.trim()) body.address = address.trim();
    if (rtspPort) body.rtsp_port = rtspPort;
    if (username.trim()) body.username = username.trim();
    if (password) body.password = password;
    if (mainStreamUrl.trim()) body.main_stream_url = mainStreamUrl.trim();
    if (subStreamUrl.trim()) body.sub_stream_url = subStreamUrl.trim();

    setSubmitting(true);
    try {
      const cam = await api.createCamera(body);
      navigate(`/cameras/${encodeURIComponent(cam.id)}`);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : String(err));
      setSubmitting(false);
    }
  }

  const vendorLabel = vendor === "hikvision" ? "Hikvision" : "Dahua";

  return (
    <div className="mx-auto max-w-3xl px-4 py-6 sm:px-6">
      <div className="stagger space-y-5">
        {/* Breadcrumb + page header */}
        <div>
          <nav className="flex items-center gap-2 font-mono text-[10px] uppercase tracking-micro">
            <Link to="/" className="text-fg-muted transition-colors hover:text-fg-secondary">
              Wall
            </Link>
            <span className="text-line">/</span>
            <span className="text-accent">Add Camera</span>
          </nav>
          <div className="mt-3">
            <SectionLabel>Registry</SectionLabel>
            <h1 className="mt-1 font-display text-xl font-bold tracking-tight text-fg">
              Add Camera
            </h1>
            <p className="mt-1 max-w-2xl text-sm text-fg-secondary">
              Register an RTSP camera. Hikvision and Dahua build their stream URLs automatically;
              generic devices need explicit URLs.
            </p>
          </div>
        </div>

        <form onSubmit={handleSubmit} className="space-y-5">
          {/* Identity */}
          <Panel title="Identity" subtitle="How this camera is named and labelled.">
            <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
              <div className="sm:col-span-2">
                <Field
                  label={
                    <>
                      Name <span className="text-accent">*</span>
                    </>
                  }
                  htmlFor="name"
                >
                  <Input
                    id="name"
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                    placeholder="Front Entrance"
                    required
                  />
                </Field>
              </div>
              <Field label="ID (slug, optional)" htmlFor="id">
                <Input
                  id="id"
                  value={id}
                  onChange={(e) => setId(e.target.value)}
                  placeholder="auto from name"
                />
              </Field>
              <Field label="Site ID (optional)" htmlFor="site">
                <Input
                  id="site"
                  value={siteId}
                  onChange={(e) => setSiteId(e.target.value)}
                  placeholder="hq-lobby"
                />
              </Field>
              <Field label="Vendor" htmlFor="vendor">
                <Select
                  id="vendor"
                  value={vendor}
                  onChange={(e) => setVendor(e.target.value as Vendor)}
                >
                  <option value="hikvision">Hikvision</option>
                  <option value="dahua">Dahua</option>
                  <option value="generic">Generic / ONVIF</option>
                </Select>
              </Field>
              <Field label="Model (optional)" htmlFor="model">
                <Input
                  id="model"
                  value={model}
                  onChange={(e) => setModel(e.target.value)}
                  placeholder="DS-2CD2087G2"
                />
              </Field>
            </div>
          </Panel>

          {/* Connection by address */}
          <Panel
            title="Connection by Address"
            subtitle={
              autoBuilds
                ? "RTSP URLs are built automatically from the address and credentials."
                : "Generic cameras cannot auto-build a path — provide explicit stream URLs below."
            }
          >
            {autoBuilds ? (
              <div className="mb-4 flex items-start gap-2.5 rounded-md border border-line bg-panel2/60 px-3 py-2.5">
                <InfoIcon className="mt-0.5 h-3.5 w-3.5 shrink-0 text-accent" />
                <p className="text-xs leading-relaxed text-fg-secondary">
                  For <span className="text-fg">{vendorLabel}</span>, the username and password are
                  embedded directly into the auto-built RTSP URL — e.g.{" "}
                  <code className="text-accent">
                    rtsp://user:••••@host:{rtspPort}
                    {vendor === "hikvision"
                      ? "/Streaming/Channels/101"
                      : "/cam/realmonitor?channel=1&subtype=0"}
                  </code>
                  . No separate stream URL is required.
                </p>
              </div>
            ) : (
              <div className="mb-4 flex items-start gap-2.5 rounded-md border border-line bg-panel2/60 px-3 py-2.5">
                <InfoIcon className="mt-0.5 h-3.5 w-3.5 shrink-0 text-fg-muted" />
                <p className="text-xs leading-relaxed text-fg-secondary">
                  Generic / ONVIF devices have no known URL template. Fill in the explicit stream
                  URLs in the next panel.
                </p>
              </div>
            )}

            <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
              <Field label="Address (host / IP)" htmlFor="address">
                <Input
                  id="address"
                  value={address}
                  onChange={(e) => setAddress(e.target.value)}
                  placeholder="192.168.1.64"
                />
              </Field>
              <Field label="RTSP port" htmlFor="port">
                <Input
                  id="port"
                  type="number"
                  value={rtspPort}
                  min={1}
                  max={65535}
                  onChange={(e) => setRtspPort(Number(e.target.value))}
                />
              </Field>
              <Field label="Username" htmlFor="username">
                <Input
                  id="username"
                  value={username}
                  onChange={(e) => setUsername(e.target.value)}
                  autoComplete="off"
                  placeholder="admin"
                />
              </Field>
              <Field label="Password" htmlFor="password">
                <Input
                  id="password"
                  type="password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  autoComplete="new-password"
                  placeholder="••••••••"
                />
              </Field>
            </div>

            {autoBuilds && preview && (
              <div className="mt-4 rounded-md border border-line bg-canvas px-3 py-2.5">
                <SectionLabel>Auto-built record URL</SectionLabel>
                <code className="mt-1.5 block break-all font-mono text-xs text-accent">
                  {preview}
                </code>
              </div>
            )}
          </Panel>

          {/* Explicit stream URLs */}
          <Panel
            title="Explicit Stream URLs"
            subtitle={
              vendor === "generic"
                ? "Required for generic cameras."
                : "Optional override — takes precedence over auto-built URLs."
            }
          >
            <div className="space-y-4">
              <Field label="Main stream URL" htmlFor="main-url">
                <Input
                  id="main-url"
                  value={mainStreamUrl}
                  onChange={(e) => setMainStreamUrl(e.target.value)}
                  placeholder="rtsp://user:pass@host:554/stream1"
                />
              </Field>
              <Field label="Sub stream URL" htmlFor="sub-url">
                <Input
                  id="sub-url"
                  value={subStreamUrl}
                  onChange={(e) => setSubStreamUrl(e.target.value)}
                  placeholder="rtsp://user:pass@host:554/stream2"
                />
              </Field>
            </div>
          </Panel>

          {/* Recording */}
          <Panel title="Recording" subtitle="Segmenting and retention policy for this camera.">
            <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
              <Field label="Record stream" htmlFor="record-stream">
                <Select
                  id="record-stream"
                  value={recordStream}
                  onChange={(e) => setRecordStream(e.target.value as RecordStream)}
                >
                  <option value="main">Main</option>
                  <option value="sub">Sub</option>
                </Select>
              </Field>
              <Field label="Segment length (s)" htmlFor="segment">
                <Input
                  id="segment"
                  type="number"
                  value={segmentSeconds}
                  min={2}
                  max={3600}
                  onChange={(e) => setSegmentSeconds(Number(e.target.value))}
                />
              </Field>
              <Field label="Retention (hours)" htmlFor="retention">
                <Input
                  id="retention"
                  type="number"
                  value={retentionHours}
                  min={1}
                  onChange={(e) => setRetentionHours(Number(e.target.value))}
                />
              </Field>
            </div>
            <div className="mt-4 flex flex-wrap gap-6 border-t border-line pt-4">
              <Checkbox
                checked={recordEnabled}
                onChange={(e) => setRecordEnabled(e.target.checked)}
              >
                Record enabled
              </Checkbox>
              <Checkbox checked={enabled} onChange={(e) => setEnabled(e.target.checked)}>
                Camera enabled
              </Checkbox>
            </div>
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
                    Cannot Create Camera
                  </div>
                  <p className="mt-1 break-words font-mono text-xs leading-relaxed text-red-200">
                    {error}
                  </p>
                </div>
              </div>
            </div>
          )}

          {/* Actions */}
          <div className="flex items-center justify-end gap-2">
            <Link
              to="/"
              className="inline-flex items-center justify-center gap-1.5 rounded-md border border-line bg-raised px-3.5 py-2 text-sm font-medium text-fg transition-colors duration-150 hover:border-[#34373e] hover:bg-[#23262c]"
            >
              Cancel
            </Link>
            <Button type="submit" variant="primary" disabled={submitting}>
              {submitting ? (
                <>
                  <Spinner size={14} />
                  Creating…
                </>
              ) : (
                "Create Camera"
              )}
            </Button>
          </div>
        </form>
      </div>
    </div>
  );
}
