import { defineConfig, devices } from "@playwright/test";

// E2E UI tests run against a RUNNING Heldar Core (the core serves the dashboard at one URL). By
// default this config boots an isolated synthetic stack (MediaMTX + 6 synthetic cameras + core) on a
// dedicated port via scripts/e2e_stack.sh — true e2e, no real cameras or credentials. Set
// HELDAR_E2E_BASE_URL to test an already-running deployment instead (no stack is booted; note the
// camera-specific specs assume the synthetic cam_e2e_* ids the stack seeds).
const externalBase = process.env.HELDAR_E2E_BASE_URL;
const baseURL = externalBase ?? "http://localhost:8011";

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? "github" : "list",
  // The dashboard polls the camera list on a 10s cadence, so a freshly-booted stack's cameras can
  // land just after first paint — give assertions enough time to absorb one poll.
  expect: { timeout: 15_000 },
  use: {
    baseURL,
    trace: "on-first-retry",
    screenshot: "only-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  // Boot the synthetic stack on the dedicated port unless an external deployment was given.
  webServer: externalBase
    ? undefined
    : {
        command: "bash ../../scripts/e2e_stack.sh",
        url: `${baseURL}/healthz`,
        // Always boot a fresh stack on the dedicated port (the script self-cleans any prior one), so a
        // stale stack from a killed run is never reused with out-of-window footage.
        reuseExistingServer: false,
        timeout: 120_000,
        stdout: "pipe",
        stderr: "pipe",
      },
});
