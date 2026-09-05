"""A Thread subclass must not shadow a real threading.Thread member.

TaskRunner used to assign `self._stop = threading.Event()`. `Thread._stop` is a
real CPython method that `join()` calls, via `_wait_for_tstate_lock`, once the
thread has finished — so every graceful shutdown died with

    TypeError: 'Event' object is not callable

after the runners had already stopped cleanly. The worker exited 1 and systemd
reported the unit as failed on every single `systemctl stop` — since 87d411d
(2026-06-13), the commit that introduced TaskRunner. Not one clean stop in the
box's whole journal.

Static because the runtime path needs a live camera: the collision is visible in
the source, and that is where it has to be caught.

Do NOT derive the member list from the running interpreter alone. CPython 3.13
DELETED `Thread._stop`, so on 3.13+ the collision does not exist and a guard that
asks the live `threading.Thread` finds nothing to report — while the box runs
3.12, where it very much does exist. Written that way, this file passed on the
CI matrix's newer half and would have gone on passing after the rename was undone.
"""

from __future__ import annotations

import ast
import sys
import threading
from pathlib import Path

APPS_AI = Path(__file__).resolve().parent

# Members that a supported CPython has carried on Thread but which the interpreter
# running this file may not, because they were removed after 3.12. `_stop` is the
# whole reason this file exists and is gone from 3.13 onwards; the rest travel with
# it in the same internal cleanup. Shadowing any of them breaks the versions that
# still have them, so the guard has to know them by name.
REMOVED_AFTER_3_12 = frozenset(
    {
        "_handle",
        "_is_stopped",
        "_reset_internal_locks",
        "_set_tstate_lock",
        "_stop",
        "_tstate_lock",
        "_wait_for_tstate_lock",
    }
)

# Every attribute a Thread instance carries on ANY supported interpreter. Assigning
# one of these in a subclass replaces the real thing. Unioned with the live class so
# members added in future versions are covered without editing this list.
THREAD_MEMBERS = (
    frozenset(dir(threading.Thread))
    | frozenset(vars(threading.Thread(target=lambda: None)))
    | REMOVED_AFTER_3_12
)

# Names a subclass is expected to set, because Thread itself documents them.
ALLOWED = frozenset({"name", "daemon"})


def _base_name(base: ast.expr) -> str | None:
    """`threading.Thread` -> "Thread"; `Runner` -> "Runner"; anything else -> None."""
    if isinstance(base, ast.Attribute):
        return base.attr
    if isinstance(base, ast.Name):
        return base.id
    return None


def _thread_subclasses(tree: ast.Module) -> list[ast.ClassDef]:
    """Every class that reaches threading.Thread, directly OR through a local base.

    Name-matching only the immediate base misses the shape that actually hides this
    bug in real code — a `class _RunnerBase(threading.Thread)` with the collision one
    level down in `class TaskRunner(_RunnerBase)`. Resolve local bases transitively.
    """
    classes = {n.name: n for n in ast.walk(tree) if isinstance(n, ast.ClassDef)}
    found: dict[str, ast.ClassDef] = {}

    def reaches_thread(cls: ast.ClassDef, seen: frozenset[str]) -> bool:
        if cls.name in seen:  # cyclic bases are not valid Python, but do not hang on them
            return False
        for base in cls.bases:
            name = _base_name(base)
            if name == "Thread":
                return True
            parent = classes.get(name) if name else None
            if parent is not None and reaches_thread(parent, seen | {cls.name}):
                return True
        return False

    for name, cls in classes.items():
        if reaches_thread(cls, frozenset()):
            found[name] = cls
    return list(found.values())


def _setattr_name(node: ast.AST) -> str | None:
    """`setattr(self, "_stop", ...)` -> "_stop". Same shadowing, no assignment node."""
    if not isinstance(node, ast.Call) or not isinstance(node.func, ast.Name):
        return None
    if node.func.id != "setattr" or len(node.args) < 2:
        return None
    obj, attr = node.args[0], node.args[1]
    if not (isinstance(obj, ast.Name) and obj.id == "self"):
        return None
    return attr.value if isinstance(attr, ast.Constant) and isinstance(attr.value, str) else None


def _shadowing_names(cls: ast.ClassDef) -> list[tuple[str, int]]:
    """Every name the class binds on itself or its instances, however it binds it.

    Three shapes, because all three shadow identically at runtime:
      self.X = ...            instance attribute
      X = ...                 class body — resolves through the MRO just the same
      setattr(self, "X", ...) invisible to a scan that only reads assignment targets
    """
    found: list[tuple[str, int]] = []

    for stmt in cls.body:  # class-body bindings: direct children only, not nested scopes
        targets: list[ast.expr] = []
        if isinstance(stmt, ast.Assign):
            targets = list(stmt.targets)
        elif isinstance(stmt, (ast.AnnAssign, ast.AugAssign)):
            targets = [stmt.target]
        for target in targets:
            if isinstance(target, ast.Name):
                found.append((target.id, target.lineno))

    for node in ast.walk(cls):
        if (name := _setattr_name(node)) is not None:
            found.append((name, node.lineno))

        targets = []
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
    for cls in _thread_subclasses(ast.parse(source)):
        for attr, lineno in _shadowing_names(cls):
            if attr in ALLOWED or attr not in THREAD_MEMBERS:
                continue
            problems.append(
                f"{filename}:{lineno}: {cls.name}(Thread) binds {attr}, "
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


def test_guard_does_not_depend_on_the_running_interpreter() -> None:
    """`_stop` must be flagged on 3.13+, which no longer has it.

    The guard's job is to protect the deployed interpreter, not the one CI happens
    to run. This is the assertion that fails if REMOVED_AFTER_3_12 is dropped.
    """
    assert "_stop" in THREAD_MEMBERS, (
        "the name this whole file exists for is not in the member set on "
        f"python{sys.version_info.major}.{sys.version_info.minor}"
    )
    assert REMOVED_AFTER_3_12 <= THREAD_MEMBERS


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


def test_guard_catches_every_way_of_writing_the_same_collision() -> None:
    """Three evasions an independent review found. All shadow `_stop` identically."""
    cases = {
        "setattr, not an assignment target": (
            "import threading\n"
            "class TaskRunner(threading.Thread):\n"
            "    def __init__(self):\n"
            "        setattr(self, '_stop', threading.Event())\n"
        ),
        "class body, not an instance attribute": (
            "import threading\n"
            "class TaskRunner(threading.Thread):\n"
            "    _stop = threading.Event()\n"
        ),
        "Thread reached through an intermediate base": (
            "import threading\n"
            "class _RunnerBase(threading.Thread):\n"
            "    pass\n"
            "class TaskRunner(_RunnerBase):\n"
            "    def __init__(self):\n"
            "        self._stop = threading.Event()\n"
        ),
    }
    for label, src in cases.items():
        problems = thread_subclass_collisions(src, "evasion.py")
        assert problems, f"guard is blind to: {label}"
        assert any("_stop" in p for p in problems), (label, problems)


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
    test_guard_does_not_depend_on_the_running_interpreter()
    test_guard_catches_the_bug_it_was_written_for()
    test_guard_catches_every_way_of_writing_the_same_collision()
    test_guard_is_not_indiscriminate()
    print("ok")
