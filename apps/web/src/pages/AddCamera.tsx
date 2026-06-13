import { useMemo, useState } from "react";
import type { FormEvent } from "react";
import { Link, useNavigate } from "react-router-dom";
import { api, ApiError } from "../lib/api";
import type { CameraCreate, RecordStream } from "../lib/types";

type Vendor = "hikvision" | "dahua" | "generic";

/** Mirror of apps/core/src/camera_url.rs path templates, for the live preview only. */
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

  return (
    <div className="mx-auto max-w-3xl px-4 py-5">
      <div className="mb-4 flex items-center gap-2 text-sm text-slate-400">
        <Link to="/" className="hover:text-slate-200">
          Cameras
        </Link>
        <span className="text-slate-600">/</span>
        <span className="text-slate-200">Add camera</span>
      </div>

      <form onSubmit={handleSubmit} className="space-y-4">
        <section className="panel p-4">
          <h2 className="mb-3 text-sm font-semibold text-slate-100">Identity</h2>
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            <div className="sm:col-span-2">
              <label className="label" htmlFor="name">
                Name *
              </label>
              <input
                id="name"
                className="input"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="Front Entrance"
                required
              />
            </div>
            <div>
              <label className="label" htmlFor="id">
                ID (slug, optional)
              </label>
              <input
                id="id"
                className="input font-mono"
                value={id}
                onChange={(e) => setId(e.target.value)}
                placeholder="auto from name"
              />
            </div>
            <div>
              <label className="label" htmlFor="site">
                Site ID (optional)
              </label>
              <input
                id="site"
                className="input"
                value={siteId}
                onChange={(e) => setSiteId(e.target.value)}
                placeholder="hq-lobby"
              />
            </div>
            <div>
              <label className="label" htmlFor="vendor">
                Vendor
              </label>
              <select
                id="vendor"
                className="input"
                value={vendor}
                onChange={(e) => setVendor(e.target.value as Vendor)}
              >
                <option value="hikvision">Hikvision</option>
                <option value="dahua">Dahua</option>
                <option value="generic">Generic / ONVIF</option>
              </select>
            </div>
            <div>
              <label className="label" htmlFor="model">
                Model (optional)
              </label>
              <input
                id="model"
                className="input"
                value={model}
                onChange={(e) => setModel(e.target.value)}
                placeholder="DS-2CD2087G2"
              />
            </div>
          </div>
        </section>

        <section className="panel p-4">
          <h2 className="mb-1 text-sm font-semibold text-slate-100">Connection by address</h2>
          <p className="mb-3 text-xs text-slate-500">
            {autoBuilds
              ? "RTSP URLs are built automatically from the address and credentials for this vendor."
              : "Generic cameras cannot auto-build a path — provide explicit stream URLs below."}
          </p>
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            <div>
              <label className="label" htmlFor="address">
                Address (host / IP)
              </label>
              <input
                id="address"
                className="input"
                value={address}
                onChange={(e) => setAddress(e.target.value)}
                placeholder="192.168.1.64"
              />
            </div>
            <div>
              <label className="label" htmlFor="port">
                RTSP port
              </label>
              <input
                id="port"
                type="number"
                className="input"
                value={rtspPort}
                min={1}
                max={65535}
                onChange={(e) => setRtspPort(Number(e.target.value))}
              />
            </div>
            <div>
              <label className="label" htmlFor="username">
                Username
              </label>
              <input
                id="username"
                className="input"
                value={username}
                onChange={(e) => setUsername(e.target.value)}
                autoComplete="off"
                placeholder="admin"
              />
            </div>
            <div>
              <label className="label" htmlFor="password">
                Password
              </label>
              <input
                id="password"
                type="password"
                className="input"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                autoComplete="new-password"
                placeholder="••••••••"
              />
            </div>
          </div>

          {autoBuilds && preview && (
            <div className="mt-3 rounded-md border border-line bg-ink px-3 py-2">
              <div className="stat-k mb-0.5">Auto-built record URL preview</div>
              <code className="break-all font-mono text-xs text-accent">{preview}</code>
            </div>
          )}
        </section>

        <section className="panel p-4">
          <h2 className="mb-1 text-sm font-semibold text-slate-100">Explicit stream URLs</h2>
          <p className="mb-3 text-xs text-slate-500">
            Optional override. {vendor === "generic" ? "Required for generic cameras." : "Takes precedence over auto-built URLs."}
          </p>
          <div className="space-y-3">
            <div>
              <label className="label" htmlFor="main-url">
                Main stream URL
              </label>
              <input
                id="main-url"
                className="input font-mono"
                value={mainStreamUrl}
                onChange={(e) => setMainStreamUrl(e.target.value)}
                placeholder="rtsp://user:pass@host:554/stream1"
              />
            </div>
            <div>
              <label className="label" htmlFor="sub-url">
                Sub stream URL
              </label>
              <input
                id="sub-url"
                className="input font-mono"
                value={subStreamUrl}
                onChange={(e) => setSubStreamUrl(e.target.value)}
                placeholder="rtsp://user:pass@host:554/stream2"
              />
            </div>
          </div>
        </section>

        <section className="panel p-4">
          <h2 className="mb-3 text-sm font-semibold text-slate-100">Recording</h2>
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
            <div>
              <label className="label" htmlFor="record-stream">
                Record stream
              </label>
              <select
                id="record-stream"
                className="input"
                value={recordStream}
                onChange={(e) => setRecordStream(e.target.value as RecordStream)}
              >
                <option value="main">Main</option>
                <option value="sub">Sub</option>
              </select>
            </div>
            <div>
              <label className="label" htmlFor="segment">
                Segment length (s)
              </label>
              <input
                id="segment"
                type="number"
                className="input"
                value={segmentSeconds}
                min={2}
                max={3600}
                onChange={(e) => setSegmentSeconds(Number(e.target.value))}
              />
            </div>
            <div>
              <label className="label" htmlFor="retention">
                Retention (hours)
              </label>
              <input
                id="retention"
                type="number"
                className="input"
                value={retentionHours}
                min={1}
                onChange={(e) => setRetentionHours(Number(e.target.value))}
              />
            </div>
          </div>
          <div className="mt-3 flex flex-wrap gap-5">
            <label className="flex items-center gap-2 text-sm text-slate-300">
              <input
                type="checkbox"
                className="h-4 w-4 accent-sky-400"
                checked={recordEnabled}
                onChange={(e) => setRecordEnabled(e.target.checked)}
              />
              Record enabled
            </label>
            <label className="flex items-center gap-2 text-sm text-slate-300">
              <input
                type="checkbox"
                className="h-4 w-4 accent-sky-400"
                checked={enabled}
                onChange={(e) => setEnabled(e.target.checked)}
              />
              Camera enabled
            </label>
          </div>
        </section>

        {error && (
          <div className="rounded-md border border-red-500/40 bg-red-950/30 px-3 py-2 text-sm text-red-300">
            {error}
          </div>
        )}

        <div className="flex items-center justify-end gap-2">
          <Link to="/" className="btn">
            Cancel
          </Link>
          <button type="submit" className="btn btn-primary" disabled={submitting}>
            {submitting ? "Creating…" : "Create camera"}
          </button>
        </div>
      </form>
    </div>
  );
}
