import { defineConfig } from "vitest/config";

// Unit lane for pure logic that must not regress silently — chiefly the sidecar bridge's containment
// check, which decides what a sandboxed plugin may ask the host to fetch with the operator's session.
// The browser-level suites (playwright.config.ts, playwright.tls.config.ts) stay separate: they boot a
// real stack and are far too slow to run on every edit.
export default defineConfig({
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
  },
});
