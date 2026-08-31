#!/usr/bin/env python3
"""Controls for scripts/check_action_outputs.py.

Run: python3 scripts/test_action_outputs.py

The first two cases are the ACTUAL bug this guard was written for, reproduced by mutating the fixed
file back to what shipped. A guard that cannot re-catch the defect that motivated it is decoration.
"""

import os
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ACTION = os.path.join(ROOT, ".github", "actions", "build-musl-binary", "action.yml")
CHECK = os.path.join(ROOT, "scripts", "check_action_outputs.py")

CASES = [
    (
        "the shipped bug: an output declared with no `value`",
        "  asset:\n    description: Path to the named heldar-core binary asset\n"
        "    value: ${{ steps.package.outputs.asset }}\n",
        "  asset:\n    description: Path to the named heldar-core binary asset\n",
        "always resolves to the empty string",
    ),
    (
        "the shipped bug: two outputs resolving to the same expression",
        "value: ${{ steps.package.outputs.cli-asset }}",
        "value: ${{ steps.package.outputs.asset }}",
        "both resolve to",
    ),
    (
        "a value naming a step that does not exist",
        "value: ${{ steps.package.outputs.mcp-asset }}",
        "value: ${{ steps.nosuchstep.outputs.mcp-asset }}",
        "has `id: nosuchstep`",
    ),
    (
        "a value naming a key no step writes to $GITHUB_OUTPUT",
        "value: ${{ steps.package.outputs.mcp-asset }}",
        "value: ${{ steps.package.outputs.never-written }}",
        "writes `never-written=`",
    ),
]


def run():
    return subprocess.run([sys.executable, CHECK], capture_output=True, text=True)


def main():
    src = open(ACTION).read()
    bad = 0
    for name, old, new, want in CASES:
        if src.count(old) != 1:
            print(f"  VACUOUS {name}: anchor appears {src.count(old)} times, so this control "
                  f"proves nothing. Fix the anchor.")
            bad += 1
            continue
        shutil.copy(ACTION, ACTION + ".bak")
        try:
            open(ACTION, "w").write(src.replace(old, new))
            r = run()
            ok = r.returncode == 1 and want in r.stdout
            print(("  ok    " if ok else "  FAIL  ") + name)
            if not ok:
                bad += 1
                print(f"        rc={r.returncode}, wanted {want!r} in:\n        "
                      + r.stdout.strip()[-300:])
        finally:
            shutil.move(ACTION + ".bak", ACTION)

    r = run()
    ok = r.returncode == 0
    print(("  ok    " if ok else "  FAIL  ") + "the tree as committed passes")
    if not ok:
        bad += 1
        print("        " + r.stdout.strip()[-300:])

    total = len(CASES) + 1
    print(f"\n{total - bad}/{total} controls behaved as specified")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
