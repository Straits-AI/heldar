---
id: webhooks
title: Webhooks & Integration
sidebar_label: Webhooks & Integration
sidebar_position: 3
---

# Webhooks & the integration substrate

Webhooks are how an external or **parent** application receives Heldar events in
near real time. A *webhook subscription* registers a URL, an event-type filter,
a minimum severity, and an optional signing secret; the kernel then POSTs every
matching event to that URL as signed JSON, with at-least-once delivery and
retry.

This is the **generic** integration machinery that lives in the open kernel.
Verticals build on the same substrate — they declare their own domain event
types and expose their own REST endpoints — without the kernel knowing they
exist (see [Verticals on the same substrate](#verticals-on-the-same-substrate)).

All paths below live under `/api/v1`. Managing subscriptions requires the
`manager` role (or `admin`); reads require any authenticated principal. When
`HELDAR_AUTH_ENABLED=false` (the default single-tenant LAN appliance mode) every
caller is a permissive principal, so the endpoints are open. Authenticated
deployments pass a key as `Authorization: Bearer <key>` or `X-API-Key: <key>`.

## Register a webhook

Create a subscription with `POST /api/v1/webhooks`:

```bash
curl -sS -X POST http://localhost:8000/api/v1/webhooks \
  -H 'Authorization: Bearer <api-key>' \
  -H 'Content-Type: application/json' \
  -d '{
        "name": "Ops Slack bridge",
        "url": "https://example.com/heldar/webhook",
        "event_types": ["zone_enter", "disk_pressure"],
        "min_severity": "warning",
        "secret": "whsec_a5f3…"
      }'
```

| Field          | Default | Meaning                                                                                       |
| -------------- | ------- | --------------------------------------------------------------------------------------------- |
| `name`         | —       | Human label (required).                                                                       |
| `url`          | —       | The `http(s)` POST target (required).                                                          |
| `event_types`  | `["*"]` | Exact-membership set of event types to deliver. `["*"]` (or omitted) matches **every** type.  |
| `min_severity` | `info`  | `info` (all), `warning` (warning + critical), or `critical` (critical only).                  |
| `secret`       | none    | HMAC-SHA256 signing key. When set, every delivery carries an `X-Heldar-Signature` header.     |
| `enabled`      | `true`  | Pause delivery without deleting the subscription.                                             |

The secret is **write-only**: it is never returned. Reads expose only a
`has_secret` boolean. On update (`PATCH /api/v1/webhooks/{id}`) the `secret`
field is three-state — omit it to keep the current secret, send `null`/`""` to
clear it, or send a value to replace it.

Other endpoints:

- `GET /api/v1/webhooks` — list subscriptions.
- `PATCH /api/v1/webhooks/{id}` — partial update (any absent field is unchanged).
- `DELETE /api/v1/webhooks/{id}` — remove a subscription.
- `POST /api/v1/webhooks/{id}/test` — deliver one synthetic signed event to the
  URL and return `{ ok, status, error }`.
- `GET /api/v1/webhooks/{id}/deliveries?limit=` — the recent delivery attempts
  (status, response code, timestamps).

Operators can do all of this without the API from the dashboard:
**System → Webhooks**.

## The delivered payload

Each delivery is a single JSON object — the event envelope — POSTed with these
headers:

| Header               | Value                                                                |
| -------------------- | -------------------------------------------------------------------- |
| `Content-Type`       | `application/json`                                                   |
| `X-Heldar-Event`     | The event type (e.g. `zone_enter`).                                  |
| `X-Heldar-Delivery`  | A unique id for this delivery attempt (use it to deduplicate).      |
| `X-Heldar-Timestamp` | Unix seconds when the request was sent.                             |
| `X-Heldar-Signature` | `sha256=<hex>` HMAC-SHA256 of the **raw body** — only when a secret is set. |

The body:

```json
{
  "id": "evt_9c1f…",
  "camera_id": "gate_a",
  "site_id": "hq",
  "event_type": "zone_enter",
  "severity": "warning",
  "timestamp": "2026-01-12T09:14:33.102Z",
  "payload": { "zone_id": "zone_7", "zone_name": "Loading bay", "track_id": "t-42", "label": "person" }
}
```

`camera_id` and `site_id` may be `null` for system-level events. `payload` is an
event-type-specific object — its shape is defined by whoever emits the event
(the kernel, an app, or an AI worker).

## Verify the signature

When a secret is configured, verify `X-Heldar-Signature` before trusting a
request. Compute HMAC-SHA256 over the **exact raw request bytes** — do not
re-serialize the parsed JSON, since key ordering and whitespace would differ and
the signature would not match. Always compare in constant time.

```python
import hashlib
import hmac

def verify(secret: str, raw_body: bytes, signature_header: str | None) -> bool:
    if not signature_header:
        return False
    expected = "sha256=" + hmac.new(secret.encode(), raw_body, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, signature_header)
```

```js
// Node.js
import { createHmac, timingSafeEqual } from "node:crypto";

function verify(secret, rawBody, signatureHeader) {
  if (!signatureHeader) return false;
  const expected = "sha256=" + createHmac("sha256", secret).update(rawBody).digest("hex");
  const a = Buffer.from(expected);
  const b = Buffer.from(signatureHeader);
  return a.length === b.length && timingSafeEqual(a, b);
}
```

## Delivery semantics

- **At-least-once.** Each subscription keeps its own delivery cursor (an event
  timestamp). Plan for duplicates: make your handler idempotent by deduplicating
  on the event `id` (or `X-Heldar-Delivery`).
- **No backlog replay.** A new subscription starts at "now", so adding one never
  floods you with historical events.
- **Acknowledge with 2xx.** Any `2xx` response counts as delivered. A non-2xx
  response, a timeout, or a connection error is a failure and is retried on the
  next cycle (the poll interval, minimum 5s).
- **Bounded retry.** An event is retried up to 5 times. After that the kernel
  gives up on that one event and advances the cursor past it, so a single bad
  endpoint can never wedge the queue. Every attempt — success or failure — is
  recorded in the delivery log (`GET /api/v1/webhooks/{id}/deliveries`).
- **Respond fast.** Return quickly (ack first, process async). Slow handlers
  count against the per-request timeout and look like failures.

## Event-type taxonomy

`GET /api/v1/events/types` returns the built-in event types with a one-line
description each — the same list that populates the dashboard's event-type
picker. Use it to drive a UI or to validate a filter. The built-in kernel +
reference-app types include:

| `event_type`         | Description                                                          |
| -------------------- | -------------------------------------------------------------------- |
| `camera_offline`     | A camera's recorder lost its RTSP connection.                        |
| `recorder_error`     | A recorder process errored or its segments went stale.               |
| `recording_gap`      | A hole was detected between consecutive recorded segments.           |
| `sampler_offline`    | An AI frame sampler for a camera went offline.                       |
| `retention_delete`   | Old segments were pruned by the retention sweeper.                   |
| `disk_pressure`      | Recording storage is under pressure (quota, size cap, or free-space floor). |
| `disk_smart_warning` | A SMART self-assessment reported a disk health warning.              |
| `raid_degraded`      | A Linux md/RAID array reported a degraded or down member.            |
| `zone_enter`         | A tracked detection entered a configured zone.                       |
| `zone_exit`          | A tracked detection left a configured zone.                          |
| `zone_dwell`         | A tracked detection dwelled inside a zone past its threshold.        |
| `entry_matched`      | Access control: an entry matched the registry and was authorized.    |
| `entry_exception`    | Access control: an entry needs operator review.                      |
| `entry_unmatched`    | Access control: an entry did not match any registry record.          |
| `entry_blocked`      | Access control: an entry matched a watchlist/blocklist and was denied. |

This list is **descriptive, not exhaustive**. Apps and AI workers emit their own
custom `event_type` strings on the same event log, and a webhook with
`event_types: ["*"]` delivers those too.

## Verticals on the same substrate

A vertical (a domain app built on the kernel) reuses this machinery rather than
reinventing it. It declares its own domain `event_type` strings — written to the
canonical event log through the kernel — and exposes its own REST endpoints; the
generic auth, event log, transactional outbox, and webhook delivery are all
inherited from the kernel. See [Build a module](./build-a-module.md) for the
seams.

Take a campus visitor-portal as the worked pattern. It integrates in two
directions on top of the kernel:

- **Inbound** — the portal calls the vertical's own REST endpoints (for example
  to pre-register a visitor), authenticated with a Heldar **API key** scoped to
  the `integration` role. The endpoints are the vertical's; the API key, RBAC,
  and audit log are the kernel's.
- **Outbound** — the parent app subscribes a webhook to the vertical's domain
  events (for example a `campus.*` event the vertical emits), filtered by event
  type and severity and verified with the same `X-Heldar-Signature` HMAC. No
  vertical-specific delivery code is needed — it is the same engine documented
  above.

So a vertical's integration story is just: *declare domain event types + expose
domain endpoints*, and the generic API-key auth (inbound) and webhook
subscriptions (outbound) come for free.
