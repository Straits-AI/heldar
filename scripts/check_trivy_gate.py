#!/usr/bin/env python3
"""Keep the Trivy gate, and its self-test, honest (issue #114).

The filesystem scan is meant to BLOCK on a HIGH/CRITICAL finding. For a long time it did not:
`exit-code` was unset, so the job produced SARIF, the code-scanning dashboard filled up, and every
PR reported success. The flag is there now, and `.github/workflows/security.yml` self-tests it
against a seeded fixture — but a self-test can rot in three ways that all look like success:

  1. the real scan quietly loses `exit-code`, while the self-test keeps its own copy and passes;
  2. the self-test drifts to different inputs, so it exercises a configuration nobody ships;
  3. the fixtures stop being scanned at all — dropped from the repo, or swallowed by `skip-dirs`.

This script closes all three. It compares the self-test steps against the real scan rather than
holding its own idea of what the settings should be, so tightening the real scan does not require
editing this file — only keeping the two in agreement.

It also asserts the opposite direction: the fixtures directory MUST stay in the real scan's
`skip-dirs`. It contains knowingly vulnerable pins, so without that every unrelated PR goes red and
the first person to hit it deletes the fixtures, taking the self-test with them.

Finally it holds every OTHER Trivy invocation in the repo to the same standard: blocking, and — for
the image scans in docker-open.yml — ordered BEFORE the push. An image scan after the push reports
on something already handed to everyone who pulls it, which is a report, not a gate.
"""

from __future__ import annotations

import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github/workflows/security.yml"
FIXTURES = ROOT / "scripts/fixtures/trivy-gate"

# Inputs that decide the VERDICT. The self-test must match the real scan on every one of these, or
# it is testing something else. `scan-ref` and `output` legitimately differ; `skip-dirs` applies only
# to the repo-wide scan.
VERDICT_INPUTS = ("scan-type", "severity", "ignore-unfixed", "exit-code")

REAL_STEP = "trivy filesystem scan"


def rel(path: Path) -> str:
    """Path relative to the repo when it is inside it, else as-is (tests point elsewhere)."""
    try:
        return str(path.relative_to(ROOT))
    except ValueError:
        return str(path)
SEEDED_STEP_ID = "seeded"
CLEAN_STEP_ID = "clean"


def steps() -> list[dict]:
    wf = yaml.safe_load(WORKFLOW.read_text())
    return wf["jobs"]["trivy-fs"]["steps"]


def by_name(all_steps: list[dict], name: str) -> dict | None:
    return next((s for s in all_steps if s.get("name", "").startswith(name)), None)


def by_id(all_steps: list[dict], step_id: str) -> dict | None:
    return next((s for s in all_steps if s.get("id") == step_id), None)


def check() -> list[str]:
    problems: list[str] = []
    all_steps = steps()

    real = by_name(all_steps, REAL_STEP)
    if real is None:
        return [f"no step named {REAL_STEP!r} in the trivy-fs job — has the scan been renamed?"]
    real_with = real.get("with", {})

    # 1. the gate must actually block
    if str(real_with.get("exit-code", "")) != "1":
        problems.append(
            f"the real scan has exit-code={real_with.get('exit-code')!r}, not '1'. Without it Trivy "
            f"reports findings and returns 0, which is how a HIGH finding sat on the dashboard "
            f"while every PR showed green."
        )

    # 2. findings must still be published when the scan fails, or a red build hides its own evidence
    upload = by_name(all_steps, "upload trivy SARIF")
    if upload is None:
        problems.append("no SARIF upload step — findings would never reach the dashboard")
    elif str(upload.get("if", "")).strip() != "always()":
        problems.append(
            f"the SARIF upload has if={upload.get('if')!r}, not 'always()'. A failing scan would "
            f"then publish nothing, so the build goes red with the evidence withheld."
        )

    # 3. the fixtures must be excluded from the repo-wide scan
    skip = str(real_with.get("skip-dirs", ""))
    if "scripts/fixtures" not in [d.strip() for d in skip.split(",")]:
        problems.append(
            f"skip-dirs is {skip!r} and does not exclude scripts/fixtures. The gate self-test "
            f"fixtures are deliberately vulnerable, so the repo-wide scan would fail every PR."
        )

    # 4. the self-test must exercise the shipped configuration
    for step_id, expected_ref in ((SEEDED_STEP_ID, "vulnerable"), (CLEAN_STEP_ID, "clean")):
        step = by_id(all_steps, step_id)
        if step is None:
            problems.append(f"no self-test step with id {step_id!r} — the gate is untested")
            continue

        if step.get("uses") != real.get("uses"):
            problems.append(
                f"self-test {step_id!r} uses {step.get('uses')!r} but the real scan uses "
                f"{real.get('uses')!r} — it would prove nothing about the action actually shipped"
            )

        if step.get("continue-on-error") is not True:
            problems.append(
                f"self-test {step_id!r} lacks continue-on-error: true, so its outcome can never be "
                f"inspected — the expected failure would just fail the job"
            )

        step_with = step.get("with", {})
        for key in VERDICT_INPUTS:
            if step_with.get(key) != real_with.get(key):
                problems.append(
                    f"self-test {step_id!r} sets {key}={step_with.get(key)!r} but the real scan "
                    f"sets {real_with.get(key)!r} — the self-test is exercising a different gate"
                )

        ref = str(step_with.get("scan-ref", ""))
        if not ref.endswith(f"trivy-gate/{expected_ref}"):
            problems.append(f"self-test {step_id!r} scans {ref!r}, not the {expected_ref} fixture")

    # 5. both outcomes must actually be asserted on
    assertion = next(
        (s for s in all_steps if "run" in s and "steps.seeded.outcome" in yaml.dump(s.get("env", {}))),
        None,
    )
    if assertion is None:
        problems.append(
            "no step reads steps.seeded.outcome — the self-test would run and its result be ignored"
        )
    else:
        body = assertion.get("run", "")
        if 'SEEDED" != "failure"' not in body:
            problems.append("the assertion step does not require the seeded fixture to FAIL")
        if 'CLEAN" != "success"' not in body:
            problems.append(
                "the assertion step does not require the clean fixture to PASS — without that, a "
                "scanner failing on everything would satisfy the self-test"
            )

    # 6. the fixtures have to exist and say something
    for name in ("vulnerable", "clean"):
        req = FIXTURES / name / "requirements.txt"
        if not req.exists():
            problems.append(f"{rel(req)} is missing — the self-test scans nothing")
        elif not [
            ln for ln in req.read_text().splitlines() if ln.strip() and not ln.startswith("#")
        ]:
            problems.append(f"{rel(req)} has no pinned packages, only comments")

    return problems


def check_image_scans() -> list[str]:
    """Every Trivy step in the repo blocks, and image scans run before the push."""
    problems: list[str] = []
    workflows = sorted((ROOT / ".github/workflows").glob("*.yml"))

    for path in workflows:
        wf = yaml.safe_load(path.read_text())
        for jid, job in (wf.get("jobs") or {}).items():
            job_steps = job.get("steps") or []
            for i, step in enumerate(job_steps):
                if not str(step.get("uses", "")).startswith("aquasecurity/trivy-action"):
                    continue
                with_ = step.get("with", {})
                if str(with_.get("exit-code", "")) != "1":
                    problems.append(
                        f"{path.name}: job {jid!r} step {step.get('name', i)!r} runs Trivy with "
                        f"exit-code={with_.get('exit-code')!r} — it reports without blocking"
                    )
                if with_.get("scan-type") != "image":
                    continue
                # An image scan is only a gate if nothing has been published yet.
                pushes_after = [
                    s2.get("name", j)
                    for j, s2 in enumerate(job_steps[i + 1 :], start=i + 1)
                    if str(s2.get("uses", "")).startswith("docker/build-push-action")
                    and str(s2.get("with", {}).get("push", "")) not in ("", "false")
                ]
                if not pushes_after:
                    problems.append(
                        f"{path.name}: job {jid!r} scans an image but no pushing build step follows "
                        f"it — the scan runs after publication, so it reports rather than gates"
                    )

    # The release workflow must actually scan what it ships.
    docker = ROOT / ".github/workflows/docker-open.yml"
    if docker.exists():
        wf = yaml.safe_load(docker.read_text())
        scans = [
            s2
            for job in (wf.get("jobs") or {}).values()
            for s2 in (job.get("steps") or [])
            if str(s2.get("uses", "")).startswith("aquasecurity/trivy-action")
            and s2.get("with", {}).get("scan-type") == "image"
        ]
        if not scans:
            problems.append(
                "docker-open.yml publishes images but never scans one — a base bump could ship a "
                "HIGH to everyone who pulls `latest`"
            )

    return problems


def main() -> int:
    problems = check() + check_image_scans()
    prefix = "ERROR: " if sys.stdout.isatty() else "::error::"
    for p in problems:
        print(f"{prefix}{p}")
    if problems:
        print(f"\n{len(problems)} problem(s) with the Trivy gate", file=sys.stderr)
        return 1
    print("ok — the Trivy gate blocks, publishes its findings, and self-tests the shipped config")
    return 0


if __name__ == "__main__":
    sys.exit(main())
