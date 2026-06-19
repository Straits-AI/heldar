// Heldar Core — Remote Access console.
//
// Manages Heldar's OWN kernel-managed WireGuard interface (the server `wireguard` feature): shows
// status and lets a manager enroll a device, returning a WireGuard `.conf` to import on a phone/laptop.
// When the feature isn't built/enabled the API 404s; we render setup guidance instead of an error.

import { useCallback, useEffect, useState } from "react";
import QRCode from "qrcode";
import { ApiError, api } from "../lib/api";
import type { EnrolledPeer, RemoteAccessStatus, RemotePeerInfo } from "../lib/types";
import { Button, EmptyState, Panel, SectionLabel, Spinner, StatusPill, cx } from "../components/ui";
import { fillPrivateKey, generateWgKeypair } from "../lib/wgkeys";

function handshakeLabel(unixSecs: number): string {
  if (!unixSecs) return "never connected";
  const ageS = Math.floor(Date.now() / 1000) - unixSecs;
  if (ageS < 0) return "just now";
  if (ageS < 90) return "connected now";
  if (ageS < 3600) return `${Math.floor(ageS / 60)}m ago`;
  if (ageS < 86400) return `${Math.floor(ageS / 3600)}h ago`;
  return `${Math.floor(ageS / 86400)}d ago`;
}

export function RemoteAccess() {
  const [status, setStatus] = useState<RemoteAccessStatus | null>(null);
  const [peers, setPeers] = useState<RemotePeerInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [disabled, setDisabled] = useState(false); // feature not built/enabled (API 404)
  const [error, setError] = useState<string | null>(null);

  const [name, setName] = useState("");
  const [enrolling, setEnrolling] = useState(false);
  const [enrolled, setEnrolled] = useState<EnrolledPeer | null>(null); // shown ONCE after enroll
  const [confQr, setConfQr] = useState<string | null>(null); // QR of the .conf (scan into WireGuard app)
  const [copied, setCopied] = useState(false);
  const [pairing, setPairing] = useState<{ qr: string; expiresAt: number } | null>(null);

  const load = useCallback(async () => {
    try {
      const s = await api.remoteAccess();
      setStatus(s);
      setDisabled(false);
      setPeers(s.up ? await api.listRemotePeers() : []);
      setError(null);
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) {
        setDisabled(true);
      } else {
        setError(e instanceof Error ? e.message : String(e));
      }
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  async function enroll() {
    const trimmed = name.trim();
    if (!trimmed) return;
    setEnrolling(true);
    setError(null);
    try {
      // Generate the keypair HERE so the private key never leaves this browser; send only the public key.
      const kp = generateWgKeypair();
      const peer = await api.enrollRemotePeer(trimmed, kp.publicKey);
      const filled = { ...peer, config: fillPrivateKey(peer.config, kp.privateKey) };
      setEnrolled(filled);
      setConfQr(await QRCode.toDataURL(filled.config, { margin: 1, width: 320 }));
      setName("");
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setEnrolling(false);
    }
  }

  async function startPairing() {
    setError(null);
    try {
      const t = await api.mintPairingToken();
      const payload = JSON.stringify({
        v: 1,
        token: t.token,
        apiBase: window.location.origin,
        expiresAt: t.expires_at,
      });
      setPairing({ qr: await QRCode.toDataURL(payload, { margin: 1, width: 320 }), expiresAt: t.expires_at });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function remove(publicKey: string) {
    try {
      await api.removeRemotePeer(publicKey);
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  function downloadConf(peer: EnrolledPeer) {
    const safe = peer.name.replace(/[^a-z0-9-_]/gi, "_").toLowerCase() || "heldar";
    const blob = new Blob([peer.config], { type: "text/plain" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `heldar-${safe}.conf`;
    a.click();
    URL.revokeObjectURL(url);
  }

  async function copyConf(peer: EnrolledPeer) {
    try {
      await navigator.clipboard.writeText(peer.config);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard blocked — the user can still download */
    }
  }

  if (loading) {
    return (
      <div className="flex items-center gap-2 p-6 text-fg-secondary">
        <Spinner /> Loading remote access…
      </div>
    );
  }

  if (disabled) {
    return (
      <div className="mx-auto max-w-2xl p-4">
        <Panel title="Remote Access" subtitle="Kernel-managed WireGuard (not enabled)">
          <p className="text-sm text-fg-secondary">
            This build does not have kernel-managed WireGuard enabled. To turn it on:
          </p>
          <ol className="mt-3 list-decimal space-y-2 pl-5 text-sm text-fg-secondary">
            <li>
              Build/run the server with the feature:{" "}
              <code className="text-fg">cargo run -p heldar-server --features wireguard</code>
            </li>
            <li>
              Grant the binary network privilege (one time):{" "}
              <code className="text-fg">sudo setcap cap_net_admin,cap_net_raw+eip ./heldar-core</code>
            </li>
            <li>
              Enable it and restart: <code className="text-fg">HELDAR_WG_MANAGED=true</code>
            </li>
          </ol>
          <p className="mt-3 text-xs text-fg-muted">
            The kernel then brings up its own isolated interface, auto-picking a non-conflicting name,
            subnet, and port — it never touches existing interfaces or routes.
          </p>
        </Panel>
      </div>
    );
  }

  const up = status?.up ?? false;

  return (
    <div className="mx-auto max-w-3xl space-y-4 p-4">
      <Panel
        title="Remote Access"
        subtitle="Heldar-managed WireGuard — private, peer-to-peer remote viewing"
        actions={<StatusPill state={up ? "recording" : "error"} label={up ? "up" : "down"} />}
      >
        {error && <p className="mb-3 text-sm text-danger">{error}</p>}
        {status && (
          <dl className="grid grid-cols-2 gap-x-6 gap-y-2 text-sm sm:grid-cols-3">
            <Field label="Interface" value={status.iface ?? "—"} />
            <Field label="Subnet" value={status.subnet ?? "—"} />
            <Field label="Listen port" value={status.port ? String(status.port) : "—"} />
            <Field label="Endpoint" value={status.endpoint ?? "—"} mono />
            <Field label="Devices" value={String(status.peers)} />
          </dl>
        )}
        <p className="mt-3 text-xs text-fg-muted">{status?.note}</p>
      </Panel>

      {up && (
        <Panel title="Devices" subtitle="WireGuard peers enrolled for remote viewing">
          <div className="mb-4 flex items-end gap-2">
            <label className="flex-1 text-sm">
              <SectionLabel>New device name</SectionLabel>
              <input
                className="mt-1 w-full rounded-md border border-line bg-canvas px-3 py-2 text-sm text-fg focus:border-accent focus:outline-none"
                placeholder="e.g. My phone"
                value={name}
                onChange={(e) => setName(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && void enroll()}
                data-testid="ra-name"
              />
            </label>
            <Button variant="primary" onClick={() => void enroll()} disabled={enrolling || !name.trim()} data-testid="ra-enroll">
              {enrolling ? "Enrolling…" : "Add device"}
            </Button>
          </div>

          <div className="mb-4 flex flex-wrap items-center gap-3">
            <Button size="sm" onClick={() => void startPairing()} data-testid="ra-pair">
              Pair a phone (Heldar app)
            </Button>
            <span className="text-xs text-fg-muted">
              One-time QR for the Heldar mobile app — auto-configures the tunnel (10-min expiry).
            </span>
          </div>
          {pairing && (
            <div className="mb-4 flex flex-col items-center gap-2 rounded-md border border-line bg-canvas/40 p-3">
              <img src={pairing.qr} alt="Heldar pairing QR" className="rounded-md bg-white p-2" width={200} height={200} />
              <span className="text-xs text-fg-muted">Scan with the Heldar app. Expires in ~10 minutes.</span>
              <button className="text-xs text-fg-muted hover:text-fg" onClick={() => setPairing(null)}>
                Dismiss
              </button>
            </div>
          )}

          {peers.length === 0 ? (
            <EmptyState title="No devices yet" hint="Enroll a device to view your cameras remotely." />
          ) : (
            <ul className="divide-y divide-line">
              {peers.map((p) => (
                <li key={p.public_key} className="flex items-center justify-between py-2 text-sm">
                  <div className="min-w-0">
                    <div className="truncate font-medium text-fg">{p.name}</div>
                    <div className="text-xs text-fg-muted">
                      {p.address} · {handshakeLabel(p.last_handshake)}
                    </div>
                  </div>
                  <Button variant="danger" size="sm" onClick={() => void remove(p.public_key)}>
                    Remove
                  </Button>
                </li>
              ))}
            </ul>
          )}
        </Panel>
      )}

      {enrolled && (
        <Panel
          title={`Configure “${enrolled.name}”`}
          subtitle="Import this into the WireGuard app on the device — shown only once"
          actions={
            <button
              className="text-xs text-fg-muted hover:text-fg"
              onClick={() => {
                setEnrolled(null);
                setConfQr(null);
              }}
            >
              Dismiss
            </button>
          }
        >
          <p className="mb-2 text-xs text-fg-muted">
            Address <code className="text-fg">{enrolled.address}</code>. On a phone, install WireGuard
            and <strong>scan the QR</strong> below (or import the config file). The private key was
            generated in your browser and never sent to the server. Recorded playback works immediately;
            for live video also point the media bases at the WireGuard host IP (see docs).
          </p>
          {confQr && (
            <div className="mb-3 flex justify-center">
              <img src={confQr} alt="WireGuard config QR" className="rounded-md bg-white p-2" width={220} height={220} />
            </div>
          )}
          <pre
            className={cx(
              "max-h-64 overflow-auto rounded-md border border-line bg-canvas p-3 text-xs text-fg-secondary",
            )}
            data-testid="ra-config"
          >
            {enrolled.config}
          </pre>
          <div className="mt-3 flex gap-2">
            <Button variant="primary" size="sm" onClick={() => downloadConf(enrolled)}>
              Download .conf
            </Button>
            <Button size="sm" onClick={() => void copyConf(enrolled)}>
              {copied ? "Copied" : "Copy"}
            </Button>
          </div>
        </Panel>
      )}
    </div>
  );
}

function Field({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div>
      <dt className="text-xs uppercase tracking-wide text-fg-muted">{label}</dt>
      <dd className={cx("text-fg", mono && "font-mono text-xs")}>{value}</dd>
    </div>
  );
}
