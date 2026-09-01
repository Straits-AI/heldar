#!/usr/bin/env python3
"""Lint the skill bundle (#124). Fails CLOSED.

A skill is prose an agent follows. Prose cannot be compiled, so the parts that CAN be checked
mechanically are checked here, and the parts that cannot are at least required to exist.

What this refuses, and why each one matters:

  * A `permitted_tools` entry that does not exist in `heldar-mcp` or `heldarctl`. A skill naming a
    tool that is not there teaches an agent to hallucinate one, and a hallucinated tool call becomes
    a hallucinated RESULT the moment a model decides to be helpful.
  * A `permitted_tools` entry that mutates, unless the skill declares `mutating: true`. The initial
    bundle is read-only and the acceptance criteria say no skill may actuate, delete or administer.
  * A missing safety rule. The common set is not advisory — a skill that quietly drops "check
    recording gaps before asserting nothing happened" is the skill that produces a confident,
    wrong "nothing happened".
  * A missing required section, especially `Stop conditions`: it is where the skill says stop and
    ask a human, and it is the section an author under time pressure skips.
  * A compatibility range that does not parse, or that excludes the contract this repo ships. A
    skill pinned to an API that no longer exists is worse than no skill.

Usage: validate_skills.py [skills-dir]
"""

import json
import os
import re
import sys

REQUIRED_SECTIONS = [
    "Purpose",
    "Inputs",
    "Prerequisites",
    "Workflow",
    "Stop conditions",
    "Output",
]

# Every skill must carry all of these. Kept here rather than in each skill so dropping one is a
# lint failure rather than an editorial choice nobody notices.
REQUIRED_PROHIBITIONS = [
    "actuate a gate, relay or PTZ",
    "delete recordings, evidence or weaken retention",
    "create, modify or retrieve credentials",
    "identify a person from appearance similarity alone",
    "assert that nothing happened without first checking recording gaps",
    "present a correlation or hypothesis as an observation",
]

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

failures = []
skipped_path_check = []


def fail(skill, msg):
    failures.append(f"{skill}: {msg}")


def known_tools():
    """Every tool name a skill may legitimately name, and whether it mutates.

    Read from the SOURCE of heldar-mcp and heldarctl rather than a list kept here — a second list
    is a list that goes stale, and this repository has learned that four times.
    """
    tools = {}

    mcp = os.path.join(ROOT, "crates/heldar-mcp/src/tools.rs")
    src = open(mcp).read()
    # `name: "x",` followed within the same Tool block by `method: "GET",`
    for block in re.findall(r"Tool \{(.*?)\n    \}", src, re.S):
        name = re.search(r'name:\s*"([^"]+)"', block)
        method = re.search(r'method:\s*"([^"]+)"', block)
        if name and method:
            tools[name.group(1)] = method.group(1) != "GET"
    if not tools:
        fail("validator", f"parsed no tools out of {mcp} — the parser is looking at the wrong shape")

    cli = os.path.join(ROOT, "crates/heldarctl/src/main.rs")
    csrc = open(cli).read()
    for m in re.findall(r'Some\("([a-z][a-z-]*)"\)\s*=>', csrc):
        # heldarctl's read-only surface; a mutating subcommand would need `mutating: true` too.
        tools[f"heldarctl {m}"] = m not in {
            "version",
            "status",
            "doctor",
            "context",
            "help",
        }
    return tools


def parse_frontmatter(text, skill):
    """Parse the small YAML subset this format uses.

    Deliberately not a YAML dependency and deliberately not a general parser: the format is ours,
    and a parser that accepts more than the format allows lets a malformed skill through. It
    handles exactly three shapes — `key: value`, a nested one-level map, and a `- ` list — and
    reports anything else rather than guessing.
    """
    if not text.startswith("---\n"):
        fail(skill, "SKILL.md does not start with YAML frontmatter")
        return None
    end = text.find("\n---\n", 3)
    if end < 0:
        fail(skill, "the frontmatter is not closed with `---`")
        return None

    data = {}
    current_list = None   # (container, key) currently being appended to
    current_map = None    # container for an indented `k: v` block
    for line in text[4:end].splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        indent = len(line) - len(line.rstrip("\n").lstrip())
        s = line.strip()

        if s.startswith("- "):
            if current_list is None:
                fail(skill, f"list item outside a list: {s!r}")
                continue
            container, key = current_list
            # The bare key pre-created an empty map because it could have been either shape. The
            # first list item is what decides; convert it once.
            if not isinstance(container.get(key), list):
                container[key] = []
            container[key].append(s[2:].strip().strip('"'))
            current_map = None
            continue

        if ":" not in s:
            fail(skill, f"unparseable frontmatter line: {s!r}")
            continue

        k, _, v = s.partition(":")
        k, v = k.strip(), v.strip().strip('"')

        if indent > 0:
            # Inside a nested map opened by a bare `key:` at indent 0.
            if current_map is None:
                fail(skill, f"indented key with no parent: {s!r}")
                continue
            current_map[k] = v
            continue

        current_list, current_map = None, None
        if v == "":
            # A bare key opens either a list or a map; the next line decides. Both are prepared.
            data[k] = {}
            current_map = data[k]
            current_list = (data, k)
        else:
            data[k] = v

    # A bare key that got list items is a list; one that got nested keys stays a map; one that got
    # neither is an empty declaration, which the checks below will reject on its own terms.
    return data


def check_range(spec, skill):
    """`>=X.Y.Z <A.B.C` — and the contract this repo ships must satisfy it."""
    m = re.fullmatch(r">=(\d+\.\d+\.\d+)\s+<(\d+\.\d+\.\d+)", spec.strip())
    if not m:
        fail(skill, f"compatible.core_api {spec!r} is not `>=X.Y.Z <A.B.C`")
        return
    lo, hi = [tuple(int(x) for x in v.split(".")) for v in m.groups()]
    src = open(os.path.join(ROOT, "crates/heldar-kernel/src/openapi.rs")).read()
    cur = re.search(r'API_VERSION: &str = "([^"]+)"', src)
    if not cur:
        fail("validator", "could not read API_VERSION from openapi.rs")
        return
    have = tuple(int(x) for x in cur.group(1).split("."))
    if not (lo <= have < hi):
        fail(
            skill,
            f"compatible.core_api {spec!r} excludes the contract this repo ships "
            f"({cur.group(1)}) — a skill pinned to an API that does not exist is worse than none",
        )


def main():
    root = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "skills")
    tools = known_tools()
    names = sorted(
        d for d in os.listdir(root) if os.path.isdir(os.path.join(root, d))
    )
    if not names:
        print(f"no skills found under {root} — refusing to report success on an empty check")
        return 1

    for name in names:
        path = os.path.join(root, name, "SKILL.md")
        if not os.path.isfile(path):
            fail(name, "has no SKILL.md")
            continue
        text = open(path).read()
        fm = parse_frontmatter(text, name)
        if fm is None:
            continue

        if fm.get("name") != name:
            fail(name, f"frontmatter name {fm.get('name')!r} does not match the directory")
        if not re.fullmatch(r"\d+\.\d+\.\d+", str(fm.get("version", ""))):
            fail(name, f"version {fm.get('version')!r} is not X.Y.Z")
        if not str(fm.get("summary", "")).strip():
            fail(name, "has no summary — an agent choosing between skills reads only this")

        compat = fm.get("compatible")
        if not isinstance(compat, dict) or "core_api" not in compat:
            fail(name, "declares no compatible.core_api range")
        else:
            check_range(compat["core_api"], name)

        declared = fm.get("permitted_tools")
        if not isinstance(declared, list) or not declared:
            fail(name, "declares no permitted_tools")
        else:
            for t in declared:
                if t not in tools:
                    fail(
                        name,
                        f"permitted tool {t!r} does not exist in heldar-mcp or heldarctl — a skill "
                        f"naming a tool that is not there teaches an agent to hallucinate one",
                    )
                elif tools[t] and str(fm.get("mutating", "false")).lower() != "true":
                    fail(name, f"permitted tool {t!r} mutates, but the skill is not `mutating: true`")

        prohibited = fm.get("prohibited_actions") or []
        for rule in REQUIRED_PROHIBITIONS:
            if rule not in prohibited:
                fail(name, f"is missing the required safety rule: {rule!r}")

        body = text[text.find("\n---\n", 3) + 5 :]

        # AN API PATH A SKILL NAMES MUST EXIST. Prose is where invention hides: the tool table is
        # checked above, but a workflow step saying "GET /api/v1/cameras/{id}/thermal" teaches an
        # agent a route that is not there, and a 404 becomes a shrug rather than a stop.
        #
        # Checked against the SERVED document when it has been written, and skipped with a note when
        # it has not — a check that silently passes because its input is missing is the failure this
        # repository keeps relearning.
        spec_path = os.path.join(ROOT, "target/openapi.json")
        if os.path.isfile(spec_path):
            spec = json.load(open(spec_path))
            real = {re.sub(r"\{[^}]+\}", "{}", x) for x in spec.get("paths", {})}
            for ref in set(re.findall(r"/api/v1/[A-Za-z0-9_\-{}/]+", text)):
                cleaned = re.sub(r"\{[^}]+\}", "{}", ref.rstrip(".,)`"))
                if cleaned not in real:
                    fail(name, f"names an API path that does not exist: {ref}")
        else:
            skipped_path_check.append(name)
        for section in REQUIRED_SECTIONS:
            if not re.search(rf"^#+\s*{re.escape(section)}\s*$", body, re.M | re.I):
                fail(name, f"has no `{section}` section")

    print(f"checked {len(names)} skill(s): {', '.join(names)}")
    if skipped_path_check:
        print(
            f"  NOTE: API-path checking was skipped for {len(skipped_path_check)} skill(s) — "
            f"target/openapi.json is absent. Run `cargo test -p heldar-server --test "
            f"openapi_contract write_the_served_document` first to enable it."
        )
    if failures:
        print(f"\n{len(failures)} problem(s):")
        for f in failures:
            print(f"  {f}")
        return 1
    print("RESULT: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
