#!/usr/bin/env python3
"""A deterministic AI worker, for the synthetic stack (#113).

Usage: mock_analyzer.py <api-base> [--worker-id ID] [--interval SECONDS]

`apps/ai/worker.py` is the real thing: it loads models and its output depends on what they see. That
makes it the wrong instrument for a CI gate, where the question is not "did a detector find a person"
but "does the PIPELINE carry a worker's output from a live frame to stored, attributable evidence".

So this walks the same contract and emits a fixed detection:

    POST /api/v1/ai/leases            take (or renew) a lease over eligible tasks
    GET  <task.frame_url>             pull a frame; the response carries x-frame-ticket
    POST /api/v1/ai/events            post the detection, presenting that ticket

Every answer is constant, so a failure means the pipeline changed, never that a model did.

# What it deliberately does not do

No model, no decode, no crop embeddings. It does not retry a failed lease or back off — the stack it
runs in lives for a couple of minutes, and a worker that hid failures behind retries would make the
gate quieter rather than more honest. It logs what it did and keeps going.

# Why the frame_url is used verbatim

The server builds it with `?profile=…&task=…` precisely so a worker does not assemble it. Appending
another `?task=` produces a second `?`, the server sees no valid task, and no ticket is minted — a
bug this repository has now written twice, once in `validate_ai.sh` before it was caught.
"""

import argparse
import json
import sys
import time
import urllib.error
import urllib.request


def call(method, url, body=None, timeout=15):
    """(status, headers, parsed-or-raw-bytes). Never raises for an HTTP status."""
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    if data is not None:
        req.add_header("content-type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            raw = r.read()
            status, headers = r.status, dict(r.headers)
    except urllib.error.HTTPError as e:
        raw, status, headers = e.read(), e.code, dict(e.headers)
    except Exception as e:
        return 0, {}, str(e)
    try:
        return status, headers, json.loads(raw)
    except (json.JSONDecodeError, UnicodeDecodeError):
        return status, headers, raw


def one_pass(api, worker_id, task_types=None):
    """One lease → frame → ingest cycle. Returns how many tasks produced ticketed ingests.

    `task_types` confines this worker AT THE LEASE, which is the only place it can be confined.

    A lease is EXCLUSIVE — that is the property `validate_ingest_provenance.sh` asserts — so a worker
    left unfiltered takes every eligible task and starves anything else that leases. Filtering the
    RESULT instead does not help: the tasks are already leased by then and simply go unworked, which
    is worse, because they are held and idle. The first version of this filtered by camera after the
    fact and silently disarmed `validate_ai.sh`'s chain assertions.
    """
    req = {"worker_id": worker_id, "ttl_secs": 60}
    if task_types:
        req["task_types"] = task_types
    status, _, lease = call("POST", f"{api}/api/v1/ai/leases", req)
    if status != 200 or not isinstance(lease, dict):
        print(f"mock-analyzer: lease failed ({status}): {lease}", flush=True)
        return 0

    ticketed = 0
    for task in lease.get("tasks", []):
        # Verbatim — see the module docstring.
        frame_status, headers, _ = call("GET", f"{api}{task['frame_url']}")
        if frame_status != 200:
            print(
                f"mock-analyzer: frame pull for {task['id']} -> {frame_status}",
                flush=True,
            )
            continue
        ticket = headers.get("x-frame-ticket") or headers.get("X-Frame-Ticket")

        body = {
            "camera_id": task["camera_id"],
            "task_type": task.get("task_type", "detection"),
            # A fixed detection: the assertion downstream is about provenance, not perception.
            "detections": [
                {
                    "label": "person",
                    "confidence": 0.9,
                    "bbox": [0.35, 0.35, 0.2, 0.3],
                    "track_id": f"mock-{task['camera_id']}",
                }
            ],
        }
        if ticket:
            body["frame_ticket"] = ticket
        ing_status, _, ing = call("POST", f"{api}/api/v1/ai/events", body)
        ok = ing_status == 200 and isinstance(ing, dict) and ing.get("ticketed")
        ticketed += 1 if ok else 0
        print(
            f"mock-analyzer: {task['camera_id']} frame=200 "
            f"ticket={'yes' if ticket else 'NO'} ingest={ing_status} "
            f"ticketed={isinstance(ing, dict) and ing.get('ticketed')}",
            flush=True,
        )
    return ticketed


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("api", help="e.g. http://127.0.0.1:8000")
    ap.add_argument("--worker-id", default="mock-analyzer")
    ap.add_argument(
        "--task-types",
        default=None,
        help="comma-separated; lease only these. A lease is exclusive, so an unfiltered worker "
        "takes every task and starves anything else that leases.",
    )
    ap.add_argument("--interval", type=float, default=3.0)
    ap.add_argument("--passes", type=int, default=0, help="0 = run until killed")
    args = ap.parse_args()

    api = args.api.rstrip("/")
    types = [t.strip() for t in args.task_types.split(",")] if args.task_types else None
    n = 0
    while args.passes == 0 or n < args.passes:
        one_pass(api, args.worker_id, types)
        n += 1
        if args.passes and n >= args.passes:
            break
        time.sleep(args.interval)
    return 0


if __name__ == "__main__":
    sys.exit(main())
