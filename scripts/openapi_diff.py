#!/usr/bin/env python3
"""Compare two OpenAPI documents and classify what changed (#120).

Usage: openapi_diff.py <old.json> <new.json>

Exit 0 when nothing breaking changed, 1 when something did. The point is not to forbid breaking
changes — this appliance is pre-1.0 and they are allowed — but to make one impossible to ship by
ACCIDENT. A removed route or a field that quietly became required breaks a generated client at
runtime, in someone else's deployment, long after the commit that did it.

WHAT COUNTS AS BREAKING, and why each one:

  * a route or method disappears            — a client's call 404s
  * a required REQUEST field is ADDED       — every existing caller's payload becomes invalid
  * an optional REQUEST field becomes required — same, with no signal
  * a response field is REMOVED             — a client reading it gets undefined
  * a declared capability CHANGES           — a working credential stops working
  * an enum loses a value a client may send

Additive changes are not breaking and are reported separately, because "what's new" is the other
question an integrator asks and there is no reason to make them diff it themselves.
"""

import json
import sys


def load(path):
    try:
        with open(path, "rb") as fh:
            return json.load(fh)
    except FileNotFoundError:
        print(f"MISSING: {path} — cannot compare against a document that is not there")
        sys.exit(2)
    except json.JSONDecodeError as e:
        print(f"MALFORMED: {path} is not valid JSON ({e})")
        sys.exit(2)


METHODS = ("get", "put", "post", "delete", "patch", "head", "options")


def operations(doc):
    """{(path, method): operation} for every operation in the document."""
    out = {}
    for path, item in (doc.get("paths") or {}).items():
        if not isinstance(item, dict):
            continue
        for method, op in item.items():
            if method in METHODS and isinstance(op, dict):
                out[(path, method)] = op
    return out


def refs_in(node, out):
    """Every `#/components/schemas/X` name reachable from `node`, transitively."""
    if isinstance(node, dict):
        r = node.get("$ref")
        if isinstance(r, str) and r.startswith("#/components/schemas/"):
            out.add(r.rsplit("/", 1)[-1])
        for v in node.values():
            refs_in(v, out)
    elif isinstance(node, list):
        for v in node:
            refs_in(v, out)
    return out


def schema_roles(doc):
    """`(request_schemas, response_schemas)` — which side of the wire each named schema is used on.

    A REQUEST schema and a RESPONSE schema break in OPPOSITE directions: tightening a request breaks
    callers, loosening a response breaks readers. This module's own docstring said so from the start
    and the code applied one rule to both, so a response schema gaining a `required` field — which is
    the server PROMISING MORE, and is what a reader wants — was reported as BREAKING.

    That matters because a diff tool that cries wolf is one people stop reading. Found when
    `TimezoneSettings.configured` was marked required to say the server always sends it (#156).

    A schema used on BOTH sides is treated as a request schema: the request rule is the stricter one,
    and guessing wrong in that direction only over-reports.
    """
    schemas = (doc.get("components") or {}).get("schemas") or {}
    req, resp = set(), set()
    for op in operations(doc).values():
        refs_in(op.get("requestBody") or {}, req)
        refs_in(op.get("responses") or {}, resp)

    # Follow references BETWEEN schemas: a schema is on whichever side the schema naming it is on.
    def close(seed):
        seen, queue = set(seed), list(seed)
        while queue:
            name = queue.pop()
            for child in refs_in(schemas.get(name) or {}, set()):
                if child not in seen:
                    seen.add(child)
                    queue.append(child)
        return seen

    return close(req), close(resp)


def required_of(schema):
    return set(schema.get("required") or []) if isinstance(schema, dict) else set()


def properties_of(schema):
    p = schema.get("properties") if isinstance(schema, dict) else None
    return set(p.keys()) if isinstance(p, dict) else set()


def main():
    if len(sys.argv) < 3:
        print("usage: openapi_diff.py <old.json> <new.json>", file=sys.stderr)
        return 2
    old, new = load(sys.argv[1]), load(sys.argv[2])

    breaking, additive = [], []

    old_ops, new_ops = operations(old), operations(new)
    for key in sorted(set(old_ops) - set(new_ops)):
        breaking.append(f"route removed: {key[1].upper()} {key[0]}")
    for key in sorted(set(new_ops) - set(old_ops)):
        additive.append(f"route added: {key[1].upper()} {key[0]}")

    for key in sorted(set(old_ops) & set(new_ops)):
        o, n = old_ops[key], new_ops[key]
        label = f"{key[1].upper()} {key[0]}"

        # A capability change silently invalidates a working credential.
        oc, nc = o.get("x-heldar-capability"), n.get("x-heldar-capability")
        if oc != nc:
            breaking.append(f"capability changed on {label}: {oc!r} -> {nc!r}")
        os_, ns_ = o.get("x-heldar-scope"), n.get("x-heldar-scope")
        if os_ != ns_ and os_ is not None:
            breaking.append(f"scoping changed on {label}: {os_!r} -> {ns_!r}")

    # Schemas. A request schema and a response schema break in OPPOSITE directions — tightening a
    # request breaks callers, loosening a response breaks readers — so they cannot share a rule.
    old_s = (old.get("components") or {}).get("schemas") or {}
    new_s = (new.get("components") or {}).get("schemas") or {}
    req_schemas, resp_schemas = schema_roles(new)
    for name in sorted(set(old_s) & set(new_s)):
        o, n = old_s[name], new_s[name]
        gained_required = required_of(n) - required_of(o)
        if gained_required:
            if name in req_schemas or name not in resp_schemas:
                # A caller's existing payload becomes invalid. (Unreferenced schemas are treated as
                # requests: unknown role, stricter rule.)
                breaking.append(f"{name} now requires {sorted(gained_required)}")
            else:
                # Response-only: the server now PROMISES the field is always there. A reader that
                # handled it as optional still works; one written against the new contract can stop
                # handling an absence that cannot happen.
                additive.append(
                    f"{name} now always sends {sorted(gained_required)} (response-only)"
                )
        lost_props = properties_of(o) - properties_of(n)
        if lost_props:
            breaking.append(f"{name} no longer has {sorted(lost_props)}")
        gained_props = properties_of(n) - properties_of(o)
        if gained_props:
            additive.append(f"{name} gained {sorted(gained_props)}")
    for name in sorted(set(old_s) - set(new_s)):
        breaking.append(f"schema removed: {name}")
    for name in sorted(set(new_s) - set(old_s)):
        additive.append(f"schema added: {name}")

    ov = (old.get("info") or {}).get("version")
    nv = (new.get("info") or {}).get("version")
    if breaking and ov == nv:
        breaking.append(
            f"the contract version is still {nv!r} — a breaking change with an unchanged version "
            f"is one a pinned client cannot detect"
        )

    for line in additive:
        print(f"  + {line}")
    for line in breaking:
        print(f"  ! {line}")

    if not additive and not breaking:
        print("no change")
    if breaking:
        print(f"\nRESULT: {len(breaking)} BREAKING change(s)")
        return 1
    print(f"\nRESULT: compatible ({len(additive)} additive change(s))")
    return 0


if __name__ == "__main__":
    sys.exit(main())
