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
    for name, rel, old, new, want in CASES:
        path = os.path.join(ROOT, rel)
        src = open(path).read()
        if src.count(old) != 1:
            print(f"  VACUOUS {name}: anchor appears {src.count(old)} times in {rel}")
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
