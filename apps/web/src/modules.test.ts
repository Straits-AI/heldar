import { describe, expect, it } from "vitest";

import { resolveBridgePath } from "./modules";

// The plugin host runs sidecars in an OPAQUE origin (no `allow-same-origin`), so a plugin cannot call
// anything itself — the host calls on its behalf, holding the operator's session. That makes this
// function the containment boundary: it decides what the host is willing to be asked for. If it lets a
// path escape the plugin's own `/m/{id}/` root, the bridge becomes a confused deputy handing a plugin
// exactly the console-wide authority the sandbox just removed.
describe("resolveBridgePath", () => {
  const MOD = "weather";

  it("allows paths inside the plugin's own proxy root", () => {
    expect(resolveBridgePath(MOD, "api/status")).toBe("/m/weather/api/status");
    expect(resolveBridgePath(MOD, "./api/status")).toBe("/m/weather/api/status");
    expect(resolveBridgePath(MOD, "api/query?since=1h")).toBe("/m/weather/api/query?since=1h");
    // The root itself is legitimate (the plugin's own index).
    expect(resolveBridgePath(MOD, "")).toBe("/m/weather/");
  });

  it("refuses traversal out of the root", () => {
    for (const escape of [
      "../other/api",
      "../../api/v1/cameras",
      "api/../../../api/v1/backup",
      "./../../api/v1/auth/me",
    ]) {
      expect(resolveBridgePath(MOD, escape), `must refuse ${escape}`).toBeNull();
    }
  });

  it("refuses absolute paths that would reach the kernel API", () => {
    // The whole point: a plugin must not be able to have the host call these with the session.
    expect(resolveBridgePath(MOD, "/api/v1/cameras")).toBeNull();
    expect(resolveBridgePath(MOD, "/api/v1/api-keys")).toBeNull();
    expect(resolveBridgePath(MOD, "/media/recordings/cam_a/x.mp4")).toBeNull();
  });

  it("refuses another plugin's root", () => {
    // Confinement is per-plugin, not merely "somewhere under /m/".
    expect(resolveBridgePath(MOD, "/m/other/api")).toBeNull();
    expect(resolveBridgePath(MOD, "../other/api")).toBeNull();
  });

  it("refuses absolute and protocol-relative URLs to other origins", () => {
    expect(resolveBridgePath(MOD, "https://evil.example/steal")).toBeNull();
    // Protocol-relative: resolves against the page's scheme, landing on another host.
    expect(resolveBridgePath(MOD, "//evil.example/steal")).toBeNull();
  });

  it("does not let a crafted module id widen the root", () => {
    // The id is encoded into the root, so a traversal-shaped id cannot escape by construction.
    expect(resolveBridgePath("../admin", "api")).toBe("/m/..%2Fadmin/api");
  });
});
