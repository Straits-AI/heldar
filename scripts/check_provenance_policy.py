#!/usr/bin/env python3
"""Every document must agree that frame tickets are never auto-enabled (issue #117).

`HELDAR_DEPLOYMENT_MODE=production*` promotes `HELDAR_MACHINE_AUTH` and deliberately does NOT promote
`HELDAR_INGEST_PROVENANCE`. Seven files state that policy. The behaviour is pinned by
`config::promotion_policy_tests`; this pins the prose, because #117 was filed when the two had
already drifted apart — the README claimed production mode promoted the tier while the code, the ADR
and three other documents said the opposite.

Which direction the drift takes decides which way an operator is hurt:

  * a document that says it IS promoted -> the operator believes ticketless ingest is rejected when
    it is still accepted, and stops looking;
  * a document that says it is NOT, against code that promotes it -> the operator enables production
    mode expecting a server-side change and silently loses every AI worker that cannot mint a ticket.

So the rule is narrow and mechanical: any line that ties the deployment mode to the frame-ticket
requirement must carry an explicit negation. That is cheap to satisfy honestly and hard to satisfy
by accident.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Where the policy is stated. Missing files are an error: a document being deleted is exactly when
# the remaining ones start disagreeing unnoticed.
POLICY_DOCS = (
    "README.md",
    "docs/AI-WORKERS.md",
    "docs/adr/0005-task-lease-bound-ai-ingest.md",
    "deploy/compose.prod.yml",
    ".env.production.example",
    "crates/heldar-kernel/src/config.rs",
)

# Bare "production" counts too. An earlier version required the words "production mode" and so
# missed "HELDAR_INGEST_PROVENANCE is set to enforce when running in production" — the same
# false claim in the phrasing someone would most naturally write. Only consulted on lines that
# ALSO name the ticket requirement, so the breadth costs nothing.
MODE = re.compile(
    r"HELDAR_DEPLOYMENT_MODE|deployment mode|deployment label|\bproduction\b", re.I
)
TICKETS = re.compile(r"HELDAR_INGEST_PROVENANCE|frame[- ]ticket|ingest_provenance", re.I)

# An explicit denial that the mode turns tickets on. Deliberately a phrase list rather than a bare
# "not": prose says "not" for many reasons, and a guard that accepts any of them accepts anything.
NEGATIONS = (
    "not auto-promoted", "never promoted", "not promoted", "no deployment mode",
    "deliberately leaves", "must be set explicitly", "explicit opt-in",
    "not set here", "never turned on for you", "explicitly", "not even by",
    "leaves .* alone", "only .*machine_auth", "machine_auth only",
)
NEGATION_RE = re.compile("|".join(NEGATIONS), re.I)

# Verbs that assert the mode turns tickets ON. Order-independent on purpose: an earlier version
# matched "production ... promotes ... frame-ticket" as an ordered pattern and missed "In production
# mode, frame-ticket enforcement is turned on automatically" — the same false claim, different
# sentence shape. Negation is checked FIRST, so "No deployment mode promotes this tier" is clean
# despite containing an enabling verb.
ENABLING_VERB = re.compile(
    r"\b(promot\w*|enabl\w*|turn(s|ed)? on|requir\w*|switch(es|ed)? on|activat\w*|"
    r"set to enforce|becomes enforce|implies)\b",
    re.I,
)

# In a source file the policy lives in comments; the code itself legitimately names both variables
# side by side (a test clearing the environment, a struct field list) and is not making a claim about
# anything. Judging those lines as prose produces exactly the kind of noise that gets a guard muted.
COMMENT_PREFIXES = ("//", "#", "*", "<!--")


def _is_prose(line: str, name: str) -> bool:
    if name.endswith((".md", ".example")):
        return True
    return line.lstrip().startswith(COMMENT_PREFIXES)


def check_text(text: str, name: str) -> list[str]:
    problems: list[str] = []
    lines = text.splitlines()
    for i, line in enumerate(lines, 1):
        if not (MODE.search(line) and TICKETS.search(line)):
            continue
        if not _is_prose(line, name):
            continue
        # Prose wraps, so judge the sentence-ish window around the line, not the line alone.
        window = "\n".join(lines[max(0, i - 3): i + 3])
        if NEGATION_RE.search(window):
            continue  # states the asymmetry plainly — the whole point
        if hit := ENABLING_VERB.search(window):
            problems.append(
                f"{name}:{i}: claims the deployment mode enables frame tickets "
                f"({hit.group(0).strip()!r}). It does not — see config::promotion_policy_tests. "
                f"An operator reading this believes ticketless ingest is rejected when it is "
                f"still accepted."
            )
        else:
            problems.append(
                f"{name}:{i}: ties the deployment mode to the frame-ticket requirement without "
                f"saying it is NOT promoted. Ambiguity here is how #117 happened; state it plainly."
            )
    return problems


def check() -> list[str]:
    problems: list[str] = []
    for rel in POLICY_DOCS:
        path = ROOT / rel
        if not path.exists():
            problems.append(f"{rel} is missing — it is one of the documents stating this policy")
            continue
        problems += check_text(path.read_text(), rel)
    return problems


def main() -> int:
    problems = check()
    prefix = "ERROR: " if sys.stdout.isatty() else "::error::"
    for p in problems:
        print(f"{prefix}{p}")
    if problems:
        print(f"\n{len(problems)} document(s) disagree about the frame-ticket policy", file=sys.stderr)
        return 1
    print(f"ok — {len(POLICY_DOCS)} documents agree that frame tickets are never auto-enabled")
    return 0


if __name__ == "__main__":
    sys.exit(main())
