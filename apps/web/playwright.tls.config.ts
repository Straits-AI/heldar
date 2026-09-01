import { defineConfig, devices } from "@playwright/test";

// The SECURE end-to-end suite. Separate from playwright.config.ts on purpose: that one runs HTTP with
// auth disabled, which structurally cannot observe a mixed-content failure. This one boots a real
// Caddy terminating HTTPS in front of an auth-enabled core (scripts/e2e_tls_stack.sh), so the live
// media path is exercised the way an operator actually deploys it.
//
// The certificate is Caddy's local CA (self-signed), hence ignoreHTTPSErrors. That does NOT weaken
// what is under test: the browser still enforces the mixed-content rule on an https:// page, which is
// the rule this suite exists to check.
const tlsPort = process.env.E2E_TLS_PORT ?? "8443";
const baseURL = process.env.HELDAR_E2E_TLS_BASE_URL ?? `https://localhost:${tlsPort}`;
// Must match scripts/e2e_tls_stack.sh.
const cams = Number(process.env.E2E_TLS_CAMS ?? 2);

export default defineConfig({
  testDir: "./e2e-tls",
  fullyParallel: false, // one shared stack, and the specs assert on whole-page request sets
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? "github" : "list",
  expect: { timeout: 15_000 },
  use: {
    baseURL,
    ignoreHTTPSErrors: true,
    trace: "on-first-retry",
    screenshot: "only-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: process.env.HELDAR_E2E_TLS_BASE_URL
    ? undefined
    : {
        command: "bash ../../scripts/e2e_tls_stack.sh",
        // The stack starts Caddy LAST, after every camera is seeded, precisely so this probe is a
        // valid readiness gate: if HTTPS answers at all, seeding already finished. Gating on the
        // core's own /healthz would go green far too early (that raced the seed in the plain suite),
        // and the camera API needs a session, so an unauthenticated probe there is a 401 that
        // Playwright would never accept as ready.
        url: `${baseURL}/healthz`,
        ignoreHTTPSErrors: true,
        reuseExistingServer: false,
        timeout: 180_000,
        stdout: "pipe",
        stderr: "pipe",
      },
});
