---
id: dashboard
title: Using the Dashboard
sidebar_label: Using the Dashboard
sidebar_position: 4
---

# Using the Dashboard

The dashboard is the React web UI (`apps/web`) the server hosts alongside the API — one binary, one URL
(see [Deploy](./deploy.md)). It's the same UI locally and [remotely](./remote-access.md); RBAC gates
what each role can do.

## Signing in

With `HELDAR_AUTH_ENABLED=false` (the LAN-appliance default) the dashboard opens straight to the camera
wall with full access. With auth on you get a **login** screen (username + password, plus an optional
Cloudflare Turnstile challenge); your session is an HttpOnly cookie and a sign-out control appears once
you're in. Five roles gate the controls you see — **admin**, **manager**, **guard**, **viewer**,
**integration** — mirroring the server-side RBAC (evidence-lock and config changes are manager+,
user/webhook admin is admin-only, viewers are read-only).

A **telemetry bar** across the top is always visible: core online/offline, recording count,
camera/recorder/segment counts, a storage gauge, uptime, and the clock.

## The pages

**Camera Wall** (`/`) — the multi-camera live grid and your home screen. Auto-responsive or a fixed
layout (1×1 … 4×4), aggregate stats (total / recording / offline / error), and a click-through to any
camera.

**Camera detail** (`/cameras/:id`) — the operator view for one camera: low-latency **live** video
(WebRTC/WHEP, HLS fallback) and a **recorded timeline** you click to scrub (1h / 6h / 24h / 3d). From
here you capture snapshots, export evidence clips (MP4), evidence-lock segments, trigger a manual
recording (event mode), toggle **Warm live** (keep this camera's live stream warm for instant playback;
the default is on-demand start with idle close), and manage the camera's **AI tasks** and **zones**
(below). Panels show recent
segments and events, health telemetry (FPS, bitrate, reconnects), PTZ, the recording schedule, and ANR
gap re-fill.

**Playback** (`/playback`) — synchronized multi-camera review: pick cameras and a time window, then
scrub them all on one master clock with shared play/pause/seek and speed (0.5× … 4×).

**Add camera** (`/cameras/new`) — register a camera. Pick a vendor (Hikvision / Dahua / Generic); the
RTSP URL is built from the vendor template (you supply address + credentials) or given explicitly for
Generic. Set the recording mode (continuous / scheduled / event / scheduled+event), stream, segment
length, retention, per-camera storage quota, warm live, and pre/post-roll for event modes.

**Discover** (`/discover`) — scan an IP range for RTSP cameras (and ONVIF), identify vendor/model from
the banner, verify credentials, and jump to Add Camera pre-filled (manager+).

**AI** (`/ai`) — the perception console: a read-only view of the per-camera frame **samplers** (state +
effective FPS) and the enabled **AI tasks**, plus the shared FPS-budget / backpressure model. You
create and tune tasks on the camera-detail page.

**Incidents** (`/incidents`) — evidence-case management: segments grouped by a free-form incident ID,
with play, evidence-lock/unlock, and retag-to-another-case (manager+). Evidence-locked footage is never
pruned by retention.

**Backup** (`/backup`) — backup **destinations** (local / SFTP / FTP / S3) and **policies** (cameras ×
destination × interval), a job ledger with status, and on-demand **archive export** (pick cameras + a
date range, download a `.zip`).

**Plugins** (`/plugins`) — the plugin store: browse the merged registry (bundled + remote), install or
uninstall modules and sidecars (manager+), and see verified/signed badges and per-plugin webhook
credentials.

**System** (`/system`) — health + observability: the storage panel (disk gauge, projected fill horizon,
write rate), the **recording-limit** controls (size cap + free-disk floor, tunable live — admin+),
the **database-limit** controls (the `heldar.db` metadata-DB cap + the one-time auto_vacuum
conversion — admin+), the **live-transcode** engine selector (software / VAAPI / NVENC, with detected
hardware encoders; applies to new live sessions immediately — admin+), per-camera health, the
recent-events feed, the immutable **audit log** (manager+), bulk camera config, and the webhooks
panel (admin+).

## Domain modules

Beyond the platform pages, the dashboard renders a **Modules** section from whatever apps the running
binary composes (`GET /api/v1/modules`) — the nav updates itself, no rebuild. The open generic apps add:

- **Entry** — the access-control console (plate authorization, the visitor/vehicle/watchlist registry,
  the guard confirm/reject workflow, reports).
- **Movement** — cross-camera movement intelligence (human-reviewed ReID candidates — with an
  optional, off-by-default `appearance_score` CLIP-similarity signal secondary to the plate anchor —
  restricted-zone breach incidents).
- **Search** — natural-language query over stored event facts, with a proof ladder over every answer.
  Search also includes semantic (text and image) retrieval over stored detection crops, surfaced as a
  Semantic tab (`POST /api/v1/search/semantic`), including zone-scoped queries ("red car in
the patio zone") via the tab's per-camera Zone select.

Each has an operator guide in the [Operate](../operate/index.md) hub. Sidecar plugins mount as
sandboxed iframes under `/m/{id}/` (see [Sidecar plugins](../develop/sidecar-plugins.md)).

## Zones & AI tasks

On a camera's detail page:

- **AI tasks** — create per-camera detection tasks (`task_type`, requested `fps`, sample `width`, and a
  free-form `config` the worker reads). Enabling the first task starts that camera's frame sampler; the
  reference [AI worker](../develop/ai-worker.md) pulls frames and posts detections back.
- **Zones** — draw polygon regions (normalized 0..1, matching detection boxes), set the `labels` that
  count, a `dwell_seconds` threshold, and a `severity`. Tracked detections crossing a zone raise
  `enter` / `exit` / `dwell` events with an evidence frame, which feed alerting.

## Related

- [Quickstart](./quickstart.md) — bring the stack up and add your first camera.
- [Remote Access](./remote-access.md) — the same dashboard from anywhere, in a browser.
- [Deploy](./deploy.md) — how the server serves the SPA (one binary, one URL).
