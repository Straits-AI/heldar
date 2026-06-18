# Heldar Core — Design System

**Direction:** Operations console / SOC. Industrial, utilitarian, dark, signal-driven.
A control-room surface, not a marketing site. Dense, legible, fast, calm under load.
Always dark — there is no light theme.

This document is the contract for Phase 2 (page restyle). Import primitives from
`src/components/ui.tsx` and use the tokens below. Do not modify `lib/types.ts` or `lib/api.ts`.

---

## Color tokens

Defined in `tailwind.config.js` (Tailwind utilities) and mirrored as CSS variables in `index.css`.

### Surfaces

| Token              | Hex       | Use                                   |
| ------------------ | --------- | ------------------------------------- |
| `canvas` / `ink`   | `#09090b` | Page background (near-black)          |
| `panel`            | `#121316` | Primary panel surface                 |
| `panel2`           | `#16181c` | Secondary / inset panel surface       |
| `raised`           | `#1c1f24` | Raised controls (buttons, hover)      |
| `line` / `hairline`| `#26282e` | Hairline borders (1px)                |

### Text

| Token           | Hex       | Use            |
| --------------- | --------- | -------------- |
| `fg`            | `#f4f4f5` | Primary text   |
| `fg-secondary`  | `#a1a1aa` | Secondary text |
| `fg-muted`      | `#71717a` | Muted / micro labels |

Use Tailwind: `text-fg`, `text-fg-secondary`, `text-fg-muted`, `bg-panel`, `border-line`, etc.

### Accent (brand / active nav / primary buttons / focus)

Signal **amber**: `accent` `#f59e0b` (DEFAULT), `accent-soft` `#fbbf24`, `accent-deep` `#b45309`,
`accent-ink` `#1a1206` (text on amber fills).

### Semantic status LEDs (camera state)

These are **LEDs** — small glowing dots — not flat chips.

| State        | Token          | Hex       | Behavior        |
| ------------ | -------------- | --------- | --------------- |
| recording / live | `rec` / `live` | `#10b981` | glowing, pulses |
| connecting   | `connecting`   | `#fbbf24` | glowing, pulses |
| offline      | `offline`      | `#71717a` | static          |
| error        | `danger`       | `#ef4444` | static          |
| disabled     | `disabled`     | `#3f3f46` | static          |
| unknown      | `unknown`      | `#52525b` | static          |

---

## Typography

Loaded from Google Fonts in `index.html`.

- **Display / headings / wordmark:** `Archivo` (700/800) — `font-display`. Wide tracking for wordmark.
- **Body / UI:** `Hanken Grotesk` (400/500/600) — `font-sans` (the body default).
- **Mono / ALL data** (timestamps, ids, counts, telemetry, status labels, code, IPs):
  `JetBrains Mono` — `font-mono`. Also auto-applied to `code/kbd/samp/pre` and `[data-num]`.
- **Micro-labels:** UPPERCASE mono, `tracking-micro` (0.12em), small, `text-fg-muted`.
  Use `<SectionLabel>` or the `.micro-label` class.

Letter-spacing tokens: `tracking-micro` (0.12em), `tracking-wide2` (0.18em), `tracking-wordmark` (0.3em).

---

## Atmosphere & motion

- **Atmosphere:** a fixed, ~3–6% opacity layer (`.app-atmosphere`, rendered once by `AppShell`):
  faint grid + horizontal scanlines + soft radial vignette + a whisper of amber top-glow.
  `pointer-events: none`, sits behind content (`z-0`); content is `z-10`.
- **Page-load stagger:** add `.stagger` to a container — direct children fade/slide-up in
  sequence (`animation: rise`, staggered `animation-delay`). Or use `animate-rise` directly.
- **REC pulse:** `StatusLed` renders an expanding ring (`animate-led-ping`) for recording/connecting.
  `animate-led-breathe` is available for gentle opacity breathing.
- **Hover lift / transitions:** 150ms color transitions; amber focus rings (`:focus-visible` is
  amber globally; primary buttons add `ring-accent`).
- **Spinner:** `animate-spin-slow` (0.9s linear).
- `prefers-reduced-motion` collapses animations/transitions.

### Shadows

`shadow-panel` (hairline panel depth), `shadow-raised`, `shadow-glow` (amber focus glow),
`shadow-glow-rec` (emerald LED glow).

---

## Primitives — `src/components/ui.tsx`

Exact exports and signatures (stable contract):

```ts
cx(...classes: (string | false | null | undefined)[]): string

Panel(props: {
  title?: ReactNode; subtitle?: ReactNode; actions?: ReactNode;
  className?: string; bodyClassName?: string; padded?: boolean; children: ReactNode;
})

Button(props: ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "primary" | "default" | "ghost" | "danger";
  size?: "sm" | "md";
})   // defaults: variant="default", size="md", type="button"

Input(props: InputHTMLAttributes<HTMLInputElement>)            // mono styled text input
Textarea(props: TextareaHTMLAttributes<HTMLTextAreaElement>)
Select(props: SelectHTMLAttributes<HTMLSelectElement> & { children: ReactNode })  // custom chevron
Field(props: { label: ReactNode; hint?: ReactNode; htmlFor?: string; children: ReactNode })

type CameraState = "recording" | "connecting" | "offline" | "error" | "disabled" | "unknown"
StatusLed(props: { state: string; pulse?: boolean })   // glowing dot; auto-pulses rec/connecting
StatusPill(props: { state: string; label?: string })   // LED + UPPERCASE mono label
Stat(props: { label: ReactNode; value: ReactNode; unit?: ReactNode;
              tone?: "default" | "good" | "warn" | "bad" })
Spinner(props: { size?: number })                       // default 16
EmptyState(props: { title: ReactNode; hint?: ReactNode; action?: ReactNode })
SectionLabel(props: { children: ReactNode })            // small UPPERCASE mono label
```

Unknown `state` strings normalize to `"unknown"` in `StatusLed` / `StatusPill`.

### Shell — `src/components/AppShell.tsx`

```ts
AppShell(props: { children: ReactNode })   // also default-exported
```

- **Left nav rail** (sticky, `w-[232px]`, hidden below `sm`): wordmark `HELDAR` (Archivo, wide
  tracking) + `CORE` sublabel; NavLinks `Wall → "/"`, `Discover → "/discover"`,
  `Add Camera → "/cameras/new"` with amber active bar/icon. Footer shows build version.
- **Top telemetry bar** (sticky, polls `GET /api/v1/system` every 5s): link-status LED, live
  REC count (green) over total cameras, camera count, active recorders, segment count, a storage
  gauge (`recordings_gb / max_recordings_gb`), uptime, and a running local clock. All mono.
- Renders the `.app-atmosphere` layer behind the page content.

### Compatibility wrappers

- `StatusBadge` (`components/StatusBadge.tsx`) — named + default export; `{ state, className? }`;
  thin wrapper over `StatusPill`.
- `SystemBar` (`components/SystemBar.tsx`) — named + default export; `{ info, error }`; on-theme
  shim (live telemetry now lives in the AppShell bar).

### Back-compat CSS classes (`index.css @layer components`)

`.panel`, `.panel-head`, `.panel-title`, `.btn`, `.btn-sm`, `.btn-primary`, `.btn-danger`,
`.input`, `.label`, `.stat-k`, `.stat-v`, `.micro-label`, `.stagger` — re-skinned onto the SOC
tokens so legacy markup stays cohesive. Phase 2 should prefer the `ui.tsx` primitives.
