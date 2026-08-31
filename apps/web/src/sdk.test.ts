import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/**
 * `src/sdk.ts` and `public/modules/shell-shim.js` must name the same runtime exports.
 *
 * `sdk.ts` is what a module's `tsc` typechecks against; the SHIM is what actually resolves at
 * runtime through the shell's import map. Add a name to one and forget the other and the module
 * builds cleanly, typechecks cleanly, and then throws in the browser when it imports something the
 * shim never published — a failure that reaches an operator rather than CI.
 *
 * `sdk.ts` says "keep it in sync with public/modules/shell-shim.js". Nothing checked that until this
 * test, and adding an export (#148) is exactly the edit that would have broken it.
 *
 * TYPE-only exports are excluded: they are erased at build time and have nothing to resolve at
 * runtime, which is why the shim does not carry them.
 */
function namesIn(block: string): string[] {
  return block
    .split("\n")
    .map((l) => l.replace(/\/\/.*$/, "")) // the lists are commented by section
    .join("\n")
    .split(",")
    .map((raw) => raw.trim().split(/\s+as\s+/).pop()?.trim() ?? "")
    .filter((n) => /^[A-Za-z_$][\w$]*$/.test(n));
}

/** `export { a, b } from "…"` — how sdk.ts declares the surface. */
function sdkNames(src: string): Set<string> {
  const names = new Set<string>();
  for (const m of src.matchAll(/export\s+(?!type\b)\{([^}]*)\}/g)) {
    for (const n of namesIn(m[1])) names.add(n);
  }
  return names;
}

/**
 * `export const { a, b } = S;` — how the shim republishes them.
 *
 * A different shape from sdk.ts, which is why this needs its own parser rather than one regex over
 * both: the first version of this test used the `export {…}` form for both, matched NOTHING in the
 * shim, and reported all 32 sdk exports as missing. The "parsed a plausible surface" case below is
 * what caught that, and is the reason it exists.
 */
function shimNames(src: string): Set<string> {
  const m = src.match(/export\s+const\s*\{([\s\S]*?)\}\s*=\s*S;/);
  return new Set(m ? namesIn(m[1]) : []);
}

describe("the module SDK surface and the runtime shim agree", () => {
  const sdk = sdkNames(readFileSync("src/sdk.ts", "utf8"));
  const shim = shimNames(readFileSync("public/modules/shell-shim.js", "utf8"));

  it("parsed a plausible surface from both files", () => {
    // Without this, a regex that matched nothing would make both comparisons below pass on empty
    // sets — the check would report success having checked nothing.
    expect(sdk.size).toBeGreaterThan(20);
    expect(shim.size).toBeGreaterThan(20);
    expect(sdk.has("SectionLabel")).toBe(true);
    expect(shim.has("SectionLabel")).toBe(true);
  });

  it("every runtime export in sdk.ts is published by the shim", () => {
    const missing = [...sdk].filter((n) => !shim.has(n)).sort();
    expect(
      missing,
      `these are importable from "@heldar/shell" per sdk.ts but are NOT in the runtime shim, so a ` +
        `module using one typechecks and then fails in the browser: ${missing.join(", ")}`,
    ).toEqual([]);
  });

  it("the shim publishes nothing sdk.ts does not declare", () => {
    const extra = [...shim].filter((n) => !sdk.has(n)).sort();
    expect(
      extra,
      `the shim publishes these but sdk.ts does not declare them, so a module cannot typecheck ` +
        `against something the shell actually provides: ${extra.join(", ")}`,
    ).toEqual([]);
  });
});
