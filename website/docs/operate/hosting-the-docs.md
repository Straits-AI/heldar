---
id: hosting-the-docs
title: Hosting the docs
sidebar_label: Hosting the docs
sidebar_position: 2
---

# Hosting the docs

The documentation site is a static [Docusaurus](https://docusaurus.io/) build
(the `website/` directory) hosted on **Cloudflare Workers** using
[Static Assets](https://developers.cloudflare.com/workers/static-assets/).
Cloudflare [recommends Workers over Pages](https://developers.cloudflare.com/workers/static-assets/migration-guides/migrate-from-pages/)
for new projects: "you should start with Workers ... going forward, all of our
investment, optimizations, and feature work will be dedicated to improving Workers."

It is an **assets-only Worker** (no server code): Cloudflare serves the pre-built
output directly. The config lives in `website/wrangler.jsonc`:

```jsonc
{
  "name": "heldar-docs",
  "compatibility_date": "2026-06-15",
  "assets": {
    "directory": "./build",
    "not_found_handling": "404-page"
  }
}
```

`not_found_handling: "404-page"` serves the Docusaurus-generated `404.html` for
missing pages (use `"single-page-application"` only for SPA-style client routing).
The site is served at the root of the Worker's domain, so `baseUrl` stays `/`.

## Deploy from your machine (you already have wrangler)

```bash
cd website
npm ci
npm run build          # -> website/build
wrangler deploy        # uploads ./build as an assets-only Worker
```

Local preview before deploying:

```bash
cd website
npm run build && wrangler dev   # serves the built site on http://localhost:8787
```

The first `wrangler deploy` creates the `heldar-docs` Worker and publishes it at
`https://heldar-docs.<your-subdomain>.workers.dev`.

## Deploy from CI (optional)

The repo ships `.github/workflows/cloudflare-workers.yml`: on every push to `main`
it builds the site and runs `wrangler deploy` via
[`cloudflare/wrangler-action`](https://github.com/cloudflare/wrangler-action). It
always builds (so the build is validated even without secrets) and only deploys
when the token is present. To enable it:

1. Create a scoped **API token** with the **Workers Scripts: Edit** permission
   (plus **Workers R2 / Account** read as the token UI prompts).
2. Add two **repository secrets** in GitHub settings:
   - `CLOUDFLARE_API_TOKEN` - the scoped token.
   - `CLOUDFLARE_ACCOUNT_ID` - your account ID (Workers & Pages overview, or the
     dashboard URL).

## Custom domain

In the Cloudflare dashboard open the `heldar-docs` Worker, go to **Settings ->
Domains & Routes -> Add -> Custom Domain**, and add your domain (for example
`docs.heldar.ai`). Cloudflare provisions the certificate automatically. Because
the Worker serves at the domain root, no `baseUrl` change is needed - it stays `/`.
(Set `url` in `website/docusaurus.config.ts` to that domain for correct canonical
and sitemap URLs.)

## Internationalization (i18n)

The site is multilingual: **English is the source** (`website/docs/`), with full translations under
`website/i18n/<locale>/` — currently Chinese (`zh-Hans`) and Spanish (`es`). A navbar language switcher
lets readers change locale. `npm run build` emits every locale into one output tree (`build/`,
`build/zh-Hans/`, `build/es/`) that the same Worker serves, so translations deploy with **no** extra
configuration. Any untranslated page falls back to English automatically.

**Add a language:** run `scripts/i18n-scaffold-locale.sh <locale>` (e.g. `fr`, `ja`, `pt-BR`) from the
repo root, then follow its printed steps — add the locale to `i18n.locales` + `localeConfigs` in
`docusaurus.config.ts`, add a `README.<locale>.md`, and translate the generated `i18n/<locale>/**` files.
The translator brief (fidelity rules + layout) is in `website/i18n/TRANSLATING.md`.
