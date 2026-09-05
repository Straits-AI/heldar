#!/usr/bin/env python3
"""Every publishing job must depend on a green security run (issue #114).

Publishing is the irreversible step. A crates.io version can never be replaced, and an image tag —
`latest` especially — is pulled by everyone who trusts it. Until this gate existed, nothing connected
publishing to the security scan: a tag could sit on a commit whose scan had failed, or had never
finished, and the release went out anyway.

The gate is `.github/actions/require-security-run`, and this script asserts it cannot be quietly
detached. The failure it prevents is a one-line deletion — drop `needs: security-gate` and every
publishing job runs unguarded, with nothing in the diff that looks alarming and no test that fails.

Rather than guess which jobs publish (`cargo publish`, a push-enabled build-push-action, a release
upload — a list that rots), it requires EVERY job in a publishing workflow to reach the gate through
the `needs` graph. These workflows exist to publish; a job in one that needs no security clearance is
the thing worth flagging, not an exception to carve out.
"""

from __future__ import annotations

import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
ACTION = ROOT / ".github/actions/require-security-run"
ACTION_REF = "./.github/actions/require-security-run"

# Workflows whose purpose is to publish something the world can pull.
PUBLISHING_WORKFLOWS = ("release.yml", "docker-open.yml")


def needs_of(job: dict) -> list[str]:
    needs = job.get("needs") or []
    return [needs] if isinstance(needs, str) else list(needs)


def uses_gate(job: dict) -> bool:
    return any(str(s.get("uses", "")) == ACTION_REF for s in job.get("steps", []) or [])


def check() -> list[str]:
    problems: list[str] = []

    if not (ACTION / "action.yml").exists():
        return [f"{ACTION_REF}/action.yml is missing — nothing gates publishing at all"]

    for name in PUBLISHING_WORKFLOWS:
        path = ROOT / ".github/workflows" / name
        if not path.exists():
            problems.append(f"{name} is missing — has a publishing workflow been renamed?")
            continue

        jobs = yaml.safe_load(path.read_text())["jobs"]
        gates = {jid for jid, job in jobs.items() if uses_gate(job)}

        if not gates:
            problems.append(
                f"{name}: no job uses {ACTION_REF}, so this workflow can publish from a commit "
                f"whose security scan failed or never ran"
            )
            continue

        for jid in gates:
            perms = jobs[jid].get("permissions") or {}
            if isinstance(perms, dict) and perms.get("actions") != "read":
                problems.append(
                    f"{name}: gate job {jid!r} lacks `actions: read`; it cannot read another "
                    f"workflow's conclusions and the lookup fails closed on every run"
                )
            enforce = str(
                next(
                    (s for s in jobs[jid]["steps"] if str(s.get("uses", "")) == ACTION_REF), {}
                ).get("with", {}).get("enforce", "")
            ).strip()
            if enforce.lower() == "false":
                problems.append(
                    f"{name}: gate job {jid!r} hardcodes enforce=false — it would report and never "
                    f"block. Use an expression that enforces for real publishes."
                )

        # Every job must reach a gate through `needs`, transitively.
        def reaches_gate(jid: str, seen: frozenset[str]) -> bool:
            if jid in gates:
                return True
            if jid in seen:  # a needs cycle is invalid workflow syntax; do not hang on one
                return False
            return any(reaches_gate(n, seen | {jid}) for n in needs_of(jobs.get(jid, {})))

        for jid in jobs:
            if jid in gates:
                continue
            if not reaches_gate(jid, frozenset()):
                problems.append(
                    f"{name}: job {jid!r} does not depend on the security gate (directly or via "
                    f"needs), so it runs even when the scan for this commit failed"
                )

    return problems


def main() -> int:
    problems = check()
    prefix = "ERROR: " if sys.stdout.isatty() else "::error::"
    for p in problems:
        print(f"{prefix}{p}")
    if problems:
        print(f"\n{len(problems)} publishing job(s) not gated on a green security run", file=sys.stderr)
        return 1
    print(f"ok — every job in {', '.join(PUBLISHING_WORKFLOWS)} is gated on a green security run")
    return 0


if __name__ == "__main__":
    sys.exit(main())
