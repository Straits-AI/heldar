import { test, expect, type ConsoleMessage } from "@playwright/test";

// Smoke + connectivity e2e against a running Heldar Core. These assert the dashboard
// shell loads, renders without console/page errors, and reaches the core API. They do
// NOT require any cameras to be configured, so they pass against the synthetic stack or
// a live deployment. Richer, camera-dependent assertions belong in a separate spec that
// seeds a (synthetic) camera first.

test("core API is reachable", async ({ request }) => {
  const res = await request.get("/api/v1/cameras");
  expect(res.ok()).toBeTruthy();
  expect(Array.isArray(await res.json())).toBeTruthy();
});

test("camera wall loads with no console or page errors", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(`pageerror: ${e}`));
  page.on("console", (m: ConsoleMessage) => {
    const t = m.text();
    // Track real app/JS errors, not browser network-resource noise (favicon, a not-yet-available
    // snapshot/frame on a freshly-booted camera) — those are backend state, not UI defects.
    if (m.type() === "error" && !t.includes("favicon") && !t.includes("Failed to load resource"))
      errors.push(t);
  });

  await page.goto("/");
  await expect(page).toHaveTitle(/Heldar Core/);
  await expect(page.getByRole("heading", { name: "Camera Wall" })).toBeVisible();

  expect(errors, `unexpected console/page errors: ${errors.join(" | ")}`).toHaveLength(0);
});

test("primary nav routes render", async ({ page }) => {
  await page.goto("/ai");
  await expect(page.getByText("AI Perception")).toBeVisible();
  await expect(page.getByText(/ACTIVE SAMPLERS/i)).toBeVisible();

  await page.goto("/incidents");
  await expect(page).toHaveTitle(/Heldar Core/);

  await page.goto("/system");
  await expect(page).toHaveTitle(/Heldar Core/);
});
