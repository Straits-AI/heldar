import { test, expect } from "@playwright/test";

// Uses the synthetic camera `cam_e2e_1` seeded by the e2e stack (it also has a motion AI task).
test("camera detail page renders the camera identity", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(`pageerror: ${e}`));
  page.on("console", (m) => {
    const t = m.text();
    // Real app/JS errors only — not network-resource noise (a freshly-recording camera may not have
    // a snapshot/live segment ready yet, which surfaces as a transient resource 404/500).
    if (m.type() === "error" && !t.includes("favicon") && !t.includes("Failed to load resource"))
      errors.push(t);
  });

  await page.goto("/cameras/cam_e2e_1");
  await expect(page).toHaveTitle(/Heldar Core/);
  await expect(page.getByRole("heading", { name: "E2E Camera 1" })).toBeVisible();
  await expect(page.getByText("cam_e2e_1").first()).toBeVisible();

  expect(errors, `unexpected console/page errors: ${errors.join(" | ")}`).toHaveLength(0);
});

test("a wall tile links through to its camera detail", async ({ page }) => {
  await page.goto("/");
  const tileLink = page.locator('a[href="/cameras/cam_e2e_1"]').first();
  await expect(tileLink).toBeVisible();
  await tileLink.click();
  await expect(page).toHaveURL(/\/cameras\/cam_e2e_1$/);
  await expect(page.getByRole("heading", { name: "E2E Camera 1" })).toBeVisible();
});

test("AI perception page reports the active sampler", async ({ page }) => {
  await page.goto("/ai");
  await expect(page.getByText("AI Perception")).toBeVisible();
  await expect(page.getByText(/ACTIVE SAMPLERS/i)).toBeVisible();
});
