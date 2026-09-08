#!/usr/bin/env python3
"""Controls for scripts/check_pinned_versions.py.

Run: python3 scripts/test_pinned_versions.py

The first case reproduces the ACTUAL defect this guard was written for, by mutating the fixed tree
back to what Dependabot #87 would have merged. A guard that cannot re-catch the thing that motivated
it is decoration.

The third case is the one that keeps the guard honest: if its regexes stop matching the files they
read, it must SAY so rather than silently comparing nothing and passing.

ANCHORS ARE PATTERNS, NOT LITERAL VERSIONS. Six of these seven controls used to anchor on the exact
version in the tree — `bluenviron/mediamtx:1.20.1`, `node:24.20.0-bookworm-slim@`. That makes a
control go VACUOUS on precisely the pull request that bumps its dependency, which is the one moment
the guard most needs to work: Dependabot's MediaMTX bump and the node 22 -> 24 bump each silenced
their own control this way. `Anchor` matches whatever version is there now and rewrites it to a
sentinel, so a bump changes nothing about whether the control fires.
"""

import os
import re
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CHECK = os.path.join(ROOT, "scripts", "check_pinned_versions.py")

class Anchor:
    """A version-shaped anchor: match whatever is pinned now, rewrite it to something else.

    A literal anchor names the version in the tree today, so the control silences itself the moment
    that version changes — on the very pull request doing the changing. This matches the SHAPE and
    substitutes a sentinel, so it keeps working across bumps without anybody remembering to edit it.
    """

    def __init__(self, pattern: str, replacement: str):
        self.rx = re.compile(pattern)
        self.replacement = replacement

    def count(self, src: str) -> int:
        return len(self.rx.findall(src))

    def apply(self, src: str) -> str:
        return self.rx.sub(self.replacement, src)

    def __repr__(self) -> str:
        return f"Anchor({self.rx.pattern!r})"


def apply_mutation(src: str, old, new: str) -> str:
    return old.apply(src) if isinstance(old, Anchor) else src.replace(old, new)


def count_anchor(src: str, old) -> int:
    return old.count(src) if isinstance(old, Anchor) else src.count(old)


CASES = [
    (
        "the shipped bug: compose bumped, setup script left behind",
        "scripts/setup_caddy.sh",
        Anchor(r'VERSION="\$\{CADDY_VERSION:-[\d.]+\}"', 'VERSION="${CADDY_VERSION:-0.0.1}"'),
        None,
        "they disagree",
    ),
    (
        "the two MediaMTX pins drifting apart",
        "deploy/compose.yml",
        Anchor(r"bluenviron/mediamtx:[\d.]+", "bluenviron/mediamtx:0.0.1"),
        None,
        "differs between the dev stack",
    ),
    (
        "the drift merging #78 actually left: requirements moved, the recipe did not",
        "apps/ai/Dockerfile",
        Anchor(r'"lap>=[\d.]+"', '"lap>=0.0.1"'),
        None,
        # Version-free on purpose: the sentinel this control writes is not a real version, and
        # asserting on one would re-introduce the fragility the Anchor removed.
        "tells operators to install lap>=",
    ),
    (
        "the drift this check was written for: a base image bumped, the policy table left behind",
        "apps/ai/Dockerfile",
        Anchor(r"FROM python:[\d.]+-slim@", "FROM python:0.0.1-slim@"),
        None,
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
        "the shape #144 proposed: the builder image moved, CI left behind",
        "apps/web/Dockerfile",
        Anchor(r"FROM node:[\d.]+-bookworm-slim@", "FROM node:0.0.1-bookworm-slim@"),
        None,
        "different toolchains",
    ),
    (
        # Three occurrences, and the guard reads all of them — so the control changes all three and
        # SAYS it means three. The harness refuses an ambiguous anchor rather than mutating one at
        # random, which is how a control ends up proving nothing.
        "the node parser drifting (CI stops quoting the version)",
        ".github/workflows/ci.yml",
        'node-version: "24"',
        "node-version: 24",
        "parser has drifted",
        3,
    ),
    (
        "the guard's own parser drifting from the file it reads",
        "scripts/setup_caddy.sh",
        # Drops the ${CADDY_VERSION:-...} shape the guard's regex depends on, keeping whatever
        # version is actually pinned — so this still exercises the parser after a Caddy bump.
        Anchor(r'VERSION="\$\{CADDY_VERSION:-([\d.]+)\}"', r'VERSION="\1"'),
        None,
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
        n = count_anchor(src, old)
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
        mutated = apply_mutation(src, old, new)
        if mutated == src:
            # A mutation that changes nothing tests nothing, however many times the anchor matched.
            print(f"  VACUOUS {name}: the mutation left {rel} byte-identical")
            bad += 1
            continue
        shutil.copy(path, path + ".bak")
        try:
            open(path, "w").write(mutated)
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
