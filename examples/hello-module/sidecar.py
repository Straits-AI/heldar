#!/usr/bin/env python3
"""Heldar example sidecar plugin — a zero-dependency reference module.

This is the smallest complete Heldar plugin: an out-of-process HTTP service that Heldar Core
reverse-proxies at ``/m/{id}/`` and feeds events to. It implements the four endpoints every sidecar
exposes, using only the Python standard library so it runs anywhere with `python3`:

    GET  /heldar/health   -> 200  (the kernel's health probe; anything 2xx = healthy)
    POST /heldar/events   -> 200  (signed event deliveries land here; HMAC-SHA256 verified)
    GET  /                -> the plugin's own UI (mounted as a micro-frontend in the dashboard)
    GET  /api/events      -> the UI's data API (reached via /m/{id}/api/events through the proxy)

Register it from the dashboard (Plugins -> Install) or via the API, then copy the minted
HELDAR_WEBHOOK_SECRET (and HELDAR_API_KEY, if you call the kernel back) into this process's env.

    # 1. run the sidecar
    PORT=9123 python3 sidecar.py
    # 2. register it (admin token, or with auth off):
    curl -sX POST localhost:8000/api/v1/modules -H 'content-type: application/json' -d '{
      "id":"hello","name":"Hello Plugin","base_url":"http://127.0.0.1:9123",
      "subscribes":["*"],"role":"viewer" }'
    # 3. paste the returned webhook_secret into HELDAR_WEBHOOK_SECRET and restart, then open
    #    the dashboard -> Hello Plugin (the UI is proxied at /m/hello/).
"""

import hashlib
import hmac
import json
import os
from collections import deque
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(os.environ.get("PORT", "9123"))
# The HMAC key Heldar signs deliveries with (returned once at registration). If unset, signatures are
# not verified — fine for a local demo, but ALWAYS set it in production so spoofed events are rejected.
WEBHOOK_SECRET = os.environ.get("HELDAR_WEBHOOK_SECRET", "")
# The scoped API key Heldar minted for this plugin, for calling kernel APIs back (unused in this demo).
API_KEY = os.environ.get("HELDAR_API_KEY", "")

# Last 50 received events, newest first (in-memory; a real plugin persists what it needs).
EVENTS: deque = deque(maxlen=50)


def verify_signature(raw: bytes, header: str) -> bool:
    """True if `X-Heldar-Signature: sha256=<hex>` matches HMAC-SHA256(secret, raw)."""
    if not WEBHOOK_SECRET:
        return True  # no secret configured -> accept (demo only)
    if not header or not header.startswith("sha256="):
        return False
    expected = hmac.new(WEBHOOK_SECRET.encode(), raw, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, header[len("sha256=") :])


INDEX_HTML = """<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Hello Plugin</title>
<style>
  :root { color-scheme: dark; }
  body { margin:0; font:14px/1.5 ui-sans-serif,system-ui,sans-serif; background:#09090b; color:#e4e4e7; }
  .wrap { max-width:760px; margin:0 auto; padding:28px 20px; }
  h1 { font-size:20px; margin:0 0 2px; letter-spacing:-.01em; }
  .tag { font:600 10px ui-monospace,monospace; text-transform:uppercase; letter-spacing:.12em; color:#f59e0b; }
  .sub { color:#a1a1aa; margin:4px 0 20px; }
  .card { border:1px solid #26282e; background:#0f0f12; border-radius:12px; padding:14px 16px; margin-bottom:12px; }
  .muted { color:#71717a; font:600 10px ui-monospace,monospace; text-transform:uppercase; letter-spacing:.1em; }
  ul { list-style:none; margin:8px 0 0; padding:0; }
  li { border-top:1px solid #1c1d22; padding:8px 0; font:12px ui-monospace,monospace; color:#d4d4d8; }
  li:first-child { border-top:0; }
  .ev { color:#fbbf24; }
  .empty { color:#71717a; padding:10px 0; }
</style></head>
<body><div class="wrap">
  <div class="tag">Sidecar plugin</div>
  <h1>Hello Plugin</h1>
  <div class="sub">A reference Heldar module. This whole page is served by an out-of-process Python
    sidecar and proxied into the console at <code>/m/hello/</code>.</div>
  <div class="card">
    <div class="muted">Live events received via webhook</div>
    <ul id="events"><li class="empty">Waiting for events…</li></ul>
  </div>
</div>
<script>
  async function tick() {
    try {
      // Relative path -> resolves to /m/hello/api/events, proxied back to this sidecar.
      const r = await fetch("api/events", { credentials: "same-origin" });
      const evs = await r.json();
      const ul = document.getElementById("events");
      if (!evs.length) { ul.innerHTML = '<li class="empty">No events yet.</li>'; return; }
      ul.innerHTML = evs.map(e =>
        `<li><span class="ev">${e.event_type || "event"}</span> · ${e.received_at}` +
        (e.severity ? ` · ${e.severity}` : "") + `</li>`).join("");
    } catch (_) { /* kernel not reachable yet */ }
  }
  tick(); setInterval(tick, 3000);
</script>
</body></html>"""


class Handler(BaseHTTPRequestHandler):
    server_version = "HelloSidecar/1.0"

    def _send(self, code: int, body: bytes, ctype: str = "application/json") -> None:
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:  # noqa: N802 (stdlib naming)
        path = self.path.split("?", 1)[0]
        if path == "/heldar/health":
            self._send(200, b'{"status":"ok"}')
        elif path in ("/", ""):
            self._send(200, INDEX_HTML.encode(), "text/html; charset=utf-8")
        elif path == "/api/events":
            self._send(200, json.dumps(list(EVENTS)).encode())
        else:
            self._send(404, b'{"error":"not found"}')

    def do_POST(self) -> None:  # noqa: N802
        path = self.path.split("?", 1)[0]
        if path != "/heldar/events":
            self._send(404, b'{"error":"not found"}')
            return
        length = int(self.headers.get("Content-Length", "0"))
        raw = self.rfile.read(length) if length else b""
        if not verify_signature(raw, self.headers.get("X-Heldar-Signature", "")):
            self._send(401, b'{"error":"bad signature"}')
            return
        try:
            payload = json.loads(raw or b"{}")
        except json.JSONDecodeError:
            payload = {}
        EVENTS.appendleft(
            {
                "event_type": self.headers.get("X-Heldar-Event") or payload.get("event_type"),
                "severity": payload.get("severity"),
                "received_at": datetime.now(timezone.utc).strftime("%H:%M:%S"),
            }
        )
        self._send(200, b'{"ok":true}')

    def log_message(self, *_args) -> None:  # quieter console
        pass


if __name__ == "__main__":
    print(f"hello-module sidecar on :{PORT}  (secret {'set' if WEBHOOK_SECRET else 'UNSET'})")
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
