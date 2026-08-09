import { test, expect, type Page } from "@playwright/test";

// Live view over real HTTPS, with kernel auth enabled — the configuration a customer deploys.
//
// This suite exists because of a shipped regression: MediaMTX serves HLS/WebRTC on plaintext
// 8888/8889, and the kernel used to hand the browser absolute `http://host:8888/...` URLs (rewriting
// only the host, preserving scheme and port). On an HTTPS dashboard the browser blocks those as mixed
// content, so live view — the appliance's primary function — was dead behind the TLS overlay, and the
// HTTP-only e2e suite could not observe it. Everything below is about that boundary.

const ADMIN_USER = process.env.E2E_TLS_ADMIN_USER ?? "e2eadmin";
const ADMIN_PASS = process.env.E2E_TLS_ADMIN_PASSWORD ?? "e2e-tls-suite-pw";
const CAMERA = "cam_tls_1";

/** Log in through the API on the page's own context, so the HttpOnly session cookie lands in the
 *  browser jar without coupling this suite to the login form's markup. */
async function login(page: Page) {
  const res = await page.request.post("/api/v1/auth/login", {
    data: { username: ADMIN_USER, password: ADMIN_PASS },
  });
  expect(res.ok(), `login failed: ${res.status()} ${await res.text()}`).toBeTruthy();
}

test("auth is actually enforced on this stack", async ({ page }) => {
  // Guards the suite itself: if auth silently defaulted off, every assertion below would still pass
  // while testing the wrong posture entirely.
  const res = await page.request.get(`/api/v1/cameras/${CAMERA}`);
  expect(res.status(), "the API must reject an unauthenticated read").toBe(401);
});

test("live URLs are origin-relative so an HTTPS page can load them", async ({ page }) => {
  await login(page);
  const res = await page.request.get(`/api/v1/cameras/${CAMERA}/liveview`);
  expect(res.ok(), `liveview failed: ${res.status()}`).toBeTruthy();
  const urls = await res.json();

  // The regression, asserted directly: an absolute http:// URL here is what the browser blocks.
  for (const key of ["hls_url", "webrtc_url"] as const) {
    const value: string = urls[key];
    expect(value, `${key} must be origin-relative`).toMatch(/^\//);
    expect(value, `${key} must not carry its own scheme`).not.toContain("://");
  }
  expect(urls.hls_url).toContain("/live/hls/");
  expect(urls.webrtc_url).toContain("/live/whep/");
});

test("the HLS playlist is served through HTTPS on the dashboard origin", async ({ page }) => {
  await login(page);
  const live = await (await page.request.get(`/api/v1/cameras/${CAMERA}/liveview`)).json();

  // Fetching the relative URL resolves it against the HTTPS baseURL, so a 200 here proves the whole
  // chain: same-origin URL -> Caddy /live/hls handler -> MediaMTX. Poll because the live publisher
  // may still be starting the stream when the suite opens.
  await expect
    .poll(
      async () => (await page.request.get(live.hls_url)).status(),
      { timeout: 45_000, intervals: [2000] },
    )
    .toBe(200);

  const playlist = await (await page.request.get(live.hls_url)).text();
  expect(playlist, "expected an HLS playlist from MediaMTX").toContain("#EXTM3U");

  // Proxying the media plane must not become a way around kernel auth. MediaMTX still calls the
  // kernel back per read, so the same URL without the minted token is refused — through Caddy, and
  // through MediaMTX's cookieCheck redirect hop.
  const noToken = live.hls_url.split("?")[0];
  const refused = await page.request.get(noToken);
  expect(refused.status(), "an untokened live read must not be served").toBe(401);
});

test("opening live view issues no insecure requests", async ({ page }) => {
  await login(page);

  // The browser-level check. Mixed content is blocked rather than downgraded, so a regression shows
  // up here as an http:// request attempt (and a dead player), not as a silent fallback.
  const insecure: string[] = [];
  page.on("request", (req) => {
    const url = req.url();
    if (url.startsWith("http://")) insecure.push(url);
  });
  const consoleErrors: string[] = [];
  page.on("console", (m) => {
    const t = m.text();
    // "Mixed Content: ... was loaded over HTTPS, but requested an insecure ..." is the exact symptom.
    if (m.type() === "error" && /mixed content/i.test(t)) consoleErrors.push(t);
  });

  await page.goto(`/cameras/${CAMERA}`);
  await expect(page.getByRole("heading", { name: "TLS Camera 1" })).toBeVisible();
  // Give the player time to request media (WHEP first, HLS fallback) rather than asserting on an
  // empty request set.
  await page.waitForTimeout(8000);

  expect(insecure, `page requested plaintext URLs: ${insecure.join(", ")}`).toHaveLength(0);
  expect(consoleErrors, `mixed-content errors: ${consoleErrors.join(" | ")}`).toHaveLength(0);
});
