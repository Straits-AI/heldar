import { describe, expect, it } from "vitest";
import { friendlyError, scheduleClockLabel } from "./format";
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
