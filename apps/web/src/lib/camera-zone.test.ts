import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/**
 * Camera-scoped surfaces must not render an absolute clock in the browser's zone (#148).
 *
 * The page-level fix is only as good as the next person's edit. `formatClock` and `formatTimeShort`
 * render in whatever zone the viewer's browser is in; on a camera page that produces exactly the
 * inconsistency this issue was filed for — a site-zone timeline sitting above a browser-zone list.
 *
 * A source-level check rather than a rendering one, because the failure is a WRONG CLOCK, not a
 * crash or a missing element: it renders perfectly and says the wrong time, so no smoke test would
 * catch it. This one fails the moment the import reappears.
 *
 * `timeAgo` is deliberately not on the list — "2m ago" reads the same in every zone.
 */
const CAMERA_SCOPED = [
  "src/pages/CameraDetail.tsx",
  "src/components/Timeline.tsx",
  "src/components/AiPanel.tsx",
  "src/components/ZonePanel.tsx",
  "src/components/RecordingPanels.tsx",
  "src/components/CameraConfigPanel.tsx",
];

const BROWSER_ZONE = ["formatClock", "formatTimeShort"];

describe("camera-scoped surfaces render in the camera's zone (#148)", () => {
  for (const file of CAMERA_SCOPED) {
    it(`${file} uses no browser-zone clock helper`, () => {
      const src = readFileSync(file, "utf8");
      for (const helper of BROWSER_ZONE) {
        // `formatClockIn(` must not match `formatClock(` — the "In" suffix is the whole point.
        const bare = new RegExp(`\\b${helper}\\s*\\(`, "g");
        const found = [...src.matchAll(bare)].filter(
          (m) => !src.slice(m.index).startsWith(`${helper}In`),
        );
        expect(
          found.length,
          `${file} calls ${helper}(), which renders in the VIEWER's zone. On a camera-scoped ` +
            `surface use ${helper}In(iso, zone) with the zone the page resolved, or the page ends ` +
            `up showing two clocks and labelling one.`,
        ).toBe(0);
      }
    });
  }

  it("the guard would actually fire", () => {
    // Without this, a typo in the regex above would make every case above pass vacuously.
    const sample = `import { formatClock } from "./format";\nconst x = formatClock(iso);`;
    const bare = /\bformatClock\s*\(/g;
    const found = [...sample.matchAll(bare)].filter(
      (m) => !sample.slice(m.index).startsWith("formatClockIn"),
    );
    expect(found.length).toBe(1);
    // ...and does not fire on the zone-aware form.
    const ok = `const x = formatClockIn(iso, zone);`;
    const okFound = [...ok.matchAll(/\bformatClock\s*\(/g)].filter(
      (m) => !ok.slice(m.index).startsWith("formatClockIn"),
    );
    expect(okFound.length).toBe(0);
  });
});
