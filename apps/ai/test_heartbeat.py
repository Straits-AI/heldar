"""Controls for the supervisor heartbeat and the deadline it publishes.

The heartbeat exists so a container health check can tell a LIVE worker from a wedged one — the
worker serves no HTTP, so there is nothing else to probe. Its first version hardcoded a 90s
threshold in compose.yml, and an independent review showed that was wrong in both directions:

  * TOO TIGHT — one retry-exhausted poll is (retries+1) x timeout plus backoff. At the DEFAULTS
    that is ~79s, and one poll interval on top left 0.6s of margin. What consumes it is a kernel
    the worker cannot reach, which is exactly what the check is documented as NOT reporting on.
  * TOO LOOSE — HELDAR_AI_POLL_INTERVAL has no ceiling, so setting it above the threshold made a
    perfectly healthy worker permanently unhealthy, on every cycle, with nothing wrong.

The worker now publishes its own deadline and the check only asks whether it has passed. These
controls pin that against both failure modes.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import worker  # noqa: E402


def _supervisor(**overrides):
    """A Supervisor with real Settings, without touching the network."""
    os.environ.setdefault("HELDAR_AI_HEARTBEAT_FILE", "")
    base = worker.parse_settings([])
    s = worker.Settings(**{**base.__dict__, **overrides})
    sup = worker.Supervisor.__new__(worker.Supervisor)
    sup.s = s
    sup._heartbeat_warned = False
    return sup


def worst_case_poll_seconds(s) -> float:
    """The longest one supervisor cycle can legitimately take, derived independently here.

    Deliberately NOT calling the worker's own helper: a regression test that reuses the code under
    test would move with the bug.
    """
    attempts = s.http_max_retries + 1
    backoff = sum(
        min(s.backoff_cap, s.backoff_base * (2 ** (a - 1))) * 1.25  # worst-case jitter
        for a in range(1, attempts)
    )
    return attempts * s.http_timeout + backoff + s.poll_interval


def the_deadline_survives_a_retry_exhausted_poll():
    """The 0.6s-margin regression. A kernel outage must not mark a working worker unhealthy."""
    sup = _supervisor()
    worst = worst_case_poll_seconds(sup.s)
    gap = sup._max_beat_gap()
    assert gap > worst, (
        f"deadline {gap:.1f}s does not survive a worst-case cycle of {worst:.1f}s — "
        f"a core outage would report the worker as wedged"
    )
    # And with real headroom, not by a hair.
    assert gap >= worst * 1.5, f"only {gap - worst:.1f}s of margin over {worst:.1f}s"


def the_deadline_tracks_a_raised_poll_interval():
    """The permanently-unhealthy regression: the interval has no ceiling, so a fixed bar breaks."""
    for interval in (10.0, 60.0, 120.0, 600.0):
        sup = _supervisor(poll_interval=interval)
        assert sup._max_beat_gap() > worst_case_poll_seconds(sup.s), interval
        assert sup._max_beat_gap() > interval, interval


def the_deadline_tracks_raised_http_settings():
    """Both are documented, uncapped env vars an operator raises for a slow network."""
    sup = _supervisor(http_timeout=60.0, http_max_retries=10)
    assert sup._max_beat_gap() > worst_case_poll_seconds(sup.s)


def a_beat_publishes_a_future_deadline():
    with tempfile.TemporaryDirectory() as d:
        path = Path(d) / "nested" / "hb"
        sup = _supervisor(heartbeat_file=str(path))
        sup._beat()
        assert path.exists(), "parent directories must be created"
        rec = json.loads(path.read_text())
        assert set(rec) == {"ts", "stale_after"}, rec
        assert rec["stale_after"] > time.time(), "a fresh beat must read as healthy"
        assert rec["stale_after"] > rec["ts"]


def the_healthcheck_expression_agrees_with_the_beat():
    """Run compose.yml's actual health-check logic against a real beat, and against a stale one."""
    with tempfile.TemporaryDirectory() as d:
        path = Path(d) / "hb"
        sup = _supervisor(heartbeat_file=str(path))
        sup._beat()

        def verdict() -> int:
            try:
                with open(path) as f:
                    return 0 if json.load(f)["stale_after"] > time.time() else 1
            except Exception:
                return 1

        assert verdict() == 0, "a fresh beat must be healthy"

        rec = json.loads(path.read_text())
        rec["stale_after"] = time.time() - 1
        path.write_text(json.dumps(rec))
        assert verdict() == 1, "a passed deadline must be unhealthy"

        path.write_text("not json")
        assert verdict() == 1, "an unreadable heartbeat is not healthy"
        path.unlink()
        assert verdict() == 1, "a missing heartbeat is not healthy"


def an_unwritable_heartbeat_never_takes_down_the_worker():
    """Health reporting must not be able to kill the thing it reports on."""
    sup = _supervisor(heartbeat_file="/proc/definitely/not/writable/hb")
    sup._beat()
    sup._beat()
    assert sup._heartbeat_warned, "it should say so once"


def an_empty_path_disables_it():
    sup = _supervisor(heartbeat_file="")
    sup._beat()  # must not raise, must not create anything


CHECKS = [
    the_deadline_survives_a_retry_exhausted_poll,
    the_deadline_tracks_a_raised_poll_interval,
    the_deadline_tracks_raised_http_settings,
    a_beat_publishes_a_future_deadline,
    the_healthcheck_expression_agrees_with_the_beat,
    an_unwritable_heartbeat_never_takes_down_the_worker,
    an_empty_path_disables_it,
]

if __name__ == "__main__":
    for fn in CHECKS:
        fn()
        print(f"  ok    {fn.__name__.replace('_', ' ')}")
    print(f"\nall {len(CHECKS)} heartbeat controls passed")
