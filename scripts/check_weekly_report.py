#!/usr/bin/env python3
"""The weekly advisory report must cover every scan (issue #114).

The Monday cron is the only thing that catches an advisory published AFTER a release, against code
nobody has touched. `advisory-report` turns a red cron into an issue — but only for the jobs it
lists in `needs`. Add a fifth scanner tomorrow and forget to list it, and its failures become
invisible again: no PR to redden, no author to notify, and now no issue either.

So the rule is coverage, not configuration: every job in security.yml must be depended on. There is
nothing to keep in sync by hand, because the expected list IS the set of jobs in the file.
"""

from __future__ import annotations

import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github/workflows/security.yml"
REPORTER = "advisory-report"


def check() -> list[str]:
    problems: list[str] = []
    jobs = yaml.safe_load(WORKFLOW.read_text())["jobs"]

    reporter = jobs.get(REPORTER)
    if reporter is None:
        return [f"security.yml has no {REPORTER!r} job — a failing weekly scan would tell nobody"]

    needs = reporter.get("needs") or []
    needs = [needs] if isinstance(needs, str) else list(needs)

    uncovered = [j for j in jobs if j != REPORTER and j not in needs]
    for j in uncovered:
        problems.append(
            f"security.yml: job {j!r} is not in {REPORTER}'s `needs`, so when it fails on the "
            f"weekly cron nothing opens an issue and the failure is invisible"
        )

    for j in needs:
        if j not in jobs:
            problems.append(f"security.yml: {REPORTER} depends on {j!r}, which is not a job here")

    cond = str(reporter.get("if", ""))
    if "always()" not in cond:
        problems.append(
            f"{REPORTER} lacks always() in its `if`, so it is skipped exactly when its dependencies "
            f"failed — which is the only time it has anything to report"
        )
    if "schedule" not in cond:
        problems.append(
            f"{REPORTER} is not restricted to the schedule event; on a PR the author already sees "
            f"the failure and an issue per red PR would be noise"
        )

    perms = reporter.get("permissions") or {}
    if perms.get("issues") != "write":
        problems.append(f"{REPORTER} lacks `issues: write` and cannot file anything")

    if not (ROOT / "scripts/report_weekly_advisories.py").exists():
        problems.append("scripts/report_weekly_advisories.py is missing")

    return problems


def main() -> int:
    problems = check()
    prefix = "ERROR: " if sys.stdout.isatty() else "::error::"
    for p in problems:
        print(f"{prefix}{p}")
    if problems:
        print(f"\n{len(problems)} problem(s) with the weekly advisory report", file=sys.stderr)
        return 1
    print("ok — every security.yml scan is covered by the weekly advisory report")
    return 0


if __name__ == "__main__":
    sys.exit(main())
