"""Controls for scripts/check_security_exceptions.py.

The register is EMPTY, and should stay that way. That makes these controls the only thing that ever
exercises the validator: without them the first real exception would be the first time the code
ran, on the day someone is trying to unblock a release. Each control builds a realistic entry and
breaks exactly one thing.
"""

from __future__ import annotations

import datetime as dt
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_security_exceptions as mod  # noqa: E402

TODAY = dt.date(2026, 9, 5)

VALID = {
    "id": "GHSA-abcd-1234-wxyz",
    "ecosystem": "pip",
    "component": "somepkg==1.2.3",
    "reachable": False,
    "reason": "The vulnerable parser is only reached through the CLI entrypoint, which we never call.",
    "control": "Worker runs with no network egress except the kernel API.",
    "owner": "wms2537",
    "expires": "2026-11-01",
    "issue": "https://github.com/Straits-AI/heldar/issues/114",
}


def check(entries, today=TODAY):
    return mod.validate({"exceptions": entries}, today)


def _expect_error(entries, needle, label):
    errors, _ = check(entries)
    assert any(needle in e for e in errors), f"{label}: expected {needle!r}, got {errors}"


def a_well_formed_exception_passes():
    errors, warnings = check([VALID])
    assert errors == [], errors
    assert warnings == [], warnings


def an_expired_exception_fails():
    """The whole point. An exception past its date must break the build, not warn."""
    _expect_error([{**VALID, "expires": "2026-09-04"}], "EXPIRED", "yesterday")
    _expect_error([{**VALID, "expires": "2026-09-05"}], "EXPIRED", "today is not still accepted")
    _expect_error([{**VALID, "expires": "2020-01-01"}], "EXPIRED", "long past")


def an_expiry_beyond_the_horizon_fails():
    _expect_error([{**VALID, "expires": "2027-09-01"}], "maximum", "a year out")


def an_imminent_expiry_warns_but_does_not_fail():
    errors, warnings = check([{**VALID, "expires": "2026-09-12"}])
    assert errors == [], errors
    assert len(warnings) == 1 and "7 day(s)" in warnings[0], warnings


def every_required_field_is_required():
    for field in mod.REQUIRED:
        entry = {k: v for k, v in VALID.items() if k != field}
        _expect_error([entry], f"missing required field {field!r}", field)


def a_field_present_but_empty_is_not_enough():
    """Blank prose satisfies 'present' and satisfies nobody reading the register later."""
    for field in ("reason", "control", "component"):
        _expect_error([{**VALID, field: "   "}], f"{field!r} must not be empty", field)
    _expect_error([{**VALID, "owner": ""}], "no owner", "owner")


def reachability_must_be_a_boolean():
    """'unknown' as a string reads as a decision; it is the absence of one."""
    _expect_error([{**VALID, "reachable": "unknown"}], "must be true or false", "string")
    _expect_error([{**VALID, "reachable": "false"}], "must be true or false", "stringy false")


def a_bogus_advisory_id_fails():
    for bad in ("not-an-advisory", "GHSA", "CVE-20-1", ""):
        _expect_error([{**VALID, "id": bad}], "not a recognised advisory", bad or "empty")


def real_advisory_id_shapes_are_accepted():
    for good in ("GHSA-abcd-1234-wxyz", "CVE-2026-12345", "PYSEC-2026-42", "RUSTSEC-2026-0001"):
        errors, _ = check([{**VALID, "id": good}])
        assert errors == [], (good, errors)


def an_unknown_ecosystem_fails():
    _expect_error([{**VALID, "ecosystem": "maven"}], "ecosystem must be one of", "maven")


def the_issue_must_link_somewhere_real():
    _expect_error([{**VALID, "issue": "see slack"}], "must link to the follow-up issue", "prose")


def duplicates_fail():
    _expect_error([VALID, VALID], "duplicate exception", "same id and component")
    # ...but the same advisory against a DIFFERENT component is legitimate.
    errors, _ = check([VALID, {**VALID, "component": "otherpkg==9.9"}])
    assert errors == [], errors


def the_register_on_disk_is_valid_today():
    """The real file, at the real date — this is what CI runs."""
    errors, _ = mod.validate(mod.load(), dt.date.today())
    assert errors == [], errors


def the_audits_take_their_suppressions_from_the_register():
    """A hardcoded ignore in a workflow would silence an advisory with no owner and no expiry.

    That is the exact failure the register exists to prevent, so the register must be the ONLY
    place an advisory id appears in the security workflow.
    """
    wf_path = Path(__file__).resolve().parent.parent / ".github/workflows/security.yml"
    wf = wf_path.read_text()

    assert "check_security_exceptions.py --ignore-ids" in wf, (
        "security.yml does not take its suppression list from the register"
    )

    literal = re.compile(r"\b(GHSA-[0-9a-z-]{5,}|CVE-\d{4}-\d{4,}|PYSEC-\d{4}-\d+)\b")
    for lineno, line in enumerate(wf.splitlines(), 1):
        if (hit := literal.search(line)) and not line.lstrip().startswith("#"):
            raise AssertionError(
                f"security.yml:{lineno} suppresses {hit.group()} outside the register: {line.strip()!r}"
            )


def unfixed_findings_are_ignored_only_where_that_is_documented():
    """`ignore-unfixed: true` is a real policy choice; #114 requires it not be a silent one."""
    root = Path(__file__).resolve().parent.parent
    wf = (root / ".github/workflows/security.yml").read_text()
    if "ignore-unfixed: true" not in wf:
        return  # the policy changed; nothing to document
    doc = (root / "docs/SUPPLY-CHAIN.md").read_text()
    assert "ignore-unfixed" in doc, (
        "security.yml ignores unfixed findings but docs/SUPPLY-CHAIN.md does not say so"
    )


CHECKS = [
    a_well_formed_exception_passes,
    an_expired_exception_fails,
    an_expiry_beyond_the_horizon_fails,
    an_imminent_expiry_warns_but_does_not_fail,
    every_required_field_is_required,
    a_field_present_but_empty_is_not_enough,
    reachability_must_be_a_boolean,
    a_bogus_advisory_id_fails,
    real_advisory_id_shapes_are_accepted,
    an_unknown_ecosystem_fails,
    the_issue_must_link_somewhere_real,
    duplicates_fail,
    the_register_on_disk_is_valid_today,
    the_audits_take_their_suppressions_from_the_register,
    unfixed_findings_are_ignored_only_where_that_is_documented,
]

if __name__ == "__main__":
    for fn in CHECKS:
        fn()
        print(f"  ok    {fn.__name__.replace('_', ' ')}")
    print(f"\nall {len(CHECKS)} exception-register controls passed")
