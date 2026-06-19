import { defineConfig, devices } from "@playwright/test";

// E2E UI tests run against a RUNNING Heldar Core (the dashboard is served by the
// core at one URL). Point at a deployment with HELDAR_E2E_BASE_URL; defaults to a
// local core on :8000. The specs assert the operator dashboard shell loads and
// talks to the core — they need no cameras, so they pass against the synthetic
// validate.sh stack or a real deployment alike.
const baseURL = process.env.HELDAR_E2E_BASE_URL ?? "http://localhost:8000";

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? "github" : "list",
  use: {
    baseURL,
    trace: "on-first-retry",
    screenshot: "only-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
});
