"""A Thread subclass must not shadow a real threading.Thread member.

TaskRunner used to assign `self._stop = threading.Event()`. `Thread._stop` is a
real CPython method that `join()` calls, via `_wait_for_tstate_lock`, once the
thread has finished — so every graceful shutdown died with

    TypeError: 'Event' object is not callable

after the runners had already stopped cleanly. The worker exited 1 and systemd
reported the unit as failed on every single `systemctl stop`, for months.

Static because the runtime path needs a live camera: the collision is visible in
the source, and that is where it has to be caught.
"""

from __future__ import annotations

import ast
import threading
from pathlib import Path

APPS_AI = Path(__file__).resolve().parent

# Every attribute a threading.Thread instance already carries. Assigning any of
# these in a subclass replaces the real thing.
THREAD_MEMBERS = frozenset(dir(threading.Thread)) | frozenset(
    vars(threading.Thread(target=lambda: None))
)

# Names a subclass is expected to set, because Thread itself documents them.
ALLOWED = frozenset({"name", "daemon"})


def _is_thread_base(base: ast.expr) -> bool:
    """`class X(threading.Thread)` or `class X(Thread)`."""
    if isinstance(base, ast.Attribute):
        return base.attr == "Thread"
    return isinstance(base, ast.Name) and base.id == "Thread"


def _self_assignments(cls: ast.ClassDef) -> list[tuple[str, int]]:
    """Every `self.X = ...` / `self.X: T = ...` anywhere in the class body."""
    found = []
    for node in ast.walk(cls):
        targets: list[ast.expr] = []
        if isinstance(node, ast.Assign):
            targets = list(node.targets)
        elif isinstance(node, (ast.AnnAssign, ast.AugAssign)):
            targets = [node.target]
        for target in targets:
            if (
                isinstance(target, ast.Attribute)
                and isinstance(target.value, ast.Name)
                and target.value.id == "self"
            ):
                found.append((target.attr, target.lineno))
    return found


def thread_subclass_collisions(source: str, filename: str) -> list[str]:
    problems = []
    for cls in ast.walk(ast.parse(source)):
        if not isinstance(cls, ast.ClassDef):
            continue
        if not any(_is_thread_base(b) for b in cls.bases):
            continue
        for attr, lineno in _self_assignments(cls):
            if attr in ALLOWED or attr not in THREAD_MEMBERS:
                continue
            problems.append(
                f"{filename}:{lineno}: {cls.name}(Thread) assigns self.{attr}, "
                f"which shadows threading.Thread.{attr}"
            )
    return problems


def test_no_thread_subclass_shadows_a_thread_member() -> None:
    scanned = sorted(APPS_AI.glob("*.py"))
    assert scanned, "found no Python sources to scan"

    problems = []
    for path in scanned:
        problems += thread_subclass_collisions(path.read_text(), path.name)
    assert not problems, "\n".join(problems)


def test_guard_catches_the_bug_it_was_written_for() -> None:
    """The regression itself must trip the guard — otherwise the guard is decoration."""
    problems = thread_subclass_collisions(
        "import threading\n"
        "class TaskRunner(threading.Thread):\n"
        "    def __init__(self):\n"
        "        self._stop = threading.Event()\n",
        "regression.py",
    )
    assert len(problems) == 1, problems
    assert "shadows threading.Thread._stop" in problems[0]


def test_guard_is_not_indiscriminate() -> None:
    """A non-colliding attribute, and a plain class, must both stay silent."""
    assert not thread_subclass_collisions(
        "import threading\n"
        "class Ok(threading.Thread):\n"
        "    def __init__(self):\n"
        "        self._stop_event = threading.Event()\n"
        "        self.name = 'fine'\n",
        "ok.py",
    )
    assert not thread_subclass_collisions(
        "class NotAThread:\n"
        "    def __init__(self):\n"
        "        self._stop = object()\n",
        "plain.py",
    )


if __name__ == "__main__":
    test_no_thread_subclass_shadows_a_thread_member()
    test_guard_catches_the_bug_it_was_written_for()
    test_guard_is_not_indiscriminate()
    print("ok")
