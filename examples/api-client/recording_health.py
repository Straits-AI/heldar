#!/usr/bin/env python3
"""Nightly recording-health check, built on the GENERATED Heldar client.

    python3 recording_health.py --url http://box:8000 --token vok_... [--hours 24]

Answers the question a recorder exists to answer: *did every camera actually record last night?* —
and reports the gaps, which is the part a green dashboard will not tell you.

Exit 0 when every camera was continuously recording; 1 when any gap was found; 2 when the box could
not be reached or refused the credential. A monitoring system can branch on that.

# Why this uses the generated client

Because the alternative teaches the wrong thing. A `curl` example is a fine way to see one endpoint,
and a bad model for an integration: it hardcodes paths, invents its own error handling, and goes
stale silently when a field is renamed. This file names methods that are generated from the contract
the server publishes, so a breaking change is a `AttributeError` at the top of a run rather than a
`KeyError` three hours into a night shift.

Regenerate the client (and this script's vocabulary) with:

    cargo test -p heldar-server --test openapi_contract write_the_served_document
    python3 scripts/gen_clients.py target/openapi.json clients

# What it does NOT do

It does not decide whether a gap is acceptable. A camera powered off for maintenance and a camera
that silently stopped recording produce identical gaps, and only your operations know the
difference. It reports; the judgement is yours.
"""

import argparse
import datetime as dt
import sys
from pathlib import Path

# The generated client. Kept out of the import path assumptions by locating it relative to this
# file, so the example runs from a checkout without an install step.
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "clients" / "python"))
import heldar_client  # noqa: E402


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--url", default="http://localhost:8000", help="the box")
    ap.add_argument("--token", help="an API key with `camera:read` and `video:playback`")
    ap.add_argument("--hours", type=int, default=24, help="how far back to look")
    args = ap.parse_args()

    client = heldar_client.HeldarClient(args.url, args.token)
    now = dt.datetime.now(dt.timezone.utc)
    since = now - dt.timedelta(hours=args.hours)
    frm, to = since.isoformat().replace("+00:00", "Z"), now.isoformat().replace("+00:00", "Z")

    try:
        cameras = client.list_cameras()
    except heldar_client.HeldarError as e:
        # One error shape for every endpoint, so one error path. `code` is stable; the message is not.
        print(f"could not list cameras: {e} (code={e.code}, retryable={e.retryable})",
              file=sys.stderr)
        return 2
    except OSError as e:
        print(f"could not reach {args.url}: {e}", file=sys.stderr)
        return 2

    # A camera-scoped credential sees only its own cameras here, and that is the correct answer for
    # it — not an error, and not the whole fleet.
    if not cameras:
        print("no cameras visible to this credential")
        return 0

    total_gap_seconds = 0.0
    findings = []
    for cam in cameras:
        cam_id = cam["id"]
        if not cam.get("record_enabled", True):
            findings.append((cam_id, None, "recording disabled — not checked"))
            continue
        try:
            gaps = client.list_gaps(cam_id)
        except heldar_client.HeldarError as e:
            # A 404 here means the credential does not hold this camera, which cannot happen for a
            # camera it just listed — so it is worth reporting rather than swallowing.
            findings.append((cam_id, None, f"could not read gaps: {e.code}"))
            continue
        spans = gaps.get("gaps", []) if isinstance(gaps, dict) else gaps
        if not spans:
            continue
        secs = sum(_seconds(g) for g in spans)
        total_gap_seconds += secs
        findings.append((cam_id, secs, f"{len(spans)} gap(s), {_human(secs)} missing"))

    print(f"recording health · {args.hours}h to {to}")
    print(f"  cameras checked: {len(cameras)}")
    if not findings:
        print("  no gaps — every camera recorded continuously")
        return 0
    for cam_id, _secs, note in sorted(findings, key=lambda f: -(f[1] or 0)):
        print(f"  {cam_id:24} {note}")
    print(f"\n  total missing: {_human(total_gap_seconds)}")
    print("\n  A gap is not automatically a fault: a camera powered off for maintenance looks the")
    print("  same as one that stopped recording. This reports; the judgement is yours.")
    return 1


def _seconds(gap) -> float:
    try:
        a = dt.datetime.fromisoformat(str(gap["from"]).replace("Z", "+00:00"))
        b = dt.datetime.fromisoformat(str(gap["to"]).replace("Z", "+00:00"))
        return max(0.0, (b - a).total_seconds())
    except (KeyError, TypeError, ValueError):
        return 0.0


def _human(seconds: float) -> str:
    m, s = divmod(int(seconds), 60)
    h, m = divmod(m, 60)
    return f"{h}h{m:02d}m" if h else (f"{m}m{s:02d}s" if m else f"{s}s")


if __name__ == "__main__":
    sys.exit(main())
