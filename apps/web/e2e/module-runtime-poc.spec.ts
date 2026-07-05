/**
 * Task 1 spike: runtime-load one module bundle with a shared-React import map.
 *
 * Acceptance criteria (both must pass):
 *   1. The search UI renders at /poc-module (proves the module loaded + React ran).
 *   2. No console error contains "Invalid hook call" or "two copies of React"
 *      (proves the import map wired up the shell's single React instance — no double-React).
 *
 * This test is temporary; it is removed in Task 2 once the spike is confirmed.
 */
import { test, expect } from "@playwright/test";

test.describe("module-runtime POC — shared-React import map (Task 1 spike)", () => {
  test("search module renders at /poc-module with no duplicate-React errors", async ({ page }) => {
    const consoleErrors: string[] = [];

    page.on("console", (msg) => {
      if (msg.type() === "error") {
        consoleErrors.push(msg.text());
      }
    });

    await page.goto("/poc-module");

    // The Search component goes through several states depending on whether a backend is available:
    //   - Loading spinner (Authenticating…)
    //   - Login form (if /auth/me 401s)
    //   - "Console unavailable" error panel (if /auth/me returns non-JSON — e.g. the SPA HTML)
    //   - Full search console (if authenticated)
    // Any of these prove the module bundle mounted and React ran correctly.
    // We wait for any visible text or interactive element from the Search component.
    const rendered =
      (await page
        .locator("button, input, h1, [role='alert']")
        .first()
        .waitFor({ state: "visible", timeout: 15_000 })
        .then(() => true)
        .catch(() => false)) as boolean;

    expect(rendered, "Search module should render visible UI").toBe(true);

    // Check no duplicate-React / invalid-hook-call errors fired.
    const hookErrors = consoleErrors.filter(
      (e) =>
        e.toLowerCase().includes("invalid hook call") ||
        e.toLowerCase().includes("two copies of react") ||
        e.toLowerCase().includes("duplicate react"),
    );

    expect(
      hookErrors,
      `No duplicate-React errors expected, got: ${hookErrors.join("; ")}`,
    ).toHaveLength(0);
  });
});
