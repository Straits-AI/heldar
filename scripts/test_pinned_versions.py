#!/usr/bin/env python3
"""Controls for scripts/check_pinned_versions.py.

Run: python3 scripts/test_pinned_versions.py

The first case reproduces the ACTUAL defect this guard was written for, by mutating the fixed tree
back to what Dependabot #87 would have merged. A guard that cannot re-catch the thing that motivated
it is decoration.

The third case is the one that keeps the guard honest: if its regexes stop matching the files they
read, it must SAY so rather than silently comparing nothing and passing.
"""

import os
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CHECK = os.path.join(ROOT, "scripts", "check_pinned_versions.py")

CASES = [
    (
        "the shipped bug: compose bumped, setup script left behind",
        "scripts/setup_caddy.sh",
        'VERSION="${CADDY_VERSION:-2.11.4}"',
        'VERSION="${CADDY_VERSION:-2.10.2}"',
        "they disagree",
    ),
    (
        "the two MediaMTX pins drifting apart",
        "deploy/compose.yml",
        "bluenviron/mediamtx:1.20.1",
        "bluenviron/mediamtx:1.20.0",
        "differs between the dev stack",
    ),
    (
        "the drift merging #78 actually left: requirements moved, the recipe did not",
        "apps/ai/Dockerfile",
        '"lap>=0.5.13"',
        '"lap>=0.5"',
        "tells operators to install lap>=0.5",
    ),
    (
        "the drift this check was written for: a base image bumped, the policy table left behind",
        "apps/ai/Dockerfile",
        "FROM python:3.14.7-slim@",
        "FROM python:3.15.0-slim@",
        "does not pin that image",
    ),
    (
        "the policy table's parser drifting (the heading it anchors on is renamed)",
        "docs/SUPPLY-CHAIN.md",
        "## What is pinned",
        "## Pinned images",
        "parser has drifted",
    ),
    (
        "what #144 proposed: the image on node 26, CI still testing on 22",
        "apps/web/Dockerfile",
        "FROM node:22.23.2-bookworm-slim@",
        "FROM node:26.8.1-bookworm-slim@",
        "different toolchains",
    ),
    (
        # Three occurrences, and the guard reads all of them — so the control changes all three and
        # SAYS it means three. The harness refuses an ambiguous anchor rather than mutating one at
        # random, which is how a control ends up proving nothing.
        "the node parser drifting (CI stops quoting the version)",
        ".github/workflows/ci.yml",
        'node-version: "22"',
        "node-version: 22",
        "parser has drifted",
        3,
    ),
    (
        "the guard's own parser drifting from the file it reads",
        "scripts/setup_caddy.sh",
        'VERSION="${CADDY_VERSION:-2.11.4}"',
        'VERSION="2.11.4"',
        "parser has drifted",
    ),
]


def run():
    return subprocess.run([sys.executable, CHECK], capture_output=True, text=True)


def main():
    bad = 0
    for case in CASES:
        name, rel, old, new, want = case[:5]
        expect_n = case[5] if len(case) > 5 else None
        path = os.path.join(ROOT, rel)
        src = open(path).read()
        n = src.count(old)
        if n == 0 or (expect_n is not None and n != expect_n):
            print(f"  VACUOUS {name}: anchor appears {n} times in {rel}"
                  + (f" (expected {expect_n})" if expect_n is not None else ""))
            bad += 1
            continue
        if expect_n is None and n != 1:
            print(f"  VACUOUS {name}: anchor appears {n} times in {rel} and no count was declared — "
                  f"say how many you mean to change")
            bad += 1
            continue
        shutil.copy(path, path + ".bak")
        try:
            open(path, "w").write(src.replace(old, new))
            r = run()
            ok = r.returncode == 1 and want in r.stdout
            print(("  ok    " if ok else "  FAIL  ") + name)
            if not ok:
                bad += 1
                print(f"        rc={r.returncode}, wanted {want!r} in:\n        "
                      + r.stdout.strip()[-260:])
        finally:
            shutil.move(path + ".bak", path)

    r = run()
    ok = r.returncode == 0
    print(("  ok    " if ok else "  FAIL  ") + "the tree as committed passes")
    if not ok:
        bad += 1
        print("        " + r.stdout.strip()[-260:])

    total = len(CASES) + 1
    print(f"\n{total - bad}/{total} controls behaved as specified")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
