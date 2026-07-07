import { test, expect } from "@playwright/test";

// The e2e stack registers 6 synthetic cameras, so fixed layouts paginate predictably:
//   2x2 (4/page) -> 2 pages (4 + 2);  3x3 (9/page) -> 1 page.
test.describe("camera wall — DVR multi-view", () => {
  test("default (auto) layout shows every camera", async ({ page }) => {
    await page.goto("/");
    const grid = page.getByTestId("camera-grid");
    await expect(grid).toBeVisible();
    await expect(grid).toHaveAttribute("data-layout", "auto");
    await expect(grid.locator("> *")).toHaveCount(6);
    await expect(page.getByTestId("wall-pager")).toHaveCount(0); // auto = one page
  });

  test("2x2 layout paginates 6 cameras across 2 pages", async ({ page }) => {
    await page.goto("/");
    await page.getByTestId("wall-layout-4").click();

    const grid = page.getByTestId("camera-grid");
    await expect(grid).toHaveAttribute("data-layout", "4");
    await expect(page.getByTestId("wall-layout-4")).toHaveAttribute("aria-pressed", "true");
    await expect(grid.locator("> *")).toHaveCount(4);
    await expect(page.getByTestId("wall-page-indicator")).toHaveText("Page 1 / 2");
    await expect(page.getByTestId("wall-prev")).toBeDisabled();

    await page.getByTestId("wall-next").click();
    await expect(page).toHaveURL(/layout=4/);
    await expect(page).toHaveURL(/page=2/);
    await expect(page.getByTestId("wall-page-indicator")).toHaveText("Page 2 / 2");
    await expect(grid.locator("> *")).toHaveCount(2); // remaining cameras
    await expect(page.getByTestId("wall-next")).toBeDisabled();
  });

  test("3x3 layout fits all 6 on a single page (no pager)", async ({ page }) => {
    await page.goto("/?layout=9");
    const grid = page.getByTestId("camera-grid");
    await expect(grid).toHaveAttribute("data-layout", "9");
    await expect(grid.locator("> *")).toHaveCount(6);
    await expect(page.getByTestId("wall-pager")).toHaveCount(0);
  });

  test("chosen layout persists across a fresh visit (localStorage)", async ({ page }) => {
    await page.goto("/");
    await page.getByTestId("wall-layout-16").click(); // 4x4
    await expect(page.getByTestId("camera-grid")).toHaveAttribute("data-layout", "16");

    // fresh visit with NO url param → restored from localStorage, not back to auto
    await page.goto("/");
    await expect(page.getByTestId("camera-grid")).toHaveAttribute("data-layout", "16");
  });
});
