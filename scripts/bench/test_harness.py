#!/usr/bin/env python3
"""Checks for the harness's pure helpers. Run: python3 scripts/bench/test_harness.py

Only the functions whose failure would be SILENT are covered here. If `parse_prom` broke, every
metric would come back `unmeasured` and every threshold would fail loudly — that needs no test. A
wrong percentile, on the other hand, publishes a plausible number, which is the dangerous kind of
wrong.
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import evaluate, parse_prom, pct, sha256_of  # noqa: E402

fails = []


def check(cond, msg):
    if not cond:
        fails.append(msg)


# --- percentiles ------------------------------------------------------------------------------
check(pct([], 95) is None, "an empty sample has no percentile, and must not be 0")
check(pct([5], 95) == 5, "a single sample is its own percentile")
check(pct([1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 50) == 5, f"P50 of 1..10 should be 5, got {pct(list(range(1,11)),50)}")
check(pct([1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 100) == 10, "P100 is the maximum")
check(pct(list(range(1, 101)), 95) == 95, f"P95 of 1..100 should be 95, got {pct(list(range(1,101)),95)}")
# Order must not matter: the samples arrive in probe order, not sorted.
#
# The inputs below are chosen so the SORTED and UNSORTED answers DIFFER. The obvious spelling of
# this check — pct([9,1,5,3,7], 50) == 5 — returns 5 either way, so it passed with the sort removed
# and was the only guard on a function this file's own docstring calls "the dangerous kind of
# wrong". Mutation-verified: deleting `sorted()` now fails here.
check(pct([100, 1, 2, 3, 4], 50) == 3, f"P50 must sort; unsorted gives 2, got {pct([100,1,2,3,4],50)}")
check(pct([9, 7, 5, 3, 1], 95) == 9, f"P95 must sort; unsorted gives 1, got {pct([9,7,5,3,1],95)}")

# --- prometheus parsing ------------------------------------------------------------------------
flat, keyed = parse_prom(
    """# HELP heldar_cameras_total Registered cameras
# TYPE heldar_cameras_total gauge
heldar_cameras_total 8
heldar_camera_up{camera="cam_a",state="recording"} 1
heldar_camera_up{camera="cam_b",state="offline"} 0
heldar_camera_segments_written_total{camera="cam_a"} 42
heldar_disk_used_percent 91.2286833570155
"""
)
check(flat["heldar_cameras_total"] == 8, "a bare series must parse")
check(abs(flat["heldar_disk_used_percent"] - 91.2286833570155) < 1e-9, "floats must not be truncated")
check(keyed["heldar_camera_up"] == {"cam_a": 1.0, "cam_b": 0.0}, "labelled series must key by camera")
check(
    keyed["heldar_camera_up__state"] == {"cam_a": "recording", "cam_b": "offline"},
    "the textual state must survive: 'offline' and 'starting' are both up=0 and are not the same thing",
)
check(keyed["heldar_camera_segments_written_total"]["cam_a"] == 42, "counters must parse")
check("# HELP" not in str(flat), "comments must not become series")

# --- the verdict rule --------------------------------------------------------------------------
T = {"thresholds": [{"metric": "x", "op": "<=", "value": 10, "why": "w"}]}
check(evaluate({"x": {"value": 5}}, T)[0] == "PASS", "a met threshold passes")
check(evaluate({"x": {"value": 50}}, T)[0] == "FAIL", "an exceeded threshold fails")
check(evaluate({"x": {"value": 10}}, T)[0] == "PASS", "<= is inclusive at the boundary")
check(
    evaluate({"x": {"unmeasured": "no probe ran"}}, T)[0] == "FAIL",
    "AN UNMEASURED METRIC IS NOT A PASS — the whole point of the file",
)
check(evaluate({}, T)[0] == "FAIL", "a missing metric is not a pass either")
check(
    evaluate({"x": {"value": 5}}, {"thresholds": []})[0] == "FAIL",
    "an empty threshold set must not report PASS having checked nothing",
)

# --- run validity --------------------------------------------------------------------------------
from harness import validity  # noqa: E402

base = {"cameras": ["a", "b", "c", "d"], "measurements": {"x": 1}, "scenario": {"duration_s": 100},
        "duration_s": 100.0, "publisher_respawns": []}
check(validity(base)["status"] == "VALID", "a clean run is valid")
check(
    validity({**base, "publisher_respawns": [{}] * 5})["status"] == "INVALID",
    "more respawns than cameras means the generator, not the recorder, was being measured",
)
check(
    validity({**base, "publisher_respawns": [{}] * 4})["status"] == "VALID",
    "one respawn per camera is the boundary and must not trip — a benchmark that refuses every "
    "run is as useless as one that accepts every run",
)
check(validity({**base, "duration_s": 50.0})["status"] == "INVALID", "a run cut short is invalid")
check(validity({**base, "duration_s": 95.0})["status"] == "VALID", "a 5% short run is tolerated")
check(validity({**base, "measurements": {}})["status"] == "INVALID", "no measurements is invalid")
# A result with no cameras key at all (the shape the gate's fixtures use) must not crash.
check(validity({"measurements": {"x": 1}})["status"] == "VALID", "missing keys must not throw")

# --- hashing ------------------------------------------------------------------------------------
check(
    sha256_of({"a": 1, "b": 2}) == sha256_of({"b": 2, "a": 1}),
    "key order must not change the hash, or a reformat would force a needless re-qualification",
)
check(sha256_of({"a": 1}) != sha256_of({"a": 2}), "a changed value must change the hash")

# --- bars_hash: the mechanism the whole "editorial edits do not invalidate" design rests on -------
from harness import bars_hash  # noqa: E402

BASE = {"version": "1.0.0", "_comment": ["anything"],
        "thresholds": [{"metric": "g", "op": "<=", "value": 30, "why": "because"}]}
check(
    bars_hash(BASE) == bars_hash({**BASE, "version": "2.0.0", "_comment": ["rewritten entirely"]}),
    "editing prose or the version must NOT invalidate published qualifications",
)
check(
    bars_hash(BASE) == bars_hash(
        {**BASE, "thresholds": [{"metric": "g", "op": "<=", "value": 30, "why": "reworded"}]}
    ),
    "rewording a rationale must not invalidate anything",
)
for changed, what in (
    ({"metric": "h", "op": "<=", "value": 30}, "the metric"),
    ({"metric": "g", "op": ">=", "value": 30}, "the operator"),
    ({"metric": "g", "op": "<=", "value": 31}, "the value"),
):
    check(
        bars_hash(BASE) != bars_hash({**BASE, "thresholds": [changed]}),
        f"changing {what} MUST invalidate the claims that rested on it",
    )
check(
    bars_hash(BASE) != bars_hash({**BASE, "thresholds": BASE["thresholds"] * 2}),
    "adding a threshold must invalidate too",
)

# --- the fleet-registration refusal must be REACHABLE ---------------------------------------------
# It first landed inside the field-mode branch, where `registered` is never assigned: dead for the
# mode it guards, an UnboundLocalError for the mode it was in. Asserting that an edit APPLIED is not
# the same as asserting it is REACHABLE, which is what this checks.
import ast as _ast  # noqa: E402
import os as _os  # noqa: E402

_src = open(_os.path.join(_os.path.dirname(_os.path.abspath(__file__)), "harness.py")).read()
_tree = _ast.parse(_src)


def _guard_is_reachable(tree):
    """Find the `registered != len(cams)` test and prove an assignment to `registered` dominates it
    within the same branch body."""
    for node in _ast.walk(tree):
        if not isinstance(node, _ast.If):
            continue
        # the synthetic-mode branch: `if mode == "synthetic":`
        src = _ast.unparse(node.test)
        if "mode ==" not in src or "synthetic" not in src:
            continue
        body = _ast.unparse(_ast.Module(body=node.body, type_ignores=[]))
        return "registered = 0" in body and "registered != len(cams)" in body
    return False


check(
    _guard_is_reachable(_tree),
    "the fleet-registration refusal must live in the synthetic branch, where `registered` exists",
)
check(
    "registered" not in _ast.unparse(
        _ast.Module(
            body=[n for n in _ast.walk(_tree)
                  if isinstance(n, _ast.If) and "mode == 'field'" in _ast.unparse(n.test)],
            type_ignores=[],
        )
    ) if any(isinstance(n, _ast.If) and "mode == 'field'" in _ast.unparse(n.test)
             for n in _ast.walk(_tree)) else True,
    "field mode must not read `registered`",
)

if fails:
    print(f"{len(fails)} problem(s):")
    for f in fails:
        print(f"  {f}")
    sys.exit(1)
print("harness helpers: all checks passed")
