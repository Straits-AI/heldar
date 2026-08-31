#!/usr/bin/env python3
"""Negative controls for the capacity gate (#119). Run: python3 scripts/bench/test_capacity_gate.py

A gate is only worth having if it refuses. Each case below breaks ONE thing and asserts the gate
fails for THAT reason — not merely that it failed, which a gate can do for the wrong reason and
still look healthy. The positive case at the end proves the gate can also say yes; without it, a
gate that refuses everything would pass this file.
"""

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, os.path.join(ROOT, "scripts", "bench"))
from harness import SCHEMA, bars_hash, evaluate, sha256_of  # noqa: E402

THRESHOLDS = json.load(open(os.path.join(ROOT, "scripts", "bench", "thresholds.json")))

PASSING = {
    "unexplained_gap_seconds_per_camera_hour": {"value": 0.0, "unit": "s/camera-hour", "n": 8},
    "unplayable_segment_count": {"value": 0, "unit": "segments", "n": 1},
    "recorder_reconnect_seconds_p95": {"value": 20.0, "unit": "s", "n": 8},
    "restart_recovery_seconds": {"value": 14.0, "unit": "s", "n": 1},
    "liveview_failure_rate": {"value": 0.0, "unit": "ratio", "n": 30},
    "liveview_seconds_p95": {"value": 1.2, "unit": "s", "n": 30},
    "snapshot_failure_rate": {"value": 0.0, "unit": "ratio", "n": 30},
    "snapshot_seconds_p95": {"value": 4.0, "unit": "s", "n": 30},
    "clip_success_rate": {"value": 1.0, "unit": "ratio", "n": 30},
    "clip_seconds_p95": {"value": 0.2, "unit": "s", "n": 30},
    "api_5xx_rate": {"value": 0.0, "unit": "ratio", "n": 200},
    "api_seconds_p95": {"value": 0.05, "unit": "s", "n": 200},
}


def result(**over):
    r = {
        "schema": SCHEMA,
        "run_id": "qual-8cam-h264-20260101T000000Z",
        "scenario_name": "qual-8cam-h264",
        "scenario": {"cameras": 8, "codec": "h264", "bitrate_kbps": 2000, "ai_profile": "off"},
        "thresholds": THRESHOLDS,
        "thresholds_sha256": sha256_of(THRESHOLDS),
        "thresholds_bars_sha256": bars_hash(THRESHOLDS),
        "provenance": {"hardware_class": "appliance-n100", "git_sha": "deadbeef"},
        "measurements": dict(PASSING),
    }
    r.update(over)
    r["verdict"] = evaluate(r["measurements"], r["thresholds"])[0]
    if "verdict" in over:
        r["verdict"] = over["verdict"]
    return r


ROW = "| 8-camera 720p | 8 | H.264 | 2 Mbps | off | appliance-n100 | qualified: `{path}` |"
HEADER = (
    "# Sizing\n\n<!-- qualification-table -->\n\n"
    "| Profile | Cameras | Codec | Bitrate | AI | Hardware class | Status |\n"
    "| --- | --- | --- | --- | --- | --- | --- |\n"
)


def run_gate(doc_text, result_obj=None, result_rel="docs/benchmarks/results/t.json"):
    """Run the real verifier against a temporary tree that mirrors the repo layout."""
    tmp = tempfile.mkdtemp(prefix="capgate-")
    try:
        os.makedirs(os.path.join(tmp, "scripts", "bench"), exist_ok=True)
        os.makedirs(os.path.join(tmp, "docs", "benchmarks", "results"), exist_ok=True)
        for f in ("harness.py", "thresholds.json"):
            shutil.copy(os.path.join(ROOT, "scripts", "bench", f),
                        os.path.join(tmp, "scripts", "bench", f))
        shutil.copy(os.path.join(ROOT, "scripts", "verify_capacity_claims.py"),
                    os.path.join(tmp, "scripts", "verify_capacity_claims.py"))
        if result_obj is not None:
            p = os.path.join(tmp, result_rel)
            os.makedirs(os.path.dirname(p), exist_ok=True)
            json.dump(result_obj, open(p, "w"), indent=2)
        doc = os.path.join(tmp, "docs", "sizing.md")
        open(doc, "w").write(doc_text)
        r = subprocess.run(
            [sys.executable, os.path.join(tmp, "scripts", "verify_capacity_claims.py"), doc],
            capture_output=True, text=True,
        )
        return r.returncode, r.stdout + r.stderr
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


CASES = []


def case(name, expect_rc, expect_text):
    def deco(fn):
        CASES.append((name, fn, expect_rc, expect_text))
        return fn
    return deco


@case("a valid row is accepted", 0, "RESULT: PASS")
def _():
    return HEADER + ROW.format(path="docs/benchmarks/results/t.json") + "\n", result()


@case("an EXTRAPOLATED row is accepted, because an honest extrapolation is useful", 0, "RESULT: PASS")
def _():
    return (HEADER + "| 32-camera 1080p | 32 | H.265 | 4 Mbps | off | appliance-n100 "
            "| EXTRAPOLATED |\n"), None


@case("no qualification table at all", 1, "has no <!-- qualification-table -->")
def _():
    return "# Sizing\n\nEight to sixteen cameras.\n", None


@case("a table with no rows cannot report success", 1, "empty check")
def _():
    return HEADER, None


@case("a row with no stated basis", 1, "is not `qualified:")
def _():
    return (HEADER + "| 8-camera | 8 | H.264 | 2 Mbps | off | appliance-n100 | yes |\n"), None


@case("a cited result that does not exist", 1, "which does not exist")
def _():
    return HEADER + ROW.format(path="docs/benchmarks/results/nope.json") + "\n", None


@case("a cited run that FAILED", 1, "did NOT pass")
def _():
    m = dict(PASSING)
    m["unexplained_gap_seconds_per_camera_hour"] = {"value": 900.0, "unit": "s/camera-hour", "n": 8}
    return HEADER + ROW.format(path="docs/benchmarks/results/t.json") + "\n", result(measurements=m)


@case("a run whose verdict field was edited to PASS", 1, "the file has been edited")
def _():
    m = dict(PASSING)
    m["unplayable_segment_count"] = {"value": 7, "unit": "segments", "n": 1}
    return (HEADER + ROW.format(path="docs/benchmarks/results/t.json") + "\n",
            result(measurements=m, verdict="PASS"))


@case("an unmeasured metric is not a pass", 1, "did NOT pass")
def _():
    m = dict(PASSING)
    m["restart_recovery_seconds"] = {"unmeasured": "the scenario injected no core restart"}
    return HEADER + ROW.format(path="docs/benchmarks/results/t.json") + "\n", result(measurements=m)


@case("thresholds loosened after the run", 1, "bar moved after the run")
def _():
    loosened = json.loads(json.dumps(THRESHOLDS))
    loosened["version"] = "9.9.9"
    for t in loosened["thresholds"]:
        if t["metric"] == "unexplained_gap_seconds_per_camera_hour":
            t["value"] = 100000
    r = result()
    r["thresholds"] = loosened
    r["thresholds_sha256"] = sha256_of(loosened)
    r["thresholds_bars_sha256"] = bars_hash(loosened)
    r["verdict"] = evaluate(r["measurements"], loosened)[0]
    return HEADER + ROW.format(path="docs/benchmarks/results/t.json") + "\n", r


@case("thresholds edited inside the result file", 1, "do not match its own recorded hash")
def _():
    r = result()
    r["thresholds"] = json.loads(json.dumps(THRESHOLDS))
    r["thresholds"]["thresholds"][0]["value"] = 999999
    return HEADER + ROW.format(path="docs/benchmarks/results/t.json") + "\n", r


@case("an editorial edit does NOT invalidate a claim", 0, "RESULT: PASS")
def _():
    # The control that keeps the rule proportionate: rewording a rationale must not force every
    # published profile to be re-run, or people route around the rule.
    reworded = json.loads(json.dumps(THRESHOLDS))
    reworded["thresholds"][0]["why"] = "reworded for clarity, same bar"
    reworded["version"] = "1.1.1"
    r = result()
    r["thresholds"] = reworded
    r["thresholds_sha256"] = sha256_of(reworded)
    r["thresholds_bars_sha256"] = bars_hash(reworded)
    return HEADER + ROW.format(path="docs/benchmarks/results/t.json") + "\n", r


@case("16 cameras claimed, 8 measured", 1, "the row says 16 cameras")
def _():
    row = "| 16-camera | 16 | H.264 | 2 Mbps | off | appliance-n100 | qualified: `docs/benchmarks/results/t.json` |"
    return HEADER + row + "\n", result()


@case("H.265 claimed, H.264 measured", 1, "the row says codec")
def _():
    row = "| 8-camera | 8 | H.265 | 2 Mbps | off | appliance-n100 | qualified: `docs/benchmarks/results/t.json` |"
    return HEADER + row + "\n", result()


@case("4 Mbps claimed, 2 Mbps measured", 1, "the row says 4 Mbps")
def _():
    row = "| 8-camera | 8 | H.264 | 4 Mbps | off | appliance-n100 | qualified: `docs/benchmarks/results/t.json` |"
    return HEADER + row + "\n", result()


@case("AI claimed, AI off in the run", 1, "the row says AI")
def _():
    row = "| 8-camera | 8 | H.264 | 2 Mbps | yolo | appliance-n100 | qualified: `docs/benchmarks/results/t.json` |"
    return HEADER + row + "\n", result()


@case("an appliance qualified by a laptop run", 1, "the row says hardware")
def _():
    row = "| 8-camera | 8 | H.264 | 2 Mbps | off | appliance-n100 | qualified: `docs/benchmarks/results/t.json` |"
    r = result()
    r["provenance"]["hardware_class"] = "dev-laptop"
    return HEADER + row + "\n", r


@case("a run whose generator kept dying is INVALID, not passing", 1, "is INVALID")
def _():
    # Nine respawns across eight cameras: the host could not sustain the encode load, so the run
    # measured the generator. Every threshold below still reads green, which is exactly why this
    # has to be checked separately from the verdict.
    r = result()
    r["cameras"] = [f"bench_{i:03d}" for i in range(1, 9)]
    r["publisher_respawns"] = [{"camera": "bench_001", "t": float(i)} for i in range(9)]
    return HEADER + ROW.format(path="docs/benchmarks/results/t.json") + "\n", r


@case("a run cut short of its declared duration is INVALID", 1, "cut short")
def _():
    r = result()
    r["scenario"] = dict(r["scenario"], duration_s=1800)
    r["duration_s"] = 400.0
    return HEADER + ROW.format(path="docs/benchmarks/results/t.json") + "\n", r


@case("a few respawns on a large fleet is still a valid run", 0, "RESULT: PASS")
def _():
    # The control that stops the validity rule being a blanket refusal: two transient respawns
    # across eight cameras is a benchmark, not a broken one.
    r = result()
    r["cameras"] = [f"bench_{i:03d}" for i in range(1, 9)]
    r["publisher_respawns"] = [{"camera": "bench_001", "t": 1.0}, {"camera": "bench_002", "t": 2.0}]
    return HEADER + ROW.format(path="docs/benchmarks/results/t.json") + "\n", r


@case("a second table under the same marker is NOT silently skipped", 1, "does not exist")
def _():
    # The parser used to stop at the first blank line, so a second table — the obvious way someone
    # adds "long-run profiles" — was published unverified while the gate reported PASS.
    doc = (HEADER + ROW.format(path="docs/benchmarks/results/t.json") + "\n\n"
           + "Some prose about long runs.\n\n"
           + "| Profile | Cameras | Codec | Bitrate | AI | Hardware class | Status |\n"
           + "| --- | --- | --- | --- | --- | --- | --- |\n"
           + "| Long run | 32 | H.265 | 4 Mbps | off | rk3588 | qualified: `docs/benchmarks/results/nope.json` |\n")
    return doc, result()


@case("two qualification markers are refused rather than resolved", 1, "more than one")
def _():
    return (HEADER + ROW.format(path="docs/benchmarks/results/t.json") + "\n\n"
            + "<!-- qualification-table -->\n\n"
            + "| Decoy | 99 | H.265 | 9 Mbps | off | nowhere | EXTRAPOLATED |\n"), result()


@case("a real row whose first cell says Profile is still checked", 1, "does not exist")
def _():
    # Skipping any row whose first cell was "profile" let a real claim hide behind that word.
    row = "| Profile | 8 | H.264 | 2 Mbps | off | appliance-n100 | qualified: `docs/benchmarks/results/nope.json` |"
    return HEADER + row + "\n", result()


@case("a bitrate cell with no number is refused, not skipped", 1, "not a number with a unit")
def _():
    row = "| 8-camera | 8 | H.264 | see below | off | appliance-n100 | qualified: `docs/benchmarks/results/t.json` |"
    return HEADER + row + "\n", result()


@case("a bitrate in the wrong unit is caught", 1, "the row says 2 kbps")
def _():
    # The old parser stripped non-digits, so "2 kbps" and "2 Mbps" were the same claim.
    row = "| 8-camera | 8 | H.264 | 2 kbps | off | appliance-n100 | qualified: `docs/benchmarks/results/t.json` |"
    return HEADER + row + "\n", result()


@case("kbps and Mbps are both accepted when they agree", 0, "RESULT: PASS")
def _():
    row = "| 8-camera | 8 | H.264 | 2000 kbps | off | appliance-n100 | qualified: `docs/benchmarks/results/t.json` |"
    return HEADER + row + "\n", result()


@case("an empty codec cell no longer matches every run", 1, "the row says codec")
def _():
    # `in` made "" a substring of every codec, so a blank cell qualified against anything.
    row = "| 8-camera | 8 |  | 2 Mbps | off | appliance-n100 | qualified: `docs/benchmarks/results/t.json` |"
    return HEADER + row + "\n", result()


@case("a truncated codec no longer matches by substring", 1, "the row says codec")
def _():
    row = "| 8-camera | 8 | h26 | 2 Mbps | off | appliance-n100 | qualified: `docs/benchmarks/results/t.json` |"
    return HEADER + row + "\n", result()


@case("a non-numeric cameras cell is refused", 1, "not a plain number")
def _():
    row = "| 8-camera | eight | H.264 | 2 Mbps | off | appliance-n100 | qualified: `docs/benchmarks/results/t.json` |"
    return HEADER + row + "\n", result()


@case("a lying thresholds_bars_sha256 cannot launder loosened bars", 1, "has been edited")
def _():
    # THE HOLE THE REVIEW FOUND: the gate preferred the file's own bars hash, so loosening a bar
    # inside the result and leaving the field showing the tree's hash passed the drift check while
    # evaluate() graded against the loosened bar.
    loosened = json.loads(json.dumps(THRESHOLDS))
    for t in loosened["thresholds"]:
        if t["metric"] == "unexplained_gap_seconds_per_camera_hour":
            t["value"] = 100000
    m = dict(PASSING)
    m["unexplained_gap_seconds_per_camera_hour"] = {"value": 900.0, "unit": "s/camera-hour", "n": 8}
    r = result(measurements=m)
    r["thresholds"] = loosened
    r["thresholds_sha256"] = sha256_of(loosened)
    r["thresholds_bars_sha256"] = bars_hash(THRESHOLDS)   # the lie
    r["verdict"] = evaluate(m, loosened)[0]               # PASS, against the loosened bar
    return HEADER + ROW.format(path="docs/benchmarks/results/t.json") + "\n", r


@case("a result from a future schema", 1, "expected 'heldar-benchmark/1'")
def _():
    return (HEADER + ROW.format(path="docs/benchmarks/results/t.json") + "\n",
            result(schema="heldar-benchmark/2"))


def main():
    failed = 0
    for name, fn, want_rc, want_text in CASES:
        doc, res = fn()
        rc, out = run_gate(doc, res)
        ok = rc == want_rc and want_text in out
        if not ok:
            failed += 1
            print(f"  FAIL  {name}\n        rc={rc} (want {want_rc}); wanted {want_text!r} in:\n"
                  + "\n".join("        " + l for l in out.strip().splitlines()))
        else:
            print(f"  ok    {name}")
    print(f"\n{len(CASES) - failed}/{len(CASES)} controls behaved as specified")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
