#!/usr/bin/env python3
"""Mutation controls for scripts/check_documented_metrics.py.

Run: python3 scripts/bench/test_doc_drift.py

The drift checker's whole value is that it REFUSES. Each case below breaks exactly one claim in a
copy of the tree and asserts the checker notices; each mutation is asserted to have changed
something, so a control cannot pass by mutating text that is not there. The unmodified tree is
checked last — without it, a checker that refused everything would score full marks.
"""

import os
import shutil
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def build(tmp):
    shutil.copytree(os.path.join(ROOT, "docs"), os.path.join(tmp, "docs"))
    os.makedirs(os.path.join(tmp, "crates/heldar-kernel/src/services"), exist_ok=True)
    shutil.copy(
        os.path.join(ROOT, "crates/heldar-kernel/src/services/metrics.rs"),
        os.path.join(tmp, "crates/heldar-kernel/src/services/metrics.rs"),
    )
    os.makedirs(os.path.join(tmp, "scripts/bench"), exist_ok=True)
    shutil.copy(os.path.join(ROOT, "scripts/check_documented_metrics.py"),
                os.path.join(tmp, "scripts"))
    for f in ("harness.py", "scenarios.json"):
        shutil.copy(os.path.join(ROOT, "scripts/bench", f), os.path.join(tmp, "scripts/bench"))


def run(tmp):
    return subprocess.run(
        [sys.executable, os.path.join(tmp, "scripts/check_documented_metrics.py")],
        capture_output=True, text=True,
    )


CASES = [
    # The one that was actually shipped broken: the table was fixed and the alerting rule was not.
    ("an alerting rule drifts while the table stays correct", "docs/OBSERVABILITY.md",
     lambda t: t.replace("increase(heldar_camera_segments_written_total[10m])",
                         "increase(heldar_camera_segs[10m])", 1), "references"),
    ("a metric is dropped from the exposition table", "docs/OBSERVABILITY.md",
     lambda t: t.replace("| `heldar_cameras_total` | gauge | — | Registered cameras. |\n", "", 1),
     "does not document"),
    ("the benchmarks README names a measurement the harness does not produce",
     "docs/benchmarks/README.md",
     lambda t: t.replace("`footage_lost_per_restart_seconds`",
                         "`footage_lost_per_reboot_seconds`", 1), "does not produce"),
    ("the benchmarks README names a scenario that does not exist", "docs/benchmarks/README.md",
     lambda t: t.replace("`qual-4cam-h264`", "`qual-5cam-h264`", 1), "does not define"),
    ("a scenario is added and not listed in the matrix", "scripts/bench/scenarios.json",
     lambda t: t.replace('  "field-1h": {',
                         '  "qual-64cam-h265": {"description": "x", "duration_s": 1, '
                         '"faults": []},\n  "field-1h": {', 1), "does not list"),
    ("a scenario loses a fault the README promises every scenario injects",
     "scripts/bench/scenarios.json",
     lambda t: t.replace('      {\n        "at_s": 720,\n        "kind": "mediamtx_restart"\n'
                         '      },\n', "", 1), "is missing"),
]


def main():
    bad = 0
    for name, target, mutate, want in CASES:
        d = tempfile.mkdtemp(prefix="drift-")
        try:
            build(d)
            path = os.path.join(d, target)
            text = open(path).read()
            new = mutate(text)
            if new == text:
                print(f"  VACUOUS {name} — the mutation matched nothing, so this control proves "
                      f"nothing. Fix the anchor.")
                bad += 1
                continue
            open(path, "w").write(new)
            r = run(d)
            ok = r.returncode == 1 and want in r.stdout
            print(("  ok    " if ok else "  FAIL  ") + name)
            if not ok:
                bad += 1
                print(f"        rc={r.returncode}, wanted {want!r} in:\n        "
                      + r.stdout.strip()[-300:])
        finally:
            shutil.rmtree(d, ignore_errors=True)

    d = tempfile.mkdtemp(prefix="drift-")
    try:
        build(d)
        r = run(d)
        ok = r.returncode == 0
        print(("  ok    " if ok else "  FAIL  ") + "an unmodified tree passes")
        if not ok:
            bad += 1
            print("        " + r.stdout.strip()[-300:])
    finally:
        shutil.rmtree(d, ignore_errors=True)

    total = len(CASES) + 1
    print(f"\n{total - bad}/{total} controls behaved as specified")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
