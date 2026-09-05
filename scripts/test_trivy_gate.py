"""Negative controls for scripts/check_trivy_gate.py.

The guard exists because the Trivy gate silently did not block for a long time. A guard against that
which cannot itself fail would be the same mistake one level up, so each control breaks the workflow
in exactly one way and asserts the guard names it.
"""

from __future__ import annotations

import copy
import shutil
import sys
import tempfile
from pathlib import Path

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_trivy_gate as mod  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent


def run_with(mutate=None, drop_fixture=None, blank_fixture=None) -> list[str]:
    """Apply one mutation to a throwaway copy of the workflow/fixtures and return the problems."""
    wf = yaml.safe_load(mod.WORKFLOW.read_text())
    if mutate:
        mutate(wf["jobs"]["trivy-fs"]["steps"])

    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        wf_path = tmp / "security.yml"
        wf_path.write_text(yaml.dump(wf))
        fixtures = tmp / "trivy-gate"
        shutil.copytree(mod.FIXTURES, fixtures)
        if drop_fixture:
            (fixtures / drop_fixture / "requirements.txt").unlink()
        if blank_fixture:
            (fixtures / blank_fixture / "requirements.txt").write_text("# nothing but a comment\n")

        real_wf, real_fx = mod.WORKFLOW, mod.FIXTURES
        mod.WORKFLOW, mod.FIXTURES = wf_path, fixtures
        try:
            return mod.check()
        finally:
            mod.WORKFLOW, mod.FIXTURES = real_wf, real_fx


def step_by_id(steps, sid):
    return next(s for s in steps if s.get("id") == sid)


def step_by_name(steps, name):
    return next(s for s in steps if s.get("name", "").startswith(name))


def _expect(problems, needle, label):
    assert any(needle in p for p in problems), f"{label}: expected {needle!r}, got {problems}"


def the_real_workflow_passes():
    """Baseline. Without this the other controls prove nothing."""
    assert run_with() == [], run_with()


def dropping_exit_code_is_caught():
    """The exact regression: the scan reports findings and returns 0."""
    def m(steps):
        step_by_name(steps, mod.REAL_STEP)["with"].pop("exit-code")
    _expect(run_with(m), "not '1'", "exit-code removed")

    def m2(steps):
        step_by_name(steps, mod.REAL_STEP)["with"]["exit-code"] = "0"
    _expect(run_with(m2), "not '1'", "exit-code zeroed")


def a_sarif_upload_that_skips_on_failure_is_caught():
    def m(steps):
        step_by_name(steps, "upload trivy SARIF")["if"] = "success()"
    _expect(run_with(m), "not 'always()'", "upload gated on success")


def unskipping_the_fixtures_is_caught():
    """Without skip-dirs the deliberately-vulnerable fixtures fail every unrelated PR."""
    def m(steps):
        step_by_name(steps, mod.REAL_STEP)["with"]["skip-dirs"] = "website"
    _expect(run_with(m), "does not exclude scripts/fixtures", "fixtures unskipped")


def a_selftest_on_a_different_action_version_is_caught():
    def m(steps):
        step_by_id(steps, mod.SEEDED_STEP_ID)["uses"] = "aquasecurity/trivy-action@v0.20.0"
    _expect(run_with(m), "would prove nothing about the action actually shipped", "version drift")


def a_selftest_without_continue_on_error_is_caught():
    def m(steps):
        step_by_id(steps, mod.SEEDED_STEP_ID).pop("continue-on-error")
    _expect(run_with(m), "lacks continue-on-error", "outcome uninspectable")


def selftest_input_drift_is_caught():
    """The subtle one: the self-test keeps passing while exercising a gate nobody ships."""
    for key, value in (
        ("severity", "CRITICAL"),
        ("ignore-unfixed", False),
        ("exit-code", "0"),
        ("scan-type", "config"),
    ):
        def m(steps, k=key, v=value):
            step_by_id(steps, mod.SEEDED_STEP_ID)["with"][k] = v
        _expect(run_with(m), "exercising a different gate", f"{key} drift")


def a_missing_selftest_step_is_caught():
    for sid in (mod.SEEDED_STEP_ID, mod.CLEAN_STEP_ID):
        def m(steps, s=sid):
            steps.remove(step_by_id(steps, s))
        _expect(run_with(m), "the gate is untested", f"{sid} removed")


def an_assertion_that_asserts_nothing_is_caught():
    def m(steps):
        s = next(x for x in steps if "run" in x and "SEEDED" in yaml.dump(x.get("env", {})))
        s["run"] = "echo looks fine to me"
    problems = run_with(m)
    _expect(problems, "does not require the seeded fixture to FAIL", "no seeded assertion")
    _expect(problems, "does not require the clean fixture to PASS", "no clean assertion")


def losing_the_assertion_step_entirely_is_caught():
    def m(steps):
        steps.remove(next(x for x in steps if "run" in x and "SEEDED" in yaml.dump(x.get("env", {}))))
    _expect(run_with(m), "its result be ignored", "assertion step deleted")


def a_missing_or_empty_fixture_is_caught():
    for name in ("vulnerable", "clean"):
        _expect(run_with(drop_fixture=name), "is missing", f"{name} deleted")
        _expect(run_with(blank_fixture=name), "no pinned packages", f"{name} blanked")



# --------------------------------------------------------------------- image scans

def run_images_with(mutate=None) -> list[str]:
    """Copy every workflow to a sandbox, mutate one, and run the image-scan checks."""
    import shutil
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        wfdir = tmp / ".github/workflows"
        wfdir.mkdir(parents=True)
        for src in (ROOT / ".github/workflows").glob("*.yml"):
            wf = yaml.safe_load(src.read_text())
            if mutate:
                mutate(src.name, wf)
            (wfdir / src.name).write_text(yaml.dump(wf))
        real = mod.ROOT
        mod.ROOT = tmp
        try:
            return mod.check_image_scans()
        finally:
            mod.ROOT = real


def image_scans_pass_on_the_real_workflows():
    assert run_images_with() == [], run_images_with()


def a_non_blocking_trivy_step_anywhere_is_caught():
    def m(name, wf):
        if name != "docker-open.yml":
            return
        for job in wf["jobs"].values():
            for st in job.get("steps", []):
                if str(st.get("uses", "")).startswith("aquasecurity/trivy-action"):
                    st["with"]["exit-code"] = "0"
    _expect(run_images_with(m), "reports without blocking", "exit-code zeroed on the image scan")


def an_image_scan_after_the_push_is_caught():
    """A scan that runs after publication reports on what was already handed out."""
    def m(name, wf):
        if name != "docker-open.yml":
            return
        for job in wf["jobs"].values():
            steps = job.get("steps", [])
            scan = next(
                (s for s in steps
                 if str(s.get("uses", "")).startswith("aquasecurity/trivy-action")
                 and s.get("with", {}).get("scan-type") == "image"),
                None,
            )
            if scan:
                steps.remove(scan)
                steps.append(scan)  # move it to the very end, after the push
    _expect(run_images_with(m), "reports rather than gates", "scan moved after push")


def publishing_images_without_scanning_them_is_caught():
    def m(name, wf):
        if name != "docker-open.yml":
            return
        for job in wf["jobs"].values():
            job["steps"] = [
                s for s in job.get("steps", [])
                if not (str(s.get("uses", "")).startswith("aquasecurity/trivy-action")
                        and s.get("with", {}).get("scan-type") == "image")
            ]
    _expect(run_images_with(m), "publishes images but never scans one", "scan deleted")


CHECKS = [
    the_real_workflow_passes,
    dropping_exit_code_is_caught,
    a_sarif_upload_that_skips_on_failure_is_caught,
    unskipping_the_fixtures_is_caught,
    a_selftest_on_a_different_action_version_is_caught,
    a_selftest_without_continue_on_error_is_caught,
    selftest_input_drift_is_caught,
    a_missing_selftest_step_is_caught,
    an_assertion_that_asserts_nothing_is_caught,
    losing_the_assertion_step_entirely_is_caught,
    a_missing_or_empty_fixture_is_caught,
    image_scans_pass_on_the_real_workflows,
    a_non_blocking_trivy_step_anywhere_is_caught,
    an_image_scan_after_the_push_is_caught,
    publishing_images_without_scanning_them_is_caught,
]

if __name__ == "__main__":
    for fn in CHECKS:
        fn()
        print(f"  ok    {fn.__name__.replace('_', ' ')}")
    print(f"\nall {len(CHECKS)} Trivy-gate controls passed")
