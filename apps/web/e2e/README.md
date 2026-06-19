# Dashboard e2e (Playwright)

UI smoke + connectivity tests that run against a **running Heldar Core** (the core serves the
dashboard at one URL). They assert the operator dashboard shell loads, renders without console/page
errors, and reaches the core API. They need no cameras configured, so they pass against the synthetic
`scripts/validate.sh`-style stack or a real deployment.

## Run

```bash
cd apps/web
npm ci
npm run test:e2e:install     # one-time: download the chromium browser

# point at a running core (default http://localhost:8000)
npm run test:e2e
# or against another deployment:
HELDAR_E2E_BASE_URL=http://my-core:8000 npm run test:e2e
```

Bring up a local stack first (synthetic camera, no creds needed) with `scripts/validate.sh` /
`scripts/smoke_web.sh`, or run the real core. Then `npm run test:e2e`.

## Scope

`dashboard.spec.ts` covers the shell: API reachability, the camera wall loading clean, and the
primary nav routes (AI / incidents / system). Camera-dependent flows (live tile, recordings timeline,
detections in the UI) should go in a separate spec that seeds a synthetic camera via the API first, so
the suite stays hermetic and credential-free.
