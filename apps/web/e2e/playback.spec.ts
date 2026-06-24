import { test, expect } from "@playwright/test";

// Multi-camera synchronized playback against the e2e stack (6 synthetic cameras recording 5s segments).
// Waits until footage is indexed, then opens two cameras over the default window and asserts the synced
// transport + a player per camera.
test("synchronized playback opens a session per selected camera", async ({ page, request }) => {
  // The cameras need at least one completed, indexed segment before a playback session has anything to
  // build — poll the segments API rather than guessing a sleep.
  await expect
    .poll(
      async () => {
        const r = await request.get("/api/v1/cameras/cam_e2e_1/segments?limit=1");
        if (!r.ok()) return 0;
        const segs = await r.json();
        return Array.isArray(segs) ? segs.length : 0;
      },
      { timeout: 40_000, intervals: [1500] },
    )
    .toBeGreaterThan(0);

  await page.goto("/playback");
  await expect(page.getByRole("heading", { name: "Synchronized Playback" })).toBeVisible();

  // Use the page's OWN default window ([now − 30 min, now], computed in the browser's timezone so it's
  // self-consistent and within the 2h playback cap). It covers the footage the poll above confirmed.
  await page.getByTestId("pb-cam-cam_e2e_1").click();
  await page.getByTestId("pb-cam-cam_e2e_2").click();
  await page.getByTestId("pb-open").click();

  // sessions built → the shared transport + one <video> per camera appear
  await expect(page.getByTestId("pb-transport")).toBeVisible();
  await expect(page.locator('[data-testid="pb-grid"] video')).toHaveCount(2);

  // the transport drives all players: a speed button is reflected as pressed
  await page.getByRole("button", { name: "2×" }).click();
  await expect(page.getByRole("button", { name: "2×" })).toHaveAttribute("aria-pressed", "true");
});
