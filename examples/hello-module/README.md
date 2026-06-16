# hello-module — a reference Heldar sidecar plugin

The smallest complete Heldar plugin: an out-of-process service that Heldar Core reverse-proxies and
feeds events to. Zero dependencies — just `python3` and the standard library.

A **sidecar plugin** extends Heldar without being compiled into the binary. It speaks four endpoints:

| Endpoint | Who calls it | Purpose |
| --- | --- | --- |
| `GET /heldar/health` | the kernel (every 30s) | reachability probe; any `2xx` = healthy |
| `POST /heldar/events` | the kernel | signed event deliveries (`X-Heldar-Signature: sha256=…`) |
| `GET /` | the dashboard (via iframe) | the plugin's own UI, mounted at `/m/{id}/` |
| `GET /api/...` | the plugin's UI | the UI's data API, reached through `/m/{id}/api/...` |

When you **install** a plugin, Heldar mints it two secrets and a mount:

- a **scoped API key** (`HELDAR_API_KEY`) the sidecar uses to call kernel APIs back (least-privilege:
  `viewer` or `integration`),
- a **webhook subscription** that signs deliveries to `POST {base_url}/heldar/events` with a
  **webhook secret** (`HELDAR_WEBHOOK_SECRET`),
- a reverse-proxy route so the sidecar is single-origin with the console at `/m/{id}/`.

Uninstalling reverses all three.

## Run it

```bash
# 1. start the sidecar (defaults to :9123)
PORT=9123 python3 sidecar.py

# 2. register it with a running Heldar Core (admin bearer token; or with auth off, no header)
curl -sX POST http://localhost:8000/api/v1/modules \
  -H 'authorization: Bearer <ADMIN_TOKEN>' \
  -H 'content-type: application/json' \
  -d '{
    "id": "hello",
    "name": "Hello Plugin",
    "publisher": "Heldar",
    "base_url": "http://127.0.0.1:9123",
    "subscribes": ["*"],
    "role": "viewer"
  }'
# -> returns { "module": {...}, "api_key": "vok_…", "webhook_secret": "whsec_…" }  (shown ONCE)

# 3. give the sidecar its secret and restart so it verifies signatures
HELDAR_WEBHOOK_SECRET=whsec_… PORT=9123 python3 sidecar.py
```

Open the dashboard: a **Hello Plugin** entry appears in the nav (Modules section), and selecting it
shows the sidecar's own UI, proxied at `/m/hello/`. Trigger any event (e.g. a zone enter) and it
streams into the plugin's panel.

The same flow is available without curl: **Plugins → Install a sidecar plugin** in the dashboard.

## Make your own

Copy this directory and change the four handlers. Anything that can serve HTTP works — Python, Node,
Go, a container. The contract is the four endpoints above plus the manifest you register with. See
`website/docs/develop/build-a-module.md` for the full SPI.
