import type { ComponentType } from "react";

// The contract a runtime-loaded module UI bundle fulfils: it default-exports its page component.
//
// A module bundle shares the shell's single React instance via the `window.__HELDAR_*__` bridge that
// `main.tsx` installs (its bare `import "react"` etc. resolve, through the shell's import map, to the
// `public/modules/*-shim.js` re-exports of those globals). It runs same-origin with the console, so it
// uses the shell's `api` client + cookies directly — no props or context are threaded in.
export type ModuleBundle = { default: ComponentType };
