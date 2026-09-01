#!/usr/bin/env python3
"""Controls for scripts/openapi_diff.py. Run: python3 scripts/test_openapi_diff.py

The tool classifies contract changes as breaking or additive, and a classifier is only useful if it
is right in BOTH directions. A false "breaking" trains people to ignore the output; a false
"additive" is how a break ships. Each case below pins one rule, and the last group asserts the
long-standing rules still fire — the request/response split (#156) touched shared code.
"""

import json
import os
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TOOL = os.path.join(ROOT, "scripts", "openapi_diff.py")


def doc(schemas, *, request_schema=None, response_schema=None, paths=None):
    """A minimal document. `request_schema`/`response_schema` place a schema on one side."""
    op = {"responses": {"200": {"description": "ok"}}}
    if request_schema:
        op["requestBody"] = {
            "content": {"application/json": {"schema": {"$ref": f"#/components/schemas/{request_schema}"}}}
        }
    if response_schema:
        op["responses"]["200"] = {
            "description": "ok",
            "content": {"application/json": {"schema": {"$ref": f"#/components/schemas/{response_schema}"}}},
        }
    return {
        "openapi": "3.1.0",
        "info": {"title": "t", "version": "1.0.0"},
        "paths": paths if paths is not None else {"/x": {"post": op}},
        "components": {"schemas": schemas},
    }


def run(old, new):
    with tempfile.TemporaryDirectory() as d:
        a, b = os.path.join(d, "a.json"), os.path.join(d, "b.json")
        json.dump(old, open(a, "w"))
        json.dump(new, open(b, "w"))
        r = subprocess.run([sys.executable, TOOL, a, b], capture_output=True, text=True)
        return r.returncode, r.stdout + r.stderr


def obj(required):
    return {"type": "object", "properties": {"f": {"type": "string"}}, "required": required}


CASES = [
    (
        "a RESPONSE schema gaining a required field is additive, not breaking",
        lambda: run(doc({"R": obj([])}, response_schema="R"),
                    doc({"R": obj(["f"])}, response_schema="R")),
        0, "always sends",
    ),
    (
        "a REQUEST schema gaining a required field is still breaking",
        lambda: run(doc({"Q": obj([])}, request_schema="Q"),
                    doc({"Q": obj(["f"])}, request_schema="Q")),
        1, "now requires",
    ),
    (
        "a schema used on BOTH sides is judged by the request rule",
        lambda: run(doc({"B": obj([])}, request_schema="B", response_schema="B"),
                    doc({"B": obj(["f"])}, request_schema="B", response_schema="B")),
        1, "now requires",
    ),
    (
        "a schema referenced only THROUGH a response schema counts as a response",
        lambda: run(
            doc({"Outer": {"type": "object",
                           "properties": {"inner": {"$ref": "#/components/schemas/Inner"}}},
                 "Inner": obj([])}, response_schema="Outer"),
            doc({"Outer": {"type": "object",
                           "properties": {"inner": {"$ref": "#/components/schemas/Inner"}}},
                 "Inner": obj(["f"])}, response_schema="Outer"),
        ),
        0, "always sends",
    ),
    (
        "an UNREFERENCED schema is judged by the stricter request rule",
        lambda: run(doc({"Orphan": obj([])}), doc({"Orphan": obj(["f"])})),
        1, "now requires",
    ),
    # --- rules that already existed must still fire; the split touched shared code ---
    (
        "a removed route is still breaking",
        lambda: run(doc({}, paths={"/gone": {"get": {"responses": {"200": {"description": "ok"}}}}}),
                    doc({}, paths={})),
        1, "",
    ),
    (
        "a removed response field is still breaking",
        lambda: run(
            doc({"R": {"type": "object",
                       "properties": {"f": {"type": "string"}, "g": {"type": "string"}}}},
                response_schema="R"),
            doc({"R": {"type": "object", "properties": {"f": {"type": "string"}}}},
                response_schema="R"),
        ),
        1, "no longer has",
    ),
    (
        "an identical document is not a change",
        lambda: run(doc({"R": obj(["f"])}, response_schema="R"),
                    doc({"R": obj(["f"])}, response_schema="R")),
        0, "",
    ),
]


def main():
    bad = 0
    for name, fn, want_rc, want_text in CASES:
        rc, out = fn()
        ok = rc == want_rc and (not want_text or want_text in out)
        print(("  ok    " if ok else "  FAIL  ") + name)
        if not ok:
            bad += 1
            print(f"        rc={rc} (want {want_rc}); wanted {want_text!r} in:\n"
                  + "\n".join("        " + l for l in out.strip().splitlines()[:12]))
    print(f"\n{len(CASES) - bad}/{len(CASES)} controls behaved as specified")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
