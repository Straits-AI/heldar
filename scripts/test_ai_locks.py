"""Negative controls for scripts/check_ai_locks.py.

A lock guard that cannot fail is a lock guard that is not doing anything. Each control breaks the
locks in exactly one way and asserts the checker says so — and, just as importantly, names the
right file, so the failure tells a contributor what to fix.
"""

from __future__ import annotations

import shutil
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_ai_locks as mod  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent


def _sandbox(fn):
    """Run fn against a throwaway copy of the tree, so a control never edits the real locks."""

    def wrapper():
        with tempfile.TemporaryDirectory() as tmp:
            tmp_root = Path(tmp) / "repo"
            (tmp_root / "apps" / "ai").mkdir(parents=True)
            shutil.copytree(ROOT / "apps" / "ai" / "constraints", tmp_root / "apps/ai/constraints")
            for rel in {r for inputs in mod.PROFILES.values() for r in inputs}:
                shutil.copy(ROOT / rel, tmp_root / rel)
            real_root, real_locks = mod.ROOT, mod.LOCKS
            mod.ROOT, mod.LOCKS = tmp_root, tmp_root / "apps/ai/constraints"
            try:
                fn(tmp_root)
            finally:
                mod.ROOT, mod.LOCKS = real_root, real_locks

    wrapper.__name__ = fn.__name__
    return wrapper


def check_passes_on_a_clean_tree():
    """The baseline. If this fails the other controls prove nothing."""

    @_sandbox
    def run(root: Path):
        assert mod.check() == [], mod.check()

    run()


def stale_lock_is_caught():
    """The failure this whole file exists for: inputs edited, lock not regenerated."""

    @_sandbox
    def run(root: Path):
        req = root / "apps/ai/requirements.txt"
        req.write_text(req.read_text() + "\nsomething-new>=1.0\n")
        problems = mod.check()
        stale = [p for p in problems if "STALE" in p]
        # detect, anpr and embed all consume requirements.txt; core does not.
        assert len(stale) == 3, problems
        assert not any("core.lock" in p for p in stale), stale

    run()


def unpinned_line_is_caught():
    """A range in a lock means the gate audits a version nobody chose."""

    @_sandbox
    def run(root: Path):
        lock = root / "apps/ai/constraints/core.lock"
        lock.write_text(lock.read_text().replace("requests==", "requests>="))
        problems = [p for p in mod.check() if "not pinned" in p]
        assert len(problems) == 1, mod.check()
        assert "core.lock" in problems[0] and "requests" in problems[0], problems

    run()


def missing_lock_is_caught():
    @_sandbox
    def run(root: Path):
        (root / "apps/ai/constraints/embed.lock").unlink()
        problems = [p for p in mod.check() if "missing" in p]
        assert len(problems) == 1 and "embed.lock" in problems[0], mod.check()

    run()


def orphan_lock_is_caught():
    """A lock for a profile that no longer exists keeps passing and covers nothing."""

    @_sandbox
    def run(root: Path):
        shutil.copy(
            root / "apps/ai/constraints/core.lock", root / "apps/ai/constraints/retired.lock"
        )
        problems = [p for p in mod.check() if "no profile in PROFILES" in p]
        assert len(problems) == 1 and "retired.lock" in problems[0], mod.check()

    run()


def missing_stamp_is_caught():
    """A lock with the digest line stripped must not be treated as current."""

    @_sandbox
    def run(root: Path):
        lock = root / "apps/ai/constraints/core.lock"
        lock.write_text(
            "\n".join(ln for ln in lock.read_text().splitlines() if not ln.startswith(mod.STAMP))
            + "\n"
        )
        problems = [p for p in mod.check() if "carries no" in p]
        assert len(problems) == 1 and "core.lock" in problems[0], mod.check()

    run()


def swapping_inputs_invalidates_the_lock():
    """Digest covers input NAMES, not just bytes — recomposing a profile must invalidate it."""

    @_sandbox
    def run(root: Path):
        before = mod.inputs_digest("anpr")
        original = mod.PROFILES["anpr"]
        mod.PROFILES["anpr"] = ("apps/ai/requirements-anpr.txt", "apps/ai/requirements.txt")
        try:
            assert mod.inputs_digest("anpr") != before, "input order/name is not part of the digest"
        finally:
            mod.PROFILES["anpr"] = original

    run()


CHECKS = [
    check_passes_on_a_clean_tree,
    stale_lock_is_caught,
    unpinned_line_is_caught,
    missing_lock_is_caught,
    orphan_lock_is_caught,
    missing_stamp_is_caught,
    swapping_inputs_invalidates_the_lock,
]

if __name__ == "__main__":
    for fn in CHECKS:
        fn()
        print(f"  ok    {fn.__name__.replace('_', ' ')}")
    print(f"\nall {len(CHECKS)} AI-lock controls passed")
