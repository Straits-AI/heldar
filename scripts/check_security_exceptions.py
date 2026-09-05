#!/usr/bin/env python3
"""Validate security/dependency-exceptions.json — the register of accepted advisories.

Issue #114. A blocking vulnerability gate needs a pressure valve, or the first unfixable advisory
gets it switched off wholesale. The valve is this register: an entry suppresses one finding, and
carries the things that make a suppression a decision rather than a mute button — who accepted it,
why, what compensates, and the date it stops being accepted.

This script is the part that makes the expiry real. Without it, "time-bounded" is a comment.

Also emits the suppression list the audits consume (`--ignore-ids`), so the register is the only
place an advisory can be silenced. A second, undocumented ignore flag somewhere in a workflow would
defeat the whole arrangement, which is why scripts/test_security_exceptions.py asserts the audit
step takes its ids from here.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REGISTER = ROOT / "security" / "dependency-exceptions.json"

ECOSYSTEMS = {"pip", "npm", "cargo", "container"}
ID_PATTERN = re.compile(r"^(GHSA-[0-9a-z-]+|CVE-\d{4}-\d{4,}|PYSEC-\d{4}-\d+|RUSTSEC-\d{4}-\d{4}|[A-Z]+-\d{4}-\d+)$")
REQUIRED = ("id", "ecosystem", "component", "reachable", "reason", "control", "owner", "expires", "issue")

# An exception whose expiry is further out than this is not time-bounded in any useful sense; it is
# a permanent decision wearing a date. Force it back through review sooner.
MAX_HORIZON_DAYS = 180
WARN_WITHIN_DAYS = 14


def load(path: Path = REGISTER) -> dict:
    try:
        return json.loads(path.read_text())
    except FileNotFoundError:
        raise SystemExit(f"{path} is missing — the exceptions register must exist, even if empty")
    except json.JSONDecodeError as exc:
        raise SystemExit(f"{path} is not valid JSON: {exc}")


def validate(data: dict, today: dt.date) -> tuple[list[str], list[str]]:
    errors: list[str] = []
    warnings: list[str] = []

    entries = data.get("exceptions")
    if not isinstance(entries, list):
        return ([f"'exceptions' must be a list, got {type(entries).__name__}"], warnings)

    seen: set[tuple[str, str]] = set()
    for i, e in enumerate(entries):
        where = f"exceptions[{i}]"
        if not isinstance(e, dict):
            errors.append(f"{where} must be an object")
            continue

        for field in REQUIRED:
            if field not in e:
                errors.append(f"{where} is missing required field {field!r}")
        if any(field not in e for field in REQUIRED):
            continue

        where = f"exceptions[{i}] ({e['id']})"

        if not ID_PATTERN.match(str(e["id"])):
            errors.append(f"{where}: id is not a recognised advisory identifier")
        if e["ecosystem"] not in ECOSYSTEMS:
            errors.append(f"{where}: ecosystem must be one of {sorted(ECOSYSTEMS)}")
        if not isinstance(e["reachable"], bool):
            errors.append(f"{where}: 'reachable' must be true or false, not a string — an unknown "
                          f"reachability is 'true' with the uncertainty stated in 'reason'")
        for field in ("reason", "control", "component"):
            if not str(e.get(field, "")).strip():
                errors.append(f"{where}: {field!r} must not be empty")
        if not str(e["owner"]).strip():
            errors.append(f"{where}: an exception with no owner cannot be retired by anyone")
        if not str(e["issue"]).startswith("https://github.com/"):
            errors.append(f"{where}: 'issue' must link to the follow-up issue")

        key = (str(e["id"]), str(e["component"]))
        if key in seen:
            errors.append(f"{where}: duplicate exception for the same id and component")
        seen.add(key)

        try:
            expires = dt.date.fromisoformat(str(e["expires"]))
        except ValueError:
            errors.append(f"{where}: 'expires' must be an ISO date (YYYY-MM-DD)")
            continue

        if expires <= today:
            errors.append(
                f"{where}: EXPIRED on {expires} — either fix the advisory, or re-accept it in a "
                f"commit that moves the date and says why. Owner: {e['owner']}, follow-up: {e['issue']}"
            )
        elif (expires - today).days > MAX_HORIZON_DAYS:
            errors.append(
                f"{where}: expires {expires}, {(expires - today).days} days out — longer than the "
                f"{MAX_HORIZON_DAYS}-day maximum. A date that far ahead is not a time-bound."
            )
        elif (expires - today).days <= WARN_WITHIN_DAYS:
            warnings.append(f"{where}: expires in {(expires - today).days} day(s), on {expires}")

    return errors, warnings


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--ignore-ids", action="store_true",
                    help="print the advisory ids to suppress, one per line (validates first)")
    ap.add_argument("--ecosystem", help="with --ignore-ids, restrict to one ecosystem")
    ap.add_argument("--today", help="override today's date (YYYY-MM-DD), for tests")
    args = ap.parse_args()

    today = dt.date.fromisoformat(args.today) if args.today else dt.date.today()
    data = load()
    errors, warnings = validate(data, today)

    if errors:
        for e in errors:
            print(f"::error::{e}" if not sys.stdout.isatty() else f"ERROR: {e}", file=sys.stderr)
        print(f"\n{len(errors)} problem(s) in {REGISTER.relative_to(ROOT)}", file=sys.stderr)
        return 1

    if args.ignore_ids:
        for e in data["exceptions"]:
            if args.ecosystem is None or e["ecosystem"] == args.ecosystem:
                print(e["id"])
        return 0

    for w in warnings:
        print(f"::warning::{w}" if not sys.stdout.isatty() else f"WARNING: {w}")
    n = len(data["exceptions"])
    print(f"ok — {n} accepted exception(s), none expired" if n else "ok — no accepted exceptions")
    return 0


if __name__ == "__main__":
    sys.exit(main())
