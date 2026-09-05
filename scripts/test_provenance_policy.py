"""Controls for scripts/check_provenance_policy.py.

#117 exists because the README said production mode promotes `HELDAR_INGEST_PROVENANCE` while the
code, the ADR and three other documents said it never does. The guard has to catch that sentence
coming back, and — just as important — must not cry wolf on the correct phrasing that is in the tree
today, because a noisy guard gets muted and then it is worse than nothing.
"""

from __future__ import annotations

import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_provenance_policy as mod  # noqa: E402


def _expect_flag(text, name, needle, label):
    problems = mod.check_text(text, name)
    assert problems, f"{label}: expected a finding, got none"
    assert any(needle in p for p in problems), f"{label}: expected {needle!r}, got {problems}"


def _expect_clean(text, name, label):
    problems = mod.check_text(text, name)
    assert not problems, f"{label}: expected no findings, got {problems}"


def the_real_documents_agree():
    """Baseline. Without it, every control below could pass for the wrong reason."""
    assert mod.check() == [], mod.check()


def the_sentence_that_caused_the_issue_is_caught():
    """The README's original claim, verbatim in spirit."""
    _expect_flag(
        "Production deployment mode promotes HELDAR_INGEST_PROVENANCE to enforce automatically.\n",
        "README.md", "claims the deployment mode enables frame tickets", "the #117 sentence")


def other_ways_of_saying_the_same_wrong_thing_are_caught():
    for text in (
        "Setting HELDAR_DEPLOYMENT_MODE=production enables frame tickets for you.\n",
        "In production mode, frame-ticket enforcement is turned on automatically.\n",
        "HELDAR_INGEST_PROVENANCE is set to enforce when running in production.\n",
        "production-lan requires frame tickets on the ingest path.\n",
    ):
        _expect_flag(text, "docs/AI-WORKERS.md",
                     "claims the deployment mode enables frame tickets", text.strip()[:40])


def an_ambiguous_co_mention_is_caught():
    """Silence about the asymmetry is how a reader supplies the wrong half themselves."""
    _expect_flag(
        "See HELDAR_DEPLOYMENT_MODE and HELDAR_INGEST_PROVENANCE for the production posture.\n",
        "README.md", "without saying it is NOT promoted", "ambiguous")


def the_correct_phrasing_passes():
    for text in (
        "HELDAR_INGEST_PROVENANCE is **NOT auto-promoted by HELDAR_DEPLOYMENT_MODE**.\n",
        "No deployment mode promotes this tier; HELDAR_INGEST_PROVENANCE is explicit opt-in only.\n",
        "production* promotes HELDAR_MACHINE_AUTH and deliberately leaves "
        "HELDAR_INGEST_PROVENANCE alone.\n",
        "It is never promoted automatically — not even by HELDAR_DEPLOYMENT_MODE=production*.\n",
    ):
        _expect_clean(text, "README.md", text.strip()[:40])


def a_negation_a_few_lines_away_still_counts():
    """Prose wraps; the negation is often on the next line, not the one naming both variables."""
    _expect_clean(
        "HELDAR_DEPLOYMENT_MODE=production* and HELDAR_INGEST_PROVENANCE interact in one way\n"
        "only: the mode promotes machine auth and is never promoted for the ingest tier,\n"
        "which must be set explicitly.\n",
        "docs/AI-WORKERS.md", "wrapped negation")


def code_that_merely_names_both_variables_is_not_prose():
    """A test clearing the environment is not making a claim. Judging it flags a real file."""
    _expect_clean(
        'for k in ["HELDAR_MACHINE_AUTH", "HELDAR_INGEST_PROVENANCE", "HELDAR_DEPLOYMENT_MODE"] {\n'
        "    std::env::remove_var(k);\n}\n",
        "crates/heldar-kernel/src/config.rs", "rust code")
    # ...but a comment in the same file IS.
    _expect_flag(
        "// production mode promotes HELDAR_INGEST_PROVENANCE for you\n",
        "crates/heldar-kernel/src/config.rs",
        "claims the deployment mode enables frame tickets", "rust comment")


def a_deleted_policy_document_is_caught():
    """A document vanishing is exactly when the remaining ones start disagreeing unnoticed."""
    real = mod.ROOT
    with tempfile.TemporaryDirectory() as d:
        mod.ROOT = Path(d)
        try:
            problems = mod.check()
        finally:
            mod.ROOT = real
    assert len(problems) == len(mod.POLICY_DOCS), problems
    assert all("is missing" in p for p in problems), problems


CHECKS = [
    the_real_documents_agree,
    the_sentence_that_caused_the_issue_is_caught,
    other_ways_of_saying_the_same_wrong_thing_are_caught,
    an_ambiguous_co_mention_is_caught,
    the_correct_phrasing_passes,
    a_negation_a_few_lines_away_still_counts,
    code_that_merely_names_both_variables_is_not_prose,
    a_deleted_policy_document_is_caught,
]

if __name__ == "__main__":
    for fn in CHECKS:
        fn()
        print(f"  ok    {fn.__name__.replace('_', ' ')}")
    print(f"\nall {len(CHECKS)} provenance-policy controls passed")
