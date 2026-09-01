#!/usr/bin/env python3
"""Verify a deployment against its release manifest (#112). Fails CLOSED.

Usage: verify_release_manifest.py <manifest.json> [deploy-dir]
       HELDAR_DB=/var/lib/heldar/heldar.db verify_release_manifest.py ...

Checks, in the order that matters:
  1. the deployment files on disk are byte-for-byte the ones this release shipped
  2. the database's schema is not AHEAD of what this release supports, for EVERY component

(2) is the one that matters on an upgrade. Migrations only go forward, so a database written by a
NEWER release cannot be safely served by an older binary — and the failure mode without this check
is not a clean refusal, it is an old binary reading a schema it does not understand.

WHY THIS IS ONE PROGRAM AND NOT A SHELL SCRIPT CALLING python3.

It was the latter, and that shape produced two separate silent passes:

  * the artifact list was read through a process substitution, whose exit status bash cannot see.
    A single manifest entry missing `sha256` made the reader crash mid-stream; CPython had already
    flushed the rows before it, so the loop verified a PREFIX of the artifacts, found them all good,
    and printed RESULT: PASS. The "checked nothing" guard did not fire because it had checked one.
  * the schema probe caught `Exception` and reported every failure as "fresh install". Four kilobytes
    of random bytes verified clean.

Both are the same defect: a check reporting more than it verified. One process with one exit code
removes the seam that allowed it. Everything below either verifies something or says it did not.
"""

import hashlib
import json
import os
import sqlite3
import sys

ART = "artifacts"
MIG = "migrations"

fail = False


def bad(msg: str) -> None:
    global fail
    print(f"FAIL {msg}")
    fail = True


def ok(msg: str) -> None:
    print(f"PASS {msg}")


def die(msg: str) -> None:
    """A manifest we cannot even read is a refusal, not a skip."""
    print(f"FAIL {msg}")
    print("RESULT: FAIL")
    sys.exit(1)


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: verify_release_manifest.py <manifest.json> [deploy-dir]", file=sys.stderr)
        return 2
    manifest_path = sys.argv[1]
    deploy = sys.argv[2] if len(sys.argv) > 2 else os.path.join(
        os.path.dirname(os.path.abspath(__file__)), os.pardir, "deploy"
    )
    db_path = os.environ.get("HELDAR_DB", "")

    try:
        with open(manifest_path, "rb") as fh:
            m = json.load(fh)
    except FileNotFoundError:
        die(f"manifest {manifest_path} does not exist")
    except (json.JSONDecodeError, UnicodeDecodeError) as e:
        die(f"manifest {manifest_path} is not valid JSON: {e}")
    if not isinstance(m, dict):
        die(f"manifest {manifest_path} is not a JSON object")

    # Validate the WHOLE manifest before verifying any of it. Validating as we go is what let a
    # malformed entry halfway down the list pass off a partial check as a complete one.
    artifacts = m.get(ART)
    if not isinstance(artifacts, dict) or not artifacts:
        die("manifest lists no artifacts — refusing to report success on an empty check")
    for name, entry in artifacts.items():
        if not isinstance(entry, dict) or not isinstance(entry.get("sha256"), str) or not entry["sha256"]:
            die(f"artifact {name!r} has no usable sha256 in the manifest — the manifest is malformed, "
                f"and a partial verification must not be reported as a complete one")

    ceilings = m.get(MIG)
    if not isinstance(ceilings, dict) or not ceilings:
        die("manifest declares no migration ceilings — it cannot certify a binary/database pairing")
    for comp, ceil in ceilings.items():
        if not isinstance(ceil, int) or isinstance(ceil, bool):
            die(f"migration ceiling for {comp!r} is not an integer")

    print("== artifacts ==")
    for name, entry in sorted(artifacts.items()):
        path = os.path.join(deploy, name)
        if not os.path.isfile(path):
            bad(f"{name} is missing from {deploy}")
            continue
        h = hashlib.sha256()
        with open(path, "rb") as fh:
            for chunk in iter(lambda: fh.read(1 << 20), b""):
                h.update(chunk)
        got = h.hexdigest()
        if got == entry["sha256"]:
            ok(name)
        else:
            bad(f"{name} has been modified — manifest says {entry['sha256']}, on disk {got}")

    print("== schema ceiling ==")
    if not db_path:
        print("SKIP no HELDAR_DB set — artifact check only")
    elif not os.path.isfile(db_path):
        # HELDAR_DB WAS set. Saying "no HELDAR_DB set" here would report the wrong reason for a
        # check that did not run, which is how the operator ends up believing it did.
        bad(f"HELDAR_DB is set to {db_path}, which is not a file — the schema check could not run")
    else:
        check_schema(db_path, ceilings)

    if fail:
        print("RESULT: FAIL")
        return 1
    print("RESULT: PASS")
    return 0


def check_schema(db_path: str, ceilings: dict) -> None:
    """Compare every component's applied migration against this release's ceiling.

    `kernel` is recorded by sqlx in `_sqlx_migrations`; every other component records its own in
    `_heldar_app_migrations` keyed by name. The first version of this checked only the kernel, so a
    release that moved the entry, movement or search schema shipped a ceiling nothing compared
    against — the exact upgrade failure this file exists to refuse, one table over.
    """
    try:
        conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
        tables = {
            r[0] for r in conn.execute("SELECT name FROM sqlite_master WHERE type='table'")
        }
    except sqlite3.Error as e:
        # NOT a fresh install. An unreadable database is the state in which skipping the check is
        # least defensible, so it is a refusal.
        bad(f"cannot read {db_path} as a database ({e}) — refusing to certify an unreadable database")
        return

    checked = 0
    for comp, ceil in sorted(ceilings.items()):
        applied = applied_version(conn, tables, comp)
        if applied is None:
            print(f"SKIP {comp}: no migrations applied yet (fresh install)")
            continue
        checked += 1
        if applied > ceil:
            bad(f"{comp} schema is at migration {applied} but this release supports at most {ceil} "
                f"— the database was written by a NEWER release, and this binary cannot safely serve it")
        else:
            ok(f"{comp} at {applied}, within this release's ceiling of {ceil}")
    if checked == 0:
        print(f"SKIP {db_path} has no applied migrations at all (fresh install)")


def applied_version(conn, tables: set, comp: str):
    """Highest applied migration for `comp`, or None if this database has none recorded."""
    if comp == "kernel":
        if "_sqlx_migrations" not in tables:
            return None
        row = conn.execute("SELECT MAX(version) FROM _sqlx_migrations").fetchone()
    else:
        if "_heldar_app_migrations" not in tables:
            return None
        row = conn.execute(
            "SELECT MAX(version) FROM _heldar_app_migrations WHERE component = ?", (comp,)
        ).fetchone()
    return None if row is None or row[0] is None else int(row[0])


if __name__ == "__main__":
    sys.exit(main())
