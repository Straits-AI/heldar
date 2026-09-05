"""Negative controls for scripts/check_release_gated.py.

The guard protects against a ONE-LINE deletion: drop `needs: security-gate` and every publishing job
runs unguarded, with nothing in the diff that looks alarming. So the guard has to fail loudly on
exactly that, and these controls are the only thing proving it does.
"""

from __future__ import annotations

import shutil
import sys
import tempfile
from pathlib import Path

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_release_gated as mod  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent


def run_with(mutate=None, drop_action=False) -> list[str]:
    """Apply one mutation to a throwaway copy of the publishing workflows."""
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        (tmp / ".github/workflows").mkdir(parents=True)
        for name in mod.PUBLISHING_WORKFLOWS:
            wf = yaml.safe_load((ROOT / ".github/workflows" / name).read_text())
            if mutate:
                mutate(name, wf["jobs"])
            (tmp / ".github/workflows" / name).write_text(yaml.dump(wf))

        action = tmp / ".github/actions/require-security-run"
        if not drop_action:
            action.mkdir(parents=True)
            shutil.copy(ROOT / ".github/actions/require-security-run/action.yml", action)

        real_root, real_action = mod.ROOT, mod.ACTION
        mod.ROOT, mod.ACTION = tmp, action
        try:
            return mod.check()
        finally:
            mod.ROOT, mod.ACTION = real_root, real_action


def _expect(problems, needle, label):
    assert any(needle in p for p in problems), f"{label}: expected {needle!r}, got {problems}"


def the_real_workflows_pass():
    """Baseline. Without this every control below is vacuous."""
    assert run_with() == [], run_with()


def detaching_the_gate_is_caught():
    """The one-line deletion this guard exists for."""
    def m(name, jobs):
        if name == "release.yml":
            jobs["publish"].pop("needs", None)
    problems = run_with(m)
    # publish loses its gate, and `binaries` loses it too — it only reached the gate through publish.
    _expect(problems, "job 'publish' does not depend on the security gate", "publish detached")
    _expect(problems, "job 'binaries' does not depend on the security gate", "transitive loss")


def deleting_the_gate_job_is_caught():
    def m(name, jobs):
        if name == "docker-open.yml":
            jobs.pop("security-gate")
            jobs["images"].pop("needs", None)
    _expect(run_with(m), "no job uses", "gate job removed")


def a_gate_without_actions_read_is_caught():
    """Without it the API lookup fails on every run, which is a gate that cannot pass."""
    def m(name, jobs):
        if name == "release.yml":
            jobs["security-gate"]["permissions"] = {"contents": "read"}
    _expect(run_with(m), "lacks `actions: read`", "permission dropped")


def a_gate_hardcoded_to_not_enforce_is_caught():
    def m(name, jobs):
        if name == "release.yml":
            step = next(s for s in jobs["security-gate"]["steps"] if s.get("uses") == mod.ACTION_REF)
            step["with"]["enforce"] = "false"
    _expect(run_with(m), "hardcodes enforce=false", "enforcement disabled")


def a_newly_added_ungated_job_is_caught():
    """A publishing workflow should not grow a job that needs no security clearance."""
    def m(name, jobs):
        if name == "release.yml":
            jobs["announce"] = {"runs-on": "ubuntu-latest", "steps": [{"run": "echo shipped"}]}
    _expect(run_with(m), "job 'announce' does not depend on the security gate", "new job")


def a_missing_action_is_caught():
    _expect(run_with(drop_action=True), "nothing gates publishing at all", "action deleted")


def a_needs_cycle_does_not_hang():
    """Invalid workflow syntax must produce a finding, not an infinite recursion."""
    def m(name, jobs):
        if name == "docker-open.yml":
            jobs["images"]["needs"] = ["security-gate", "images"]
    run_with(m)  # completing at all is the assertion


CHECKS = [
    the_real_workflows_pass,
    detaching_the_gate_is_caught,
    deleting_the_gate_job_is_caught,
    a_gate_without_actions_read_is_caught,
    a_gate_hardcoded_to_not_enforce_is_caught,
    a_newly_added_ungated_job_is_caught,
    a_missing_action_is_caught,
    a_needs_cycle_does_not_hang,
]

if __name__ == "__main__":
    for fn in CHECKS:
        fn()
        print(f"  ok    {fn.__name__.replace('_', ' ')}")
    print(f"\nall {len(CHECKS)} release-gate controls passed")
