import { describe, expect, it } from "vitest";
import {
  cameraSiteZone,
  formatClockIn,
  formatTimeShortIn,
  friendlyError,
  isRenderableZone,
  resolveCameraZone,
  scheduleClockLabel,
  zoneLabel,
} from "./format";
import { ApiError } from "./api";

describe("friendlyError", () => {
  it("explains 503 instead of leaking the server string", () => {
    // The kernel returns 503 deliberately (semantic search with no embedding worker), and without
    // this branch the raw message surfaced.
    expect(friendlyError(new ApiError(503, "no embedding worker"))).toMatch(
      /temporarily unavailable/i,
    );
  });

  it("still falls through to the message for unmapped statuses", () => {
    expect(friendlyError(new ApiError(418, "teapot"))).toBe("teapot");
  });
});

describe("ApiError", () => {
  it("carries the server's code and retryability when present", () => {
    const e = new ApiError(503, "no worker", "unavailable", true);
    expect(e.code).toBe("unavailable");
    expect(e.retryable).toBe(true);
  });

  it("tolerates an older box that sends neither", () => {
    // The fields are additive: a kernel predating them still produces a usable ApiError.
    const e = new ApiError(404, "gone");
    expect(e.code).toBeUndefined();
    expect(e.retryable).toBeUndefined();
    expect(friendlyError(e)).toBe("gone");
  });
});

describe("scheduleClockLabel", () => {
  it("names the configured zone", () => {
    expect(
      scheduleClockLabel({ configured: "Asia/Kuala_Lumpur", server_local_offset: "+00:00" }),
    ).toBe("Asia/Kuala_Lumpur");
  });

  it("says whose clock it is when nothing is configured, rather than implying UTC", () => {
    // The failure this guards: labelling an unconfigured box "UTC" reads as a deliberate choice,
    // when the schedule actually follows whatever TZ the container happens to have.
    const label = scheduleClockLabel({ configured: null, server_local_offset: "+08:00" });
    expect(label).toContain("server clock");
    expect(label).toContain("+08:00");
    expect(label).not.toBe("UTC");
  });

  it("renders nothing while the setting is still loading", () => {
    // Rather than flashing a label that might be wrong.
    expect(scheduleClockLabel(null)).toBeNull();
    expect(scheduleClockLabel(undefined)).toBeNull();
  });
});

describe("cameraSiteZone + scheduleClockLabel together", () => {
  const box = { configured: "UTC", server_local_offset: "+00:00" };
  const sites = [
    { id: "kl", timezone: "Asia/Kuala_Lumpur" },
    { id: "nozone", timezone: null },
  ];

  it("a camera on a site follows ITS zone, not the box-wide one", () => {
    // The bug this guards: the schedule panel read the box-wide endpoint while the recorder reads
    // the camera's site. Labelling a KL camera's window "UTC" is an 8-hour lie stated with
    // authority — worse than the unlabelled window it replaced, because it invites an operator to
    // "fix" a schedule that was already correct.
    expect(scheduleClockLabel(box, cameraSiteZone("kl", sites))).toBe("Asia/Kuala_Lumpur");
  });

  it("a camera whose site names no zone falls back to the box-wide one", () => {
    expect(scheduleClockLabel(box, cameraSiteZone("nozone", sites))).toBe("UTC");
  });

  it("a camera with no site at all follows the box-wide one", () => {
    expect(scheduleClockLabel(box, cameraSiteZone(null, sites))).toBe("UTC");
    expect(scheduleClockLabel(box, cameraSiteZone(undefined, undefined))).toBe("UTC");
  });

  it("an unknown site id does not invent a zone", () => {
    // A scoped credential cannot list every site, and a site can be deleted. Neither may produce a
    // confident label for a clock nobody named.
    expect(scheduleClockLabel(box, cameraSiteZone("ghost", sites))).toBe("UTC");
  });

  it("still says whose clock it is when nothing is configured anywhere", () => {
    const unset = { configured: null, server_local_offset: "+08:00" };
    expect(scheduleClockLabel(unset, cameraSiteZone(null, sites))).toContain("server clock");
  });
});

describe("zone resolution (#148)", () => {
  const tz = { configured: "Asia/Singapore", server_local_offset: "+08:00" };

  it("prefers the camera's own site zone over the box-wide one", () => {
    // Same order as services/tz.rs. A camera on a site with its own zone rendered in the box's zone
    // is the 8-hour lie #147 fixed for schedule labels, reached through a different door.
    expect(resolveCameraZone(tz, "Asia/Kuala_Lumpur")).toEqual({
      kind: "site",
      zone: "Asia/Kuala_Lumpur",
    });
  });

  it("falls back to the box-wide zone when the site names none", () => {
    expect(resolveCameraZone(tz, null)).toEqual({
      kind: "box",
      zone: "Asia/Singapore",
    });
  });

  it("falls back to the VIEWER when neither names an IANA zone", () => {
    // The box reads schedules in the server's own clock, of which the API gives only an offset.
    // There is nothing to convert into, so the honest answer is the viewer's zone, labelled.
    const r = resolveCameraZone(
      { configured: null, server_local_offset: "+08:00" },
      null,
    );
    expect(r.kind).toBe("viewer");
    expect(zoneLabel(r)).toMatch(/your zone/);
  });

  it("labels a resolved site or box zone without the 'your zone' qualifier", () => {
    expect(zoneLabel({ kind: "site", zone: "Asia/Kuala_Lumpur" })).toBe(
      "Asia/Kuala_Lumpur",
    );
    expect(zoneLabel({ kind: "box", zone: "UTC" })).toBe("UTC");
  });
});

describe("rendering an instant in a zone (#148)", () => {
  // 2026-01-15T02:30:00Z is 10:30 in Kuala_Lumpur (+08) and 02:30 in London (winter, +00).
  const iso = "2026-01-15T02:30:00Z";

  it("actually converts, rather than returning the same string for every zone", () => {
    const kl = formatTimeShortIn(iso, "Asia/Kuala_Lumpur");
    const london = formatTimeShortIn(iso, "Europe/London");
    expect(kl).toContain("10:30");
    expect(london).toContain("02:30");
    expect(kl).not.toBe(london);
  });

  it("crosses midnight into the target zone's next day", () => {
    // 16:00Z on the 15th is 05:00 on the 16th in Auckland (UTC+13 in January).
    //
    // Asserted as an EQUIVALENCE rather than by matching digits: the rendered string is
    // locale-dependent — this runner produces "04:00:00 PM", so an obvious `toContain("16")` fails
    // for a correct conversion, and `toContain("16")` on the date would match "2026" for an
    // incorrect one. Comparing the Auckland view of one instant against the UTC view of the
    // equivalent instant is true in any locale, and pins the DATE roll as well as the clock, which
    // is the part a clock-only shift would miss.
    expect(formatClockIn("2026-01-15T16:00:00Z", "Pacific/Auckland")).toBe(
      formatClockIn("2026-01-16T05:00:00Z", "UTC"),
    );
    // ...and it is genuinely a different rendering from leaving the instant in UTC.
    expect(formatClockIn("2026-01-15T16:00:00Z", "Pacific/Auckland")).not.toBe(
      formatClockIn("2026-01-15T16:00:00Z", "UTC"),
    );
  });

  it("falls back instead of throwing on a zone Intl will not accept", () => {
    // A site's timezone is operator-typed text. One bad row must not blank every timestamp on the
    // page with a RangeError thrown inside a render.
    expect(() => formatClockIn(iso, "Not/AZone")).not.toThrow();
    expect(formatClockIn(iso, "Not/AZone")).not.toBe("—");
    expect(formatTimeShortIn(iso, "Not/AZone")).not.toBe("—");
  });

  it("still reports missing and malformed instants as em-dash", () => {
    expect(formatClockIn(null, "UTC")).toBe("—");
    expect(formatClockIn("not-a-date", "UTC")).toBe("—");
    expect(formatTimeShortIn(undefined, "UTC")).toBe("—");
  });
});

describe("isRenderableZone (#148)", () => {
  it("accepts IANA names", () => {
    expect(isRenderableZone("Asia/Kuala_Lumpur")).toBe(true);
    expect(isRenderableZone("UTC")).toBe(true);
  });

  it("REJECTS the label scheduleClockLabel produces for an unconfigured box", () => {
    // This is the whole reason resolveCameraZone exists beside scheduleClockLabel: that function
    // returns a true, useful SENTENCE which Intl will not take as a timeZone. Passing one through
    // would throw inside a render.
    const label = scheduleClockLabel(
      { configured: null, server_local_offset: "+08:00" },
      null,
    );
    expect(label).toMatch(/server clock/);
    expect(isRenderableZone(label as string)).toBe(false);
  });

  it("rejects a bare offset, which is not an IANA identifier", () => {
    expect(isRenderableZone("+08:00")).toBe(false);
  });
});
