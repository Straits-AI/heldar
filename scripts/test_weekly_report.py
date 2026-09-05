"""Controls for the weekly advisory report — the coverage guard and the reporter's own decisions.

The reporter runs once a week, only when something is already wrong. Nobody watches it, and its
mistakes are quiet ones: a duplicate issue every Monday, or an unrelated hand-written issue closed
because the search matched its title. Both are tested here rather than discovered in production.
"""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
from pathlib import Path

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_weekly_report as guard  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
REPORTER = ROOT / "scripts/report_weekly_advisories.py"


# --------------------------------------------------------------------------- coverage guard

def run_guard(mutate=None) -> list[str]:
    wf = yaml.safe_load(guard.WORKFLOW.read_text())
    if mutate:
        mutate(wf["jobs"])
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "security.yml"
        path.write_text(yaml.dump(wf))
        real = guard.WORKFLOW
        guard.WORKFLOW = path
        try:
            return guard.check()
        finally:
            guard.WORKFLOW = real


def _expect(problems, needle, label):
    assert any(needle in p for p in problems), f"{label}: expected {needle!r}, got {problems}"


def the_real_workflow_passes():
    assert run_guard() == [], run_guard()


def a_scan_left_out_of_needs_is_caught():
    """The failure mode: add a scanner, forget to list it, its failures go unreported."""
    def m(jobs):
        jobs[guard.REPORTER]["needs"].remove("gitleaks")
    _expect(run_guard(m), "'gitleaks' is not in", "dropped from needs")


def a_newly_added_scan_job_is_caught():
    def m(jobs):
        jobs["sbom-audit"] = {"runs-on": "ubuntu-latest", "steps": [{"run": "echo scan"}]}
    _expect(run_guard(m), "'sbom-audit' is not in", "new scanner unlisted")


def losing_always_is_caught():
    """Without always() the reporter is skipped exactly when it has something to say."""
    def m(jobs):
        jobs[guard.REPORTER]["if"] = "github.event_name == 'schedule'"
    _expect(run_guard(m), "lacks always()", "always dropped")


def running_on_every_event_is_caught():
    def m(jobs):
        jobs[guard.REPORTER]["if"] = "always()"
    _expect(run_guard(m), "not restricted to the schedule event", "unrestricted")


def losing_issue_write_is_caught():
    def m(jobs):
        jobs[guard.REPORTER]["permissions"] = {"contents": "read"}
    _expect(run_guard(m), "cannot file anything", "permission dropped")


def deleting_the_reporter_is_caught():
    def m(jobs):
        jobs.pop(guard.REPORTER)
    _expect(run_guard(m), "would tell nobody", "reporter removed")


# --------------------------------------------------------------------------- reporter decisions

def reporter(existing_issues_json: str, failed: str) -> str:
    """Drive the real script with a stubbed `gh`, in dry-run, and return what it decided."""
    with tempfile.TemporaryDirectory() as tmp:
        stub = Path(tmp) / "gh"
        stub.write_text(
            "#!/usr/bin/env bash\n"
            "# only `issue list` is consulted in dry-run; anything else would be a real mutation\n"
            'if [ "$1" = "issue" ] && [ "$2" = "list" ]; then printf \'%s\' "$GH_STUB_ISSUES"; exit 0; fi\n'
            'echo "STUB REFUSED: gh $*" >&2; exit 1\n'
        )
        stub.chmod(0o755)
        env = {**os.environ, "PATH": f"{tmp}:{os.environ['PATH']}", "GH_STUB_ISSUES": existing_issues_json}
        proc = subprocess.run(
            [sys.executable, str(REPORTER), "--repo", "o/r", "--failed", failed,
             "--run-url", "http://run", "--sha", "abc123", "--dry-run"],
            capture_output=True, text=True, env=env,
        )
        assert proc.returncode == 0, proc.stderr
        return proc.stdout


def a_failing_run_with_no_open_issue_opens_one():
    out = reporter("[]", "trivy-fs,pip-audit")
    assert "opening a new issue" in out, out
    assert "2 job(s)" in out, out


def a_failing_run_with_an_open_issue_updates_it():
    """Otherwise it files a fresh issue every Monday until there are twelve."""
    import report_weekly_advisories as rep
    issues = f'[{{"number": 7, "body": "{rep.MARKER}"}}]'
    out = reporter(issues, "trivy-fs")
    assert "updating #7" in out, out
    assert "opening a new issue" not in out, out


def a_clean_run_closes_the_open_issue():
    import report_weekly_advisories as rep
    issues = f'[{{"number": 7, "body": "{rep.MARKER}"}}]'
    out = reporter(issues, "")
    assert "closing #7" in out.lower(), out


def a_clean_run_with_nothing_open_does_nothing():
    out = reporter("[]", "")
    assert "nothing to do" in out, out


def an_unmarked_issue_is_never_adopted():
    """Searching by label alone would let it edit — and later CLOSE — somebody's hand-written issue."""
    issues = '[{"number": 99, "body": "I filed this myself about a CVE"}]'
    out = reporter(issues, "trivy-fs")
    assert "opening a new issue" in out, out
    assert "99" not in out, out


CHECKS = [
    the_real_workflow_passes,
    a_scan_left_out_of_needs_is_caught,
    a_newly_added_scan_job_is_caught,
    losing_always_is_caught,
    running_on_every_event_is_caught,
    losing_issue_write_is_caught,
    deleting_the_reporter_is_caught,
    a_failing_run_with_no_open_issue_opens_one,
    a_failing_run_with_an_open_issue_updates_it,
    a_clean_run_closes_the_open_issue,
    a_clean_run_with_nothing_open_does_nothing,
    an_unmarked_issue_is_never_adopted,
]

if __name__ == "__main__":
    for fn in CHECKS:
        fn()
        print(f"  ok    {fn.__name__.replace('_', ' ')}")
    print(f"\nall {len(CHECKS)} weekly-report controls passed")
