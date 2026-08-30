#!/usr/bin/env python3
"""Refuse a capacity claim that no run supports (#119). Fails CLOSED.

`docs/sizing.md` tells people how many cameras a box will carry. That sentence is the most expensive
one in the documentation: someone buys hardware against it. This checks that every row of the
qualification table is either backed by a run that PASSED, or is labelled as an extrapolation.

What it refuses, and why each one matters:

  * A row citing a result file that does not exist, or that a reader cannot open.
  * A row citing a run that FAILED. The issue's requirement is "a failed threshold blocks the
    production-capacity claim"; this is that requirement, enforced.
  * A row whose stated profile does not match the run it cites — 16 cameras qualified by an
    8-camera run, H.265 qualified by an H.264 run. The commonest way a table drifts from its
    evidence is not fabrication, it is a row edited and a citation left behind.
  * A run judged against DIFFERENT THRESHOLDS from the ones in the tree today. Loosening a bar to
    turn a red run green therefore invalidates the claim it was loosened for, and the profile has to
    be re-run. This is the mechanical form of "avoid moving the threshold after seeing the result".
  * A row with no status at all, and a table with no rows.

The verdict is RECOMPUTED from the run's raw measurements rather than read from its `verdict` field,
using the same `evaluate()` the harness uses. A result file is a text file; a benchmark whose
conclusion can be edited with a text editor is a press release.

Usage: verify_capacity_claims.py [docs/sizing.md]
"""

import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "scripts", "bench"))

# The SAME evaluator the harness uses. Two implementations of "did this pass" is one implementation
# too many: they drift, and the one that drifts is always the one nobody runs.
from harness import SCHEMA, evaluate, sha256_of  # noqa: E402

MARKER = "<!-- qualification-table -->"

failures = []


def fail(msg):
    failures.append(msg)


def norm(s):
    return re.sub(r"\s+", " ", s.strip().lower()).replace("**", "")


def parse_table(text):
    """The rows under the qualification marker. A markdown table, because the primary reader is a
    person deciding what hardware to buy, not a program."""
    if MARKER not in text:
        return None
    after = text.split(MARKER, 1)[1]
    rows = []
    for line in after.splitlines():
        line = line.strip()
        if not line.startswith("|"):
            if rows:
                break          # the table has ended
            continue
        cells = [c.strip() for c in line.strip("|").split("|")]
        if not cells or set("".join(cells)) <= set("-: "):
            continue           # separator row
        if norm(cells[0]) in ("profile", "workload"):
            continue           # header
        rows.append(cells)
    return rows


def check_row(cells, thresholds_hash, lineno):
    if len(cells) < 7:
        fail(f"row {lineno}: expected 7 columns (profile, cameras, codec, bitrate, AI, hardware, "
             f"status), found {len(cells)}: {cells}")
        return
    profile, cameras, codec, bitrate, ai, hardware, status = cells[:7]

    if re.fullmatch(r"extrapolated", norm(status)):
        # Allowed, and the whole point of having the word: an honest extrapolation is useful, an
        # unlabelled one is a lie by omission.
        return
    if re.fullmatch(r"unqualified", norm(status)):
        return

    m = re.match(r"qualified:\s*`?([^`\s]+)`?", status, re.I)
    if not m:
        fail(f"row {lineno} ({profile}): status {status!r} is not `qualified: <file>`, "
             f"`EXTRAPOLATED` or `UNQUALIFIED`. A row with no stated basis is the one a reader "
             f"assumes was measured.")
        return

    rel = m.group(1)
    path = os.path.join(ROOT, rel)
    if not os.path.isfile(path):
        fail(f"row {lineno} ({profile}): cites {rel}, which does not exist")
        return
    try:
        r = json.load(open(path))
    except Exception as e:
        fail(f"row {lineno} ({profile}): {rel} could not be read: {e}")
        return

    if r.get("schema") != SCHEMA:
        fail(f"row {lineno} ({profile}): {rel} has schema {r.get('schema')!r}, expected {SCHEMA!r}")
        return

    for key in ("measurements", "thresholds", "scenario", "provenance"):
        if key not in r:
            fail(f"row {lineno} ({profile}): {rel} is missing {key!r}")
            return

    if sha256_of(r["thresholds"]) != r.get("thresholds_sha256"):
        fail(f"row {lineno} ({profile}): {rel} carries thresholds that do not match its own "
             f"recorded hash — the file has been edited since the run")
        return

    if r["thresholds_sha256"] != thresholds_hash:
        fail(
            f"row {lineno} ({profile}): {rel} was judged against thresholds "
            f"{r['thresholds'].get('version', '?')} ({r['thresholds_sha256'][:12]}), and the tree "
            f"now holds {thresholds_hash[:12]}. The bar moved after the run, so this claim is not "
            f"supported by it — re-run the profile."
        )
        return

    verdict, checks = evaluate(r["measurements"], r["thresholds"])
    if verdict != r.get("verdict"):
        fail(f"row {lineno} ({profile}): {rel} claims verdict {r.get('verdict')!r} but its "
             f"measurements evaluate to {verdict!r} — the file has been edited")
        return
    if verdict != "PASS":
        bad = ", ".join(f"{c['metric']}={c['status']}" for c in checks if c["status"] != "PASS")
        fail(f"row {lineno} ({profile}): cites a run that did NOT pass ({bad}). A failed threshold "
             f"blocks the capacity claim.")
        return

    # The row and the run must describe the same thing.
    sc = r["scenario"]
    prov = r["provenance"]
    if str(sc.get("cameras")) != re.sub(r"[^\d]", "", cameras):
        fail(f"row {lineno} ({profile}): the row says {cameras} cameras, the run used "
             f"{sc.get('cameras')}")
    if norm(codec).replace("h.", "h") not in norm(str(sc.get("codec", ""))).replace("h.", "h"):
        fail(f"row {lineno} ({profile}): the row says codec {codec}, the run used "
             f"{sc.get('codec')!r}")
    want_kbps = re.sub(r"[^\d.]", "", bitrate)
    if want_kbps:
        got = float(sc.get("bitrate_kbps", 0)) / 1000.0
        if abs(got - float(want_kbps)) > 0.001:
            fail(f"row {lineno} ({profile}): the row says {bitrate}, the run used {got} Mbps")
    row_ai = norm(ai)
    run_ai = norm(str(sc.get("ai_profile", "off")))
    if row_ai != run_ai:
        fail(f"row {lineno} ({profile}): the row says AI {ai!r}, the run used {run_ai!r}")
    if norm(hardware) != norm(str(prov.get("hardware_class", ""))):
        fail(f"row {lineno} ({profile}): the row says hardware {hardware!r}, the run recorded "
             f"{prov.get('hardware_class')!r}")


def main():
    doc = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "docs", "sizing.md")
    text = open(doc).read()

    rows = parse_table(text)
    if rows is None:
        print(f"{doc} has no {MARKER} — the qualification table is how a capacity claim is "
              f"connected to evidence, and its absence is a failure, not a pass.")
        return 1
    if not rows:
        print(f"{doc} has a qualification marker but no rows. Refusing to report success on an "
              f"empty check.")
        return 1

    thresholds = json.load(open(os.path.join(ROOT, "scripts", "bench", "thresholds.json")))
    thresholds_hash = sha256_of(thresholds)

    for i, cells in enumerate(rows, 1):
        check_row(cells, thresholds_hash, i)

    qualified = sum(1 for c in rows if norm(c[6] if len(c) > 6 else "").startswith("qualified"))
    print(
        f"checked {len(rows)} capacity row(s): {qualified} qualified by a run, "
        f"{len(rows) - qualified} labelled extrapolated or unqualified"
    )
    print(f"thresholds {thresholds.get('version')} ({thresholds_hash[:12]})")
    if failures:
        print(f"\n{len(failures)} problem(s):")
        for f in failures:
            print(f"  {f}")
        return 1
    print("RESULT: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
