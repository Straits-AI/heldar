#!/usr/bin/env python3
"""Verify the pinned AI dependency locks still describe what would actually install.

Issue #114. `pip-audit` was pointed at `requirements-core.txt` alone, so the largest and
fastest-moving native trees in the product — torch, opencv, the CUDA wheels, PaddleOCR — were
invisible to the blocking gate. Auditing the requirements files directly would not have fixed it
either: they carry open floors (`ultralytics>=8.4.115`), so the audit reports on whatever the
resolver picked this morning rather than on a version anyone can ship.

So the gate audits committed locks, and this file is what keeps a lock honest:

  * every documented profile HAS a lock,
  * each lock records the sha256 of the exact inputs it was compiled from, and still matches them,
  * every line in it is pinned with `==`, never a range,
  * no profile is defined here without being installable from the README.

The failure this prevents is the quiet one: someone edits requirements.txt, does not regenerate,
and the gate keeps auditing last month's tree while reporting success.

Regenerate with scripts/lock_ai_profiles.sh. That script reads its profile list from THIS file
(`--profiles`), so the mapping exists once.
"""

from __future__ import annotations

import argparse
import hashlib
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LOCKS = ROOT / "apps" / "ai" / "constraints"

# The four installs apps/ai/README.md documents, in install order. The single source of truth for
# what "a shippable AI profile" means: the lock script, the CI audit matrix and the staleness check
# all read it from here.
PROFILES: dict[str, tuple[str, ...]] = {
    "core": ("apps/ai/requirements-core.txt",),
    "detect": ("apps/ai/requirements.txt",),
    "anpr": ("apps/ai/requirements.txt", "apps/ai/requirements-anpr.txt"),
    "embed": ("apps/ai/requirements.txt", "apps/ai/requirements-embed.txt"),
}

STAMP = "# inputs-sha256: "
PINNED = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*(\[[^\]]+\])?==[^\s;]+")


def inputs_digest(profile: str) -> str:
    """sha256 over the profile's inputs, name and bytes, in install order.

    Names are hashed too: swapping which files compose a profile has to invalidate the lock even
    if the bytes happen to be identical.
    """
    h = hashlib.sha256()
    for rel in PROFILES[profile]:
        path = ROOT / rel
        if not path.exists():
            raise SystemExit(f"profile {profile!r} names a missing input: {rel}")
        h.update(rel.encode())
        h.update(b"\0")
        h.update(path.read_bytes())
        h.update(b"\0")
    return h.hexdigest()


def lock_path(profile: str) -> Path:
    return LOCKS / f"{profile}.lock"


def stamp(profile: str) -> None:
    """Record the input digest in the freshly compiled lock."""
    path = lock_path(profile)
    body = [ln for ln in path.read_text().splitlines() if not ln.startswith(STAMP)]
    header = [
        f"# Pinned lock for the {profile!r} AI profile — GENERATED, do not edit by hand.",
        f"# Regenerate with scripts/lock_ai_profiles.sh after changing: {', '.join(PROFILES[profile])}",
        f"{STAMP}{inputs_digest(profile)}",
    ]
    path.write_text("\n".join(header + body) + "\n")


def check() -> list[str]:
    problems: list[str] = []

    for profile in PROFILES:
        path = lock_path(profile)
        if not path.exists():
            problems.append(
                f"{path.relative_to(ROOT)} is missing — run scripts/lock_ai_profiles.sh"
            )
            continue

        text = path.read_text()
        recorded = next(
            (ln[len(STAMP):].strip() for ln in text.splitlines() if ln.startswith(STAMP)), None
        )
        if recorded is None:
            problems.append(f"{path.relative_to(ROOT)} carries no {STAMP.strip()} line")
        elif recorded != (actual := inputs_digest(profile)):
            problems.append(
                f"{path.relative_to(ROOT)} is STALE: compiled from inputs {recorded[:12]}, "
                f"but {', '.join(PROFILES[profile])} now hash to {actual[:12]}. "
                f"Run scripts/lock_ai_profiles.sh and commit the result."
            )

        unpinned = [
            (i, ln)
            for i, ln in enumerate(text.splitlines(), 1)
            if ln.strip()
            and not ln.lstrip().startswith("#")
            and not ln.startswith(" ")
            and not PINNED.match(ln.strip())
        ]
        for lineno, ln in unpinned:
            problems.append(
                f"{path.relative_to(ROOT)}:{lineno}: not pinned with '==': {ln.strip()!r}. "
                f"An unpinned line means the gate audits a version nobody chose."
            )

    # A lock file for a profile that no longer exists is worse than none: it keeps passing the
    # audit and covers nothing.
    for path in sorted(LOCKS.glob("*.lock")) if LOCKS.exists() else []:
        if path.stem not in PROFILES:
            problems.append(
                f"{path.relative_to(ROOT)} has no profile in PROFILES — delete it, or add the profile"
            )

    return problems


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--stamp", metavar="PROFILE", help="record the input digest in one lock")
    ap.add_argument("--profiles", action="store_true", help="print 'name<TAB>inputs' for each profile")
    args = ap.parse_args()

    if args.stamp:
        if args.stamp not in PROFILES:
            raise SystemExit(f"unknown profile {args.stamp!r}; known: {', '.join(PROFILES)}")
        stamp(args.stamp)
        return 0

    if args.profiles:
        for name, inputs in PROFILES.items():
            print(f"{name}\t{' '.join(inputs)}")
        return 0

    problems = check()
    prefix = "ERROR: " if sys.stdout.isatty() else "::error::"  # GitHub annotation off a terminal
    for p in problems:
        print(f"{prefix}{p}")
    if problems:
        print(f"\n{len(problems)} problem(s) with the AI dependency locks", file=sys.stderr)
        return 1
    print(f"ok — {len(PROFILES)} AI profile lock(s) present, pinned and current")
    return 0


if __name__ == "__main__":
    sys.exit(main())
