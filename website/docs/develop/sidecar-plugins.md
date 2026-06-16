---
id: sidecar-plugins
title: Sidecar Plugins
sidebar_label: Sidecar Plugins
sidebar_position: 2
---

# Sidecar plugins

A **sidecar plugin** extends Heldar without being compiled into the binary. It is an out-of-process
HTTP service — in any language, as a process or a container — that Heldar **installs at runtime**:
no rebuild, process/container isolation, least-privilege access. This is the path for third-party and
self-made modules. (For a tightly-integrated first-party Rust app that shares the kernel's database
and ingest hot path, use a [compiled-in app crate](./build-a-module.md) instead.)

The complete, runnable reference is
[`examples/hello-module`](https://github.com/Straits-AI/heldar/tree/main/examples/hello-module) — a
zero-dependency Python sidecar you can register and watch receive events in minutes.

## How it fits together

When you install a sidecar, Heldar does three reversible things:

1. **Mints a scoped API key** the sidecar uses to call kernel APIs back. The key is least-privilege:
   `viewer` (read) or `integration` (read + ingest). `admin`/`manager` are never granted to a plugin.
2. **Creates a webhook subscription** that signs and delivers the events you subscribe to.
3. **Reverse-proxies `/m/{id}/*`** to your service, so your UI and API are single-origin with the
   console (mounted as a micro-frontend — your UI does not ship in Heldar's bundle).

Uninstalling reverses all three: the key is revoked, the subscription deleted, the route removed.

```
  ┌────────────┐   webhook (signed events)    ┌─────────────────┐
  │            │ ───────────────────────────▶ │                 │
  │  Heldar    │   GET /heldar/health (probe)  │  your sidecar   │
  │  Core      │ ───────────────────────────▶ │  (any language) │
  │            │                               │                 │
  │  /m/{id}/* │ ◀──reverse-proxy── UI + API ─ │  :9123          │
  └────────────┘                               └─────────────────┘
        ▲   kernel API (Bearer <minted key>)          │
        └─────────────────────────────────────────────┘
```

## The four endpoints

Your sidecar serves these. Only the first two are required.

| Endpoint | Caller | Contract |
| --- | --- | --- |
| `GET /heldar/health` | kernel (every 30s) | return any `2xx` to be marked **healthy** |
| `POST /heldar/events` | kernel | event deliveries; verify `X-Heldar-Signature` (below) |
| `GET /` and your assets | dashboard iframe | your plugin UI, served at `/m/{id}/` |
| `GET /api/...` | your UI | your UI's data API, reached via `/m/{id}/api/...` |

Because the UI is mounted at `/m/{id}/`, make its asset and API requests **relative**
(`fetch("api/events")`, not `fetch("/api/events")`) so they resolve through the proxy.

## The manifest

You register by presenting a manifest. The same shape describes a compiled module (which returns it
from code); a sidecar sends it to `POST /api/v1/modules`:

```json
{
  "id": "visitor-portal",
  "name": "Visitor Portal",
  "version": "1.0.0",
  "publisher": "ACME Corp",
  "description": "Self-service visitor pre-registration",
  "base_url": "http://127.0.0.1:9123",
  "nav": [{ "path": "/visitor-portal", "label": "Visitors", "icon": "module" }],
  "subscribes": ["entry_matched", "entry_unmatched"],
  "role": "integration"
}
```

| Field | Meaning |
| --- | --- |
| `id` | stable slug; the `/m/{id}/` mount and nav key. Must not collide with a built-in module. |
| `base_url` | the origin Heldar reverse-proxies to (http/https). |
| `nav` | nav entries to surface. Omit for a single default entry at `/{id}`. `icon` falls back to a generic glyph. |
| `subscribes` | event types to receive (`["*"]` = all). See the [event taxonomy](./webhooks.md). |
| `role` | the minted key's role: `viewer` or `integration`. |

## Register

From the dashboard: **Plugins → Install a sidecar plugin**. Or via the API (admin):

```bash
curl -sX POST http://localhost:8000/api/v1/modules \
  -H 'authorization: Bearer <ADMIN_TOKEN>' \
  -H 'content-type: application/json' \
  -d @manifest.json
```

The response returns — **once** — the credentials to configure your sidecar with:

```json
{
  "module": { "id": "visitor-portal", "base_url": "http://127.0.0.1:9123", ... },
  "api_key": "vok_…",          // -> your HELDAR_API_KEY (calls to the kernel API)
  "webhook_secret": "whsec_…"  // -> your HELDAR_WEBHOOK_SECRET (verify deliveries)
}
```

Store both immediately; they are never shown again. Uninstall with
`DELETE /api/v1/modules/{id}` (or the **Uninstall** button).

## Receiving events

The kernel `POST`s each subscribed event to `{base_url}/heldar/events` with headers:

- `X-Heldar-Event` — the event type
- `X-Heldar-Delivery` — a unique delivery id
- `X-Heldar-Timestamp` — unix seconds
- `X-Heldar-Signature` — `sha256=<hex HMAC-SHA256(webhook_secret, raw_body)>`

**Always verify the signature** over the exact request bytes:

```python
import hashlib, hmac
def verify(raw: bytes, header: str, secret: str) -> bool:
    expected = "sha256=" + hmac.new(secret.encode(), raw, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, header)
```

Return `2xx` to acknowledge. Non-2xx (or a timeout) is retried by the at-least-once delivery engine,
so make your handler idempotent on `X-Heldar-Delivery`.

## Calling the kernel back

Use the minted key as a bearer token against any kernel API your role permits:

```bash
curl http://localhost:8000/api/v1/events \
  -H "authorization: Bearer $HELDAR_API_KEY"
```

`integration` keys may also POST detections into the ingest pipeline; `viewer` keys are read-only.

## Security model

- Plugins are **admin-installed** and run **out-of-process** — isolate them as you would any service
  (container, network policy, a non-loopback `base_url` only when you trust the path).
- The console **never forwards your session cookie** to a sidecar; the sidecar authenticates to the
  kernel only with its own minted key.
- The plugin UI iframe is sandboxed (`allow-scripts allow-same-origin allow-forms allow-popups`); it
  cannot navigate or act on the top console frame.
- Uninstalling fully revokes the key + subscription, so a removed plugin keeps no standing access.
