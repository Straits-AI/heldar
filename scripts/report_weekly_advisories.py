#!/usr/bin/env python3
"""Turn a failed weekly security scan into an issue somebody actually sees (issue #114).

The cron in `.github/workflows/security.yml` re-scans unchanged code every Monday, which is the only
mechanism that catches an advisory published *after* a release. But a scheduled run reports to
nobody: there is no PR to turn red and no author to notify, so a red cron sat in the Actions tab
until someone happened to look. The one class of finding this repo has no other way of learning
about was also the one nothing surfaced.

So: when the weekly run fails, open an issue — or update the one already open, rather than filing a
duplicate every Monday until there are twelve. When a later run is clean, say so on that issue and
close it, because a tracker that only ever grows gets muted.

Uses `gh` (already authenticated in Actions) rather than an HTTP client, so this is runnable by hand
against a real repo for debugging.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys

LABEL = "security-advisory"
TITLE = "Weekly security scan is failing on unchanged code"

# Marks the issue as this script's to manage. Searching by title alone would adopt — and later close
# — an unrelated issue somebody wrote by hand.
MARKER = "<!-- heldar:weekly-advisory-report -->"


def gh(*args: str, check: bool = True) -> str:
    proc = subprocess.run(["gh", *args], capture_output=True, text=True)
    if check and proc.returncode != 0:
        raise SystemExit(f"gh {' '.join(args)} failed: {proc.stderr.strip()}")
    return proc.stdout.strip()


def find_open_issue(repo: str) -> int | None:
    raw = gh("issue", "list", "--repo", repo, "--state", "open", "--label", LABEL,
             "--json", "number,body", "--limit", "50")
    for issue in json.loads(raw or "[]"):
        if MARKER in (issue.get("body") or ""):
            return issue["number"]
    return None


def body(repo: str, failed: list[str], run_url: str, sha: str) -> str:
    jobs = "\n".join(f"- `{j}`" for j in failed)
    return f"""{MARKER}

The scheduled security scan failed on code that has not changed. Nothing was merged to cause this,
so the most likely explanation is a **newly published advisory affecting an already-released
commit** — the one class of finding no PR can catch, because there is no PR.

**Failing jobs**
{jobs}

**Run:** {run_url}
**Commit scanned:** `{sha}`

### What to do

1. Open the run above and read the finding.
2. If it is fixable, upgrade and merge as normal — this issue closes itself on the next clean run.
3. If it is not yet fixable, add an entry to `security/dependency-exceptions.json` with an owner and
   an expiry. That is a decision with a date on it, not a mute button; see `docs/SUPPLY-CHAIN.md`.

Note that publishing is gated on a green security run for the commit being tagged, so **while this
is red, releasing that commit will be refused.** That is the intended behaviour, not a separate
problem to work around.

*Filed automatically by `scripts/report_weekly_advisories.py`. It updates this issue rather than
opening a new one each week, and closes it when a later run is clean.*"""


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--repo", required=True)
    ap.add_argument("--run-url", default="")
    ap.add_argument("--sha", default="")
    ap.add_argument("--failed", default="",
                    help="comma-separated job names that failed; empty means the run was clean")
    ap.add_argument("--dry-run", action="store_true", help="print the action instead of taking it")
    args = ap.parse_args()

    failed = [j.strip() for j in args.failed.split(",") if j.strip()]
    existing = find_open_issue(args.repo)

    if not failed:
        if existing is None:
            print("clean run, no open advisory issue — nothing to do")
            return 0
        print(f"clean run — closing #{existing}")
        if args.dry_run:
            return 0
        gh("issue", "comment", str(existing), "--repo", args.repo, "--body",
           f"The weekly scan is clean again as of {args.run_url or 'the latest run'}. "
           f"Closing; it will reopen automatically if the next run fails.")
        gh("issue", "close", str(existing), "--repo", args.repo)
        return 0

    text = body(args.repo, failed, args.run_url, args.sha)
    if existing is None:
        print(f"failing run — opening a new issue ({len(failed)} job(s))")
        if args.dry_run:
            print(text)
            return 0
        # The label may not exist yet on a fresh repo; create it rather than failing the report.
        gh("label", "create", LABEL, "--repo", args.repo, "--force",
           "--description", "Weekly scan found an advisory affecting unchanged code",
           "--color", "B60205", check=False)
        print(gh("issue", "create", "--repo", args.repo, "--title", TITLE,
                 "--label", LABEL, "--body", text))
        return 0

    print(f"failing run — updating #{existing} instead of filing a duplicate")
    if args.dry_run:
        return 0
    gh("issue", "edit", str(existing), "--repo", args.repo, "--body", text)
    gh("issue", "comment", str(existing), "--repo", args.repo, "--body",
       f"Still failing as of {args.run_url or 'the latest run'} — jobs: "
       + ", ".join(f"`{j}`" for j in failed))
    return 0


if __name__ == "__main__":
    sys.exit(main())
