import { describe, expect, it } from "vitest";
import { friendlyError } from "./format";
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
