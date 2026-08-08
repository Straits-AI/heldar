import { test, expect } from "@playwright/test";

// Multi-camera synchronized playback against the e2e stack (6 synthetic cameras recording 5s segments).
// Waits until footage is indexed, then opens two cameras over the default window and asserts the synced
// transport + a player per camera.
test("synchronized playback opens a session per selected camera", async ({ page, request }) => {
  // The cameras need at least one completed, indexed segment before a playback session has anything to
  // build — poll the segments API rather than guessing a sleep. Poll EVERY camera this test opens, not
  // just the first: they are registered sequentially and start recording at slightly different times,
  // so cam_e2e_2 can still have nothing indexed when cam_e2e_1 does, and its session then fails to
  // build (the transport never appears, or only one <video> mounts).
  const cameras = ["cam_e2e_1", "cam_e2e_2"];
  await expect
    .poll(
      async () => {
        const counts = await Promise.all(
          cameras.map(async (id) => {
            const r = await request.get(`/api/v1/cameras/${id}/segments?limit=1`);
            if (!r.ok()) return 0;
            const segs = await r.json();
            return Array.isArray(segs) ? segs.length : 0;
          }),
        );
        // The weakest camera gates the test.
        return Math.min(...counts);
      },
      { timeout: 60_000, intervals: [1500] },
    )
    .toBeGreaterThan(0);

  await page.goto("/playback");
  await expect(page.getByRole("heading", { name: "Synchronized Playback" })).toBeVisible();

  // Extend the page's default window ([now − 30 min, now]) a few minutes into the future. The `To`
  // control is a datetime-local input, so it has MINUTE precision: a `To` of "now" floors to :00 and
  // excludes anything recorded during the current partial minute. On a freshly-booted stack that is
  // all of the footage, and the open fails with "no recorded footage in the requested range". Computed
  // in-page so it uses the browser's own clock and timezone, and stays inside the 2h playback cap.
  const windowEnd = await page.evaluate(() => {
    const d = new Date(Date.now() + 5 * 60 * 1000);
    const pad = (n: number) => String(n).padStart(2, "0");
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
  });
  await page.getByRole("textbox", { name: "To" }).fill(windowEnd);

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
