# Heldar Core — Operator Dashboard

A dark, dense, operator-focused control plane for the Heldar Core media kernel
(camera registry, RTSP ingest, segment recording, timeline, playback/clip/snapshot,
live view, health). Built with React + Vite + TypeScript, Tailwind CSS,
react-router-dom, and hls.js.

## Prerequisites

The Heldar Core API (Rust/Axum) must be running on `http://localhost:8000`.
For live view, MediaMTX should be running (the core returns HLS URLs that point at it).

## Run

```bash
npm install
npm run dev
```

Open http://localhost:5173. The Vite dev server proxies `/api`, `/media`, and
`/healthz` to the core on `:8000`, so the SPA talks to it with same-origin
relative paths (no CORS setup needed in dev). Live HLS streams are fetched
directly from the URL returned by the API (`hls_url`, served by MediaMTX).

## Scripts

- `npm run dev` — start the dev server (port 5173) with the API proxy.
- `npm run typecheck` — `tsc --noEmit`.
- `npm run build` — typecheck then produce a production build in `dist/`.
- `npm run preview` — preview the production build.

## Layout

- `src/lib/types.ts` — TypeScript mirror of the core's JSON contract.
- `src/lib/api.ts` — typed fetch client for every endpoint.
- `src/lib/usePoll.ts` — small polling hook used across pages.
- `src/components/` — `LiveView` (hls.js), `CameraCard`, `StatusBadge`,
  `Timeline`, `SystemBar`.
- `src/pages/` — `Dashboard`, `CameraDetail`, `AddCamera`.

## Production

`npm run build` emits static assets to `dist/`. Serve them behind the same origin
as the core (or a reverse proxy that forwards `/api`, `/media`, and `/healthz` to
it) so the relative API paths resolve.
