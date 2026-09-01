#!/usr/bin/env python3
"""Prose that names an identifier must name a real one. Fails CLOSED.

Two pairings, one rule:

  docs/OBSERVABILITY.md      <-> crates/heldar-kernel/src/services/metrics.rs
  docs/benchmarks/README.md  <-> scripts/bench/harness.py + scripts/bench/scenarios.json


An operator writes an alerting rule by copying a name out of that table. A name that has drifted
from the exposition produces a rule that matches nothing and therefore NEVER FIRES — the worst
failure an alert can have, because it looks like health.

This has now happened twice in this repository in different documents (a capability in
docs/SEARCH.md, and `heldar_camera_segments_written` here, which the code emits as
`..._total`). Both were found by a human reading carefully. This is the cheap version of that.
"""

import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

src = open(os.path.join(ROOT, "crates/heldar-kernel/src/services/metrics.rs")).read()
doc = open(os.path.join(ROOT, "docs/OBSERVABILITY.md")).read()

# Emitted names: both the `metric(&mut out, "name", …)` helper and the hand-written `writeln!` series.
emitted = set(re.findall(r'"(heldar_[a-z_]+)"', src)) | set(
    re.findall(r'(heldar_[a-z_]+)(?:\{|\s)', src)
)
emitted = {m for m in emitted if not m.endswith("_")}
if not emitted:
    print("parsed no metrics out of services/metrics.rs — the parser is looking at the wrong shape")
    sys.exit(1)

# Documented names: the leading `| \`heldar_x\` |` cell of each table row.
documented = set(re.findall(r"^\|\s*`(heldar_[a-z_]+)`\s*\|", doc, re.M))
if not documented:
    print("parsed no metrics out of docs/OBSERVABILITY.md — the table shape changed")
    sys.exit(1)

# ...and EVERY OTHER MENTION in the document, wherever it appears. Checking only the table missed
# the suggested alerting rules, which is where a name actually gets copied into production: the
# `HeldarNoSegmentProgress` rule referenced `heldar_camera_segments_written` for as long as the
# table did, and fixing the table alone left the rule silently matching nothing. A metric name in an
# alert expression is the highest-consequence place for this drift, not the lowest.
mentioned = set(re.findall(r"\b(heldar_[a-z_]+)\b", doc))

problems = []
for m in sorted(mentioned - emitted):
    where = "documents" if m in documented else "references"
    problems.append(
        f"docs/OBSERVABILITY.md {where} {m!r}, which /metrics does not emit. An alerting rule "
        f"copied from it matches nothing and never fires."
    )
for m in sorted(emitted - documented):
    problems.append(
        f"/metrics emits {m!r}, which docs/OBSERVABILITY.md does not document. The table says these "
        f"are the ONLY metrics exported, so an undocumented one makes that sentence false."
    )

# ------------------------------------------------------------------------------------------------
# docs/benchmarks/README.md against the harness. An adversarial review found SEVEN drifted claims in
# that one file — a scenario duration, a fault-coverage claim, two metric descriptions that no
# longer matched the code, and a metric the harness produces that the table omitted. Prose about a
# measurement is the part a reader trusts most and the part nothing compiles.
# ------------------------------------------------------------------------------------------------
bench = os.path.join(ROOT, "docs/benchmarks/README.md")
if os.path.isfile(bench):
    rd = open(bench).read()
    hs = open(os.path.join(ROOT, "scripts/bench/harness.py")).read()
    scen = json.load(open(os.path.join(ROOT, "scripts/bench/scenarios.json")))

    # Names the harness can put in a result: direct assignments, the derived probe families, and the
    # deliberately-unmeasured list (a list of (name, why) tuples, which a m["..."] regex misses).
    produced = set(re.findall(r'm\["([a-z_0-9]+)"\]', hs))
    produced |= set(re.findall(r'^\s*\("([a-z_0-9]+)",\s*$', hs, re.M))
    produced |= set(re.findall(r'\("([a-z_0-9]+)",\s*"[^"]', hs))
    produced |= {
        f"{n}_{suffix}"
        for n in ("liveview", "snapshot")
        for suffix in ("failure_rate", "seconds_p95")
    }
    if len(produced) < 15:
        print(f"parsed only {len(produced)} metric names out of harness.py — the parser is looking "
              f"at the wrong shape")
        sys.exit(1)

    METRIC_SUFFIXES = ("_rate", "_p95", "_p50", "_count", "_ratio", "_hour", "_max", "_mean",
                       "_files", "_reclaimed", "_seconds", "_second")
    for name in sorted(set(re.findall(r"`([a-z][a-z_0-9]{6,})`", rd))):
        if name.startswith("heldar_"):
            continue                      # a kernel metric, checked against metrics.rs above
        if name.endswith(METRIC_SUFFIXES) and name not in produced:
            problems.append(
                f"docs/benchmarks/README.md names measurement {name!r}, which the harness does not "
                f"produce — a reader looking for it in a result file will not find it"
            )

    for name in sorted(set(re.findall(r"`((?:smoke|qual|rc|soak|field)-[a-z0-9-]+)`", rd))):
        if name not in scen:
            problems.append(
                f"docs/benchmarks/README.md names scenario {name!r}, which scenarios.json does not "
                f"define"
            )
    for name in sorted(scen):
        if f"`{name}`" not in rd:
            problems.append(
                f"scenarios.json defines {name!r}, which docs/benchmarks/README.md does not list — "
                f"the scenario table claims to be the matrix"
            )

    # The fault-coverage sentence is load-bearing: a threshold over reconnect or restart recovery is
    # unmeasured, and therefore failing, in a scenario that never breaks anything.
    if "Every scenario except `field-1h` injects the same four faults" in rd:
        want = {"publisher_stop", "publisher_start", "mediamtx_restart", "core_restart"}
        for name, cfg in scen.items():
            if name == "field-1h":
                continue
            have = {f["kind"] for f in cfg.get("faults", [])}
            if not want <= have:
                problems.append(
                    f"docs/benchmarks/README.md says every scenario injects the same four faults, "
                    f"but {name!r} is missing {sorted(want - have)}"
                )

print(
    f"checked {len(documented)} documented and {len(mentioned)} mentioned name(s) "
    f"against {len(emitted)} emitted metric(s)"
)
if problems:
    print(f"\n{len(problems)} problem(s):")
    for p in problems:
        print(f"  {p}")
    sys.exit(1)
print("RESULT: PASS")
