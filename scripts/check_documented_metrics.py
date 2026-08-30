#!/usr/bin/env python3
"""Every metric docs/OBSERVABILITY.md names must exist, and vice versa. Fails CLOSED.

An operator writes an alerting rule by copying a name out of that table. A name that has drifted
from the exposition produces a rule that matches nothing and therefore NEVER FIRES — the worst
failure an alert can have, because it looks like health.

This has now happened twice in this repository in different documents (a capability in
docs/SEARCH.md, and `heldar_camera_segments_written` here, which the code emits as
`..._total`). Both were found by a human reading carefully. This is the cheap version of that.
"""

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

problems = []
for m in sorted(documented - emitted):
    problems.append(
        f"docs/OBSERVABILITY.md documents {m!r}, which /metrics does not emit. An alerting rule "
        f"copied from that row matches nothing and never fires."
    )
for m in sorted(emitted - documented):
    problems.append(
        f"/metrics emits {m!r}, which docs/OBSERVABILITY.md does not document. The table says these "
        f"are the ONLY metrics exported, so an undocumented one makes that sentence false."
    )

print(f"checked {len(documented)} documented against {len(emitted)} emitted metric(s)")
if problems:
    print(f"\n{len(problems)} problem(s):")
    for p in problems:
        print(f"  {p}")
    sys.exit(1)
print("RESULT: PASS")
