---
id: deploy
title: Deploy
sidebar_label: Deploy
sidebar_position: 2
---

# Deploy

Heldar is built to run as **one binary at one URL**. The composing server
(`heldar-server`, the `heldar-core` binary) serves the JSON API, the recorded
media, the metrics/health endpoints, and the built dashboard from a single
process.

## One binary, one URL

Build the dashboard, then point the server at it with `HELDAR_WEB_DIR`:

```bash
cd apps/web && npm install && npm run build      # writes apps/web/dist
# in .env:
HELDAR_WEB_DIR=./apps/web/dist
```

When `HELDAR_WEB_DIR` is set and the directory exists, the server serves the
SPA as a fallback. The explicit routes take precedence and the SPA is only a
fallback for everything else:

- `/api/*` - the JSON API.
- `/media/recordings`, `/media/clips`, `/media/snapshots`, `/media/playback`,
  `/media/archives` - static media served from the data dir.
- `/healthz` (liveness), `/readyz` (readiness, runs `SELECT 1`), `/metrics`
  (Prometheus exposition).
- everything else - the dashboard, with unknown client-routed paths falling back
  to `index.html` so deep links return `200`.

If `HELDAR_WEB_DIR` is unset it defaults to `apps/web/dist` relative to the
binary's working directory. When neither exists, the server runs API-only and
logs that the dashboard is not served.

## Ports

| Port | Service |
| --- | --- |
| 8000 | Heldar Core HTTP API + dashboard (`HELDAR_API_HOST` / `HELDAR_API_PORT`) |
| 5173 | Vite dev server (development only; not used in the single-binary deploy) |
| 8554 / 8888 / 8889 | MediaMTX RTSP / HLS / WebRTC |
| 9997 | MediaMTX control API (loopback) |

Live view is brokered through MediaMTX: camera credentials live only in the
gateway's path config and never reach the browser, which only ever sees the
non-credentialed HLS/WebRTC/RTSP URLs.

## Authentication

Auth and RBAC are **opt-in** via `HELDAR_AUTH_ENABLED` (default `false`).

- **`false`** - open API, suitable for a single-tenant LAN appliance. The admin
  surface is reachable without a token and acts as admin.
- **`true`** - every request needs a session (login) or an `X-API-Key`. Five
  roles are enforced (`admin` / `manager` / `guard` / `viewer` / `integration`)
  across capabilities, and every mutation is written to an immutable audit log.
  On first run with no users, an admin is seeded from the bootstrap env.

Sessions use an HttpOnly, SameSite=Strict cookie. Set
`HELDAR_AUTH_COOKIE_SECURE=true` behind TLS (keep `false` for plain-HTTP LAN or
overlay access). Tune the session lifetime with `HELDAR_SESSION_TTL_HOURS`
(default 12).

Set `HELDAR_AUTH_ENABLED=true` for any multi-user or networked deployment.

## Storage and the data dir

Heldar uses SQLite only (WAL journal, embedded migrations). The default URL is
`sqlite://./data/heldar.db`.

| Var | Default | Meaning |
| --- | --- | --- |
| `HELDAR_DATABASE_URL` | `sqlite://./data/heldar.db` | SQLite only; a non-`sqlite` URL is rejected at boot |
| `HELDAR_DATA_DIR` | `./data` | root for the DB and media subdirs |
| `HELDAR_RECORDINGS_DIR` / `CLIPS_DIR` / `SNAPSHOTS_DIR` / `FRAMES_DIR` | under `./data` | media roots (created at boot) |
| `HELDAR_MAX_RECORDINGS_GB` | `20` | soft footprint cap; oldest unlocked segments are pruned past it |
| `HELDAR_MIN_FREE_DISK_GB` | `5` | hard host-protection floor; prunes unlocked segments while free space is below it |

Recordings stay on the local disk and are served from there; nothing is pushed
to a cloud by default. Evidence-locked segments are never deleted by retention.

## CORS

`HELDAR_CORS_ORIGINS` controls cross-origin access. Empty or `*` allows all
origins; otherwise it restricts to the configured list (the default allows the
Vite dev server). In a single-binary deploy the dashboard is same-origin, so
CORS is mostly relevant when a separate frontend or integration calls the API.

## Operating a deployment

For sizing, commissioning, observability, and remote access, see the
[Operate](../operate/index.md) hub.
