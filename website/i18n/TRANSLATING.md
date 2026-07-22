# Translating the Heldar docs

The docs site is internationalized with Docusaurus i18n. **English (`docs/`) is the source of truth.**
Every other language lives under `i18n/<locale>/`. Any page you don't translate **falls back to English
automatically** — so partial translations are fine and never produce a broken page.

## Layout of a locale

```
i18n/<locale>/
├── code.json                                   # theme + homepage UI strings (ids like "home.hero.tagline")
├── docusaurus-theme-classic/
│   ├── navbar.json                             # navbar item labels
│   └── footer.json                             # footer titles, link labels, copyright
└── docusaurus-plugin-content-docs/
    ├── current.json                            # sidebar category labels (Getting Started, Concepts, …)
    └── current/**/*.md                          # a mirror of docs/ — the actual doc pages
```

- **JSON files:** translate only the `"message"` values. Never change a key, an `"id"`, or a
  `"description"` field. Keep the JSON valid.
- **Markdown files (`current/**`):** translate the prose; keep everything else verbatim (see rules below).

## Fidelity rules

**Translate:** prose, headings, list items, table cell text, admonition body text, blockquotes, image alt
text, and frontmatter `title` + `sidebar_label` + `description`.

**Keep verbatim (never translate):**
- fenced code blocks and their contents; inline `` `code` ``
- URLs, file paths, CLI commands, env vars (`HELDAR_*`), flags
- frontmatter `id`, `slug`, `sidebar_position`
- admonition type markers (`:::note`, `:::warning`, `:::tip`)
- HTML / JSX / MDX
- Markdown link **targets** — translate only the link *text* in `[text](target)`, keep the `target`
  (including any `#anchor`) as-is
- product / identifier names: `Heldar`, `@heldar/shell`, `heldar-kernel`, `heldar-{entry,movement,search}`,
  `MountKind`, `ModuleHost`, `DetectionConsumer`, `ONVIF`, `ISAPI`, `RTSP`, `WebRTC`, API route paths
  (`/api/v1/...`), `Straits AI`, `Docusaurus`, `Apache-2.0`

**Preserve structure exactly:** the same number of headings, code fences, and links; the same table shape.
Don't add a translator's note. Avoid introducing a bare `{` or `<` into prose — MDX treats them as code
and the build will fail.

## Preview + verify

```bash
cd website
npm run start -- --locale <locale>     # dev-serve just that locale
npm run build                          # builds ALL locales; fails on broken MDX/links
```

A green `npm run build` is the bar. Note: a translated heading changes its auto-generated anchor, so a
link that targets a heading `#anchor` may not resolve in the translated page (the page still loads). Keep
`#anchor` targets verbatim; fix only if a specific cross-reference matters.

## Adding a new language

Run `scripts/i18n-scaffold-locale.sh <locale>` from the repo root, then follow its printed steps (add the
locale to `docusaurus.config.ts`, add `README.<locale>.md`, translate `i18n/<locale>/**`).
