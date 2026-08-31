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
check(pct([9, 1, 5, 3, 7], 50) == 5, "the percentile must sort its input")

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

if fails:
    print(f"{len(fails)} problem(s):")
    for f in fails:
        print(f"  {f}")
    sys.exit(1)
print("harness helpers: all checks passed")
