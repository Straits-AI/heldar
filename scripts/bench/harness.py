#!/usr/bin/env python3
"""The Heldar qualification harness (#119).

`docs/sizing.md` gives formulas. Formulas are not limits. This measures a real box under a declared
workload and writes a machine-readable result that a capacity claim can cite.

Two modes:

  synthetic   boot MediaMTX + the core + N deterministic ffmpeg publishers from a scenario, so a run
              is reproducible on any host with the same hardware. Fault injection is available
              because the harness owns every process.
  field       measure a real box that already exists (HELDAR_URL / HELDAR_TOKEN). Nothing is booted,
              no camera is created, and FAULT INJECTION IS REFUSED — killing a process on someone's
              production recorder is not a benchmark, it is an outage.

What this deliberately does NOT do: report a number it did not observe. A metric the harness cannot
see is written as `{"unmeasured": <why>}`, and a threshold over an unmeasured metric FAILS. That is
the same rule the security posture uses — `unknown` is not a pass — and it exists because the whole
point of the issue is to stop capacity claims that were never measured.

Usage:
  harness.py list
  harness.py run <scenario> [--out DIR] [--duration-s N] [--hardware-class NAME]
  harness.py verify <result.json>       recompute the verdict from raw measurements

Exit codes (a contract a release script branches on):
  0  PASS — every declared threshold met
  1  FAIL — a threshold was missed, unmeasured, or the stack did not come up
  2  the run was REFUSED before measuring anything: unknown scenario, no release build, faults
     requested in field mode, or a fleet that did not fully register
  1  ...and `verify` also returns 1 for a run that is INVALID — one whose generator failed, which
     did not measure the product at all and is neither a pass nor a product failure
  4  `verify` was given a result from an unsupported schema
  5  `verify` was given a result whose recorded thresholds do not match their recorded hash
"""

import argparse
import hashlib
import json
import math
import os
import platform
import re
import shutil
import signal
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timedelta, timezone

SCHEMA = "heldar-benchmark/1"
HARNESS_VERSION = "1.0.0"

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
BENCH = os.path.join(ROOT, "scripts", "bench")

# Hashed AT IMPORT, not when the result is written. A long run outlives edits to this file, and a
# hash taken at the end would attest to code that did not produce the numbers beside it — a
# provenance field that is worse than no provenance field, because it looks authoritative.
HARNESS_SHA256 = hashlib.sha256(open(os.path.abspath(__file__), "rb").read()).hexdigest()


def now():
    return datetime.now(timezone.utc)


def rfc3339(t):
    return t.strftime("%Y-%m-%dT%H:%M:%SZ")


def sha256_of(obj):
    """Hash canonical JSON. Key order must not change the hash, or a reformat would look like a
    changed threshold and force a pointless re-qualification."""
    return hashlib.sha256(
        json.dumps(obj, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def run_out(cmd, default=""):
    try:
        return subprocess.run(
            cmd, capture_output=True, text=True, timeout=20
        ).stdout.strip()
    except Exception:
        return default


# --------------------------------------------------------------------------------------------
# HTTP. urllib rather than requests: the harness must run on an appliance with no pip index.
# --------------------------------------------------------------------------------------------


class Api:
    """Every call is timed and counted, so API latency and 5xx rate are a by-product of doing the
    work rather than a separate synthetic load nobody believes."""

    def __init__(self, base, token=None):
        self.base = base.rstrip("/")
        self.token = token
        self.durations = []
        self.statuses = []
        self.times = []
        self.paths = []

    def call(self, method, path, body=None, timeout=30):
        url = f"{self.base}{path}"
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(url, data=data, method=method)
        if data is not None:
            req.add_header("content-type", "application/json")
        if self.token:
            req.add_header("authorization", f"Bearer {self.token}")
        t0 = time.monotonic()
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                payload = r.read()
                status = r.status
        except urllib.error.HTTPError as e:
            payload, status = e.read(), e.code
        except Exception:
            # A connection failure is not a status code. Recorded as 0 so it cannot be mistaken for
            # a served response, and counted against availability rather than silently dropped.
            self.durations.append(time.monotonic() - t0)
            self.statuses.append(0)
            self.times.append(now())
            self.paths.append(path)
            return 0, None
        dt = time.monotonic() - t0
        self.durations.append(dt)
        self.statuses.append(status)
        self.times.append(now())
        self.paths.append(path)
        try:
            return status, json.loads(payload) if payload else None
        except (json.JSONDecodeError, UnicodeDecodeError):
            # Snapshots are JPEG and clips are MP4. Binary is a perfectly good answer here — the
            # probe grades the status code and the latency, not the pixels — so it is returned as
            # its byte length rather than crashing the run 50 minutes in.
            return status, {"bytes": len(payload)}

    def get(self, path, **kw):
        return self.call("GET", path, **kw)

    def post(self, path, body, **kw):
        return self.call("POST", path, body, **kw)

    def put(self, path, body, **kw):
        return self.call("PUT", path, body, **kw)

    def text(self, path, timeout=30):
        """Prometheus exposition is text, not JSON."""
        req = urllib.request.Request(f"{self.base}{path}")
        if self.token:
            req.add_header("authorization", f"Bearer {self.token}")
        t0 = time.monotonic()
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                out = r.read().decode()
                self.durations.append(time.monotonic() - t0)
                self.statuses.append(r.status)
                self.times.append(now())
                self.paths.append(path)
                return out
        except Exception:
            self.durations.append(time.monotonic() - t0)
            self.statuses.append(0)
            self.times.append(now())
            self.paths.append(path)
            return ""


def parse_prom(text):
    """{name: value} for bare series, and {name: {label_value: value}} for labelled ones.

    Only the `camera` label is kept, because it is the only one the exposition uses to fan out.
    """
    flat, keyed = {}, {}
    for line in text.splitlines():
        if not line or line.startswith("#"):
            continue
        m = re.match(r"^([a-z_]+)(\{[^}]*\})?\s+(-?[\d.eE+]+)$", line.strip())
        if not m:
            continue
        name, labels, value = m.group(1), m.group(2), float(m.group(3))
        if labels:
            cam = re.search(r'camera="([^"]*)"', labels)
            if cam:
                keyed.setdefault(name, {})[cam.group(1)] = value
                # The camera's textual state rides on heldar_camera_up as a second label; the
                # reconnect measurement needs it, and re-deriving it from the gauge alone would
                # conflate "offline" with "starting".
                st = re.search(r'state="([^"]*)"', labels)
                if st:
                    keyed.setdefault(name + "__state", {})[cam.group(1)] = st.group(1)
        else:
            flat[name] = value
    return flat, keyed


def pct(values, p):
    """Nearest-rank percentile: the smallest value at or below which at least p% of the samples lie.

    No numpy on an appliance, and for the sample sizes here the interpolation difference is far
    below the sampling resolution anyway. `math.ceil`, not `round`: `round` is half-to-even in
    Python, so the obvious spelling puts P50 of 1..10 at 6 and P95 of 1..100 at 96 — plausible
    numbers, one rank high, in a field nobody re-derives by hand.
    """
    if not values:
        return None
    s = sorted(values)
    rank = max(1, min(len(s), math.ceil(p / 100.0 * len(s))))
    return s[rank - 1]


# --------------------------------------------------------------------------------------------
# Provenance. A result without it cannot be compared to another run, which makes it an anecdote.
# --------------------------------------------------------------------------------------------


def provenance(hardware_class):
    cpu = platform.processor() or platform.machine()
    if sys.platform == "darwin":
        cpu = run_out(["sysctl", "-n", "machdep.cpu.brand_string"], cpu)
        mem = run_out(["sysctl", "-n", "hw.memsize"], "")
        cores = run_out(["sysctl", "-n", "hw.ncpu"], "")
    else:
        model = ""
        try:
            with open("/proc/cpuinfo") as f:
                for line in f:
                    if line.startswith("model name"):
                        model = line.split(":", 1)[1].strip()
                        break
        except OSError:
            pass
        cpu = model or cpu
        mem = ""
        try:
            with open("/proc/meminfo") as f:
                for line in f:
                    if line.startswith("MemTotal"):
                        mem = str(int(line.split()[1]) * 1024)
                        break
        except OSError:
            pass
        cores = str(os.cpu_count() or "")

    gpu = "none detected"
    if shutil.which("nvidia-smi"):
        gpu = run_out(
            ["nvidia-smi", "--query-gpu=name,memory.total", "--format=csv,noheader"],
            "nvidia-smi present, query failed",
        )
    elif sys.platform == "darwin":
        gpu = "Apple integrated (not used for AI in this run unless stated)"

    mtx = os.path.join(ROOT, "infra", "mediamtx", "mediamtx")
    return {
        "hardware_class": hardware_class,
        "host": socket.gethostname(),
        "os": f"{platform.system()} {platform.release()}",
        "kernel": platform.version(),
        "arch": platform.machine(),
        "cpu": cpu,
        "cpu_cores": cores,
        "ram_bytes": mem,
        "gpu": gpu,
        "python": platform.python_version(),
        "ffmpeg": (run_out(["ffmpeg", "-version"]).splitlines() or [""])[0],
        "mediamtx": (
            (run_out([mtx, "--version"]) or "unknown") if os.path.exists(mtx) else "absent"
        ),
        "git_sha": run_out(["git", "-C", ROOT, "rev-parse", "HEAD"], "unknown"),
        "git_dirty": bool(run_out(["git", "-C", ROOT, "status", "--porcelain"])),
        "harness_version": HARNESS_VERSION,
        # The harness's own bytes. `git_sha` does not pin this — a run made from a dirty tree, or
        # from a working copy edited between runs, has the same SHA and different behaviour. A
        # published result has to be traceable to the code that produced it, and a version string
        # someone forgets to bump is not that.
        "harness_sha256": HARNESS_SHA256,
    }


# --------------------------------------------------------------------------------------------
# The synthetic stack.
# --------------------------------------------------------------------------------------------


def core_binary():
    """The RELEASE core, always.

    A debug build of the recorder is several times slower than the one an appliance ships, so a
    capacity number measured against it is not conservative — it is wrong, and wrong in the
    direction that makes the product look bad while telling you nothing about the product. Refused
    rather than substituted.
    """
    path = os.path.join(ROOT, "target", "release", "heldar-core")
    if not os.path.exists(path):
        # Exit 2, not 1: this is a refusal before anything was measured, which is the documented
        # meaning of 2. `raise SystemExit("msg")` would exit 1 and quietly contradict the contract
        # at the top of this file that a release script branches on.
        print(
            "no release core at target/release/heldar-core — run: "
            "cargo build --release --workspace\n"
            "A benchmark against a debug build measures the build, not the product.",
            file=sys.stderr,
        )
        raise SystemExit(2)
    return path


def free_port(preferred):
    """The scenario's port if it is free, otherwise any free one.

    A benchmark that dies on `Address already in use` because an unrelated process holds a port has
    wasted however long it was going to run. The port number does not affect a single measurement,
    so there is nothing to preserve by insisting on it — but it IS recorded in the result, because
    "which port did this run use" is a question someone reading a log will have.
    """
    for candidate in (preferred, 0):
        with socket.socket() as sk:
            try:
                sk.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                sk.bind(("127.0.0.1", candidate))
                return sk.getsockname()[1]
            except OSError:
                continue
    raise RuntimeError("no free port")


class Stack:
    """MediaMTX + core + publishers, owned by this process so faults can be injected."""

    def __init__(self, scenario, data_dir, log_dir):
        self.sc = scenario
        self.data = data_dir
        self.logs = log_dir
        self.port = free_port(scenario.get("api_port", 8021))
        self.rtsp_port = free_port(scenario.get("rtsp_port", 8654))
        # MediaMTX listens on four ports, and the kernel is told about three of them separately.
        # Remap ALL of them: leaving one on its default means a benchmark quietly shares a listener
        # with a dev stack, and the run measures two systems.
        self.mtx_api_port = free_port(9997 + 1000)
        self.hls_port = free_port(8888 + 1000)
        self.webrtc_port = free_port(8889 + 1000)
        self.mtx = None
        self.core = None
        self.publishers = {}
        self.mtx_cfg = os.path.join(data_dir, "mediamtx.yml")
        # Every interval the harness ITSELF took a camera (or the whole fleet) off the air.
        # A gap the benchmark caused is not a gap the recorder is answerable for; without this the
        # fault-injection scenarios would fail their own coverage threshold by design, and the
        # obvious fix would be to loosen the threshold — which is the thing the issue warns against.
        self.outages = []  # [camera_id | "ALL", start, end|None]

    def _log(self, name):
        return open(os.path.join(self.logs, name), "ab")

    def start_mediamtx(self):
        src = os.path.join(ROOT, "infra", "mediamtx", "mediamtx.yml")
        cfg = open(src).read()
        # The shipped config pins the auth callback to :8000. The bench core runs on its own port so
        # it never collides with a dev core; without repointing this, MediaMTX asks a dead port and
        # denies every publish with a 401 — and the run measures a fleet that never streamed.
        # (Same trap documented in scripts/e2e_stack.sh.)
        cfg = cfg.replace(
            "http://127.0.0.1:8000/internal/mediamtx-auth",
            f"http://127.0.0.1:{self.port}/internal/mediamtx-auth",
        )
        for key, value in (
            ("rtspAddress", f":{self.rtsp_port}"),
            ("apiAddress", f"127.0.0.1:{self.mtx_api_port}"),
            ("hlsAddress", f":{self.hls_port}"),
            ("webrtcAddress", f":{self.webrtc_port}"),
        ):
            cfg, n = re.subn(rf"^{key}:.*$", f"{key}: {value}", cfg, flags=re.M)
            # A silently-unmatched key would leave the listener on its default port, which is the
            # collision this remapping exists to prevent. Loud, not tolerated.
            assert n == 1, f"{key} not found exactly once in mediamtx.yml (found {n})"
        open(self.mtx_cfg, "w").write(cfg)
        self.mtx = subprocess.Popen(
            [os.path.join(ROOT, "infra", "mediamtx", "mediamtx"), self.mtx_cfg],
            stdout=self._log("mediamtx.log"),
            stderr=subprocess.STDOUT,
        )
        time.sleep(2)

    def start_core(self):
        env = dict(os.environ)
        env.update(
            {
                "HELDAR_DATABASE_URL": f"sqlite://{self.data}/heldar.db",
                "HELDAR_DATA_DIR": self.data,
                "HELDAR_API_PORT": str(self.port),
                # Loopback: a benchmark has no business exposing an unauthenticated API to the LAN,
                # and binding 0.0.0.0 also trips the kernel's (correct) commissioning warning.
                "HELDAR_API_HOST": "127.0.0.1",
                "HELDAR_DEFAULT_SEGMENT_SECONDS": str(self.sc.get("segment_seconds", 10)),
                "HELDAR_INDEXER_INTERVAL_S": "3",
                "HELDAR_HEALTH_INTERVAL_S": "5",
                "HELDAR_MAX_RECORDINGS_GB": str(self.sc.get("max_recordings_gb", 5)),
                "HELDAR_LOG": "warn,heldar_core=info",
                "HELDAR_MEDIAMTX_API_URL": f"http://127.0.0.1:{self.mtx_api_port}",
                "HELDAR_MEDIAMTX_RTSP_BASE": f"rtsp://127.0.0.1:{self.rtsp_port}",
                "HELDAR_MEDIAMTX_HLS_BASE": f"http://127.0.0.1:{self.hls_port}",
                "HELDAR_MEDIAMTX_WEBRTC_BASE": f"http://127.0.0.1:{self.webrtc_port}",
            }
        )
        if self.sc.get("ai_profile", "off") != "off":
            env["HELDAR_AI_ENABLED"] = "true"
            env["HELDAR_DEFAULT_AI_FPS"] = str(self.sc.get("ai_fps", 2))
        self.core = subprocess.Popen(
            [core_binary()],
            env=env,
            stdout=self._log("core.log"),
            stderr=subprocess.STDOUT,
        )

    def wait_api(self, api, timeout_s=60):
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            status, _ = api.get("/healthz", timeout=3)
            if status == 200:
                return True
            time.sleep(1)
        return False

    def camera_ids(self):
        return [f"bench_{i:03d}" for i in range(1, self.sc["cameras"] + 1)]

    def start_publisher(self, cam):
        """One deterministic RTSP source per camera, through scripts/synth_camera.sh so the ffmpeg
        incantation lives in exactly one place. The script `exec`s ffmpeg, so this PID is ffmpeg's
        and killing it is a real camera disconnect."""
        p = self.sc
        self.close_outage(cam)
        self.publishers[cam] = subprocess.Popen(
            [
                os.path.join(ROOT, "scripts", "synth_camera.sh"),
                cam,
                p.get("resolution", "1280x720"),
                str(p.get("fps", 15)),
                p.get("codec", "h264"),
                str(p.get("bitrate_kbps", 2000)),
                str(p.get("gop", p.get("fps", 15) * 2)),
                f"rtsp://127.0.0.1:{self.rtsp_port}",
            ],
            stdout=self._log(f"pub_{cam}.log"),
            stderr=subprocess.STDOUT,
        )

    def open_outage(self, who, backdate_s=0):
        """`backdate_s` extends the outage backwards.

        Killing a process that is midway through writing a 10-second segment loses the footage it
        had already captured for that segment — the file is left truncated and the indexer rejects
        it. That loss belongs to the fault, so the fault's window has to start at the last COMPLETE
        segment rather than at the signal. It is also reported on its own as
        `footage_lost_per_restart_seconds`, because it is a real cost of a restart and averaging it
        into a coverage figure hides it.
        """
        self.outages.append([who, now() - timedelta(seconds=backdate_s), None])

    def close_outage(self, who):
        for row in reversed(self.outages):
            if row[0] == who and row[2] is None:
                row[2] = now()
                return

    def stop_publisher(self, cam):
        self.open_outage(cam)
        p = self.publishers.pop(cam, None)
        if p and p.poll() is None:
            p.terminate()
            try:
                p.wait(timeout=5)
            except subprocess.TimeoutExpired:
                p.kill()

    def respawn_all_publishers(self):
        """Bring every publisher back after the stream server it publishes into was restarted."""
        cams = list(self.publishers)
        for cam in cams:
            p = self.publishers.get(cam)
            if p and p.poll() is None:
                p.terminate()
                try:
                    p.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    p.kill()
            self.publishers.pop(cam, None)
            self.start_publisher(cam)
        return cams

    def supervise_publishers(self, deliberately_down):
        """Restart any publisher that died on its own, and COUNT it.

        A real camera does not stop existing because the host got busy. A publisher that has exited
        makes the fleet silently smaller for the rest of the run, and the resulting coverage hole
        reads as a recorder failure — the measurement then blames the product for the benchmark's
        own generator falling over.

        The count is deliberately surfaced rather than absorbed: repeated respawns mean the HOST
        cannot sustain the encode load, which invalidates the run instead of failing the product.
        """
        respawned = []
        for cam in list(self.publishers):
            p = self.publishers.get(cam)
            if cam not in deliberately_down and p is not None and p.poll() is not None:
                self.publishers.pop(cam, None)
                self.start_publisher(cam)
                respawned.append(cam)
        return respawned

    def stop(self):
        for cam in list(self.publishers):
            self.stop_publisher(cam)
        for proc in (self.core, self.mtx):
            if proc and proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    proc.kill()


# --------------------------------------------------------------------------------------------
# Verdict. A PURE function of measurements and thresholds, so `verify` can recompute it and catch a
# result whose `verdict` field was edited by hand.
# --------------------------------------------------------------------------------------------

OPS = {
    "<=": lambda a, b: a <= b,
    ">=": lambda a, b: a >= b,
    "<": lambda a, b: a < b,
    ">": lambda a, b: a > b,
    "==": lambda a, b: a == b,
}


def bars_hash(thresholds):
    """Hash the BARS ONLY — the metric/op/value triples — not the whole thresholds file.

    The capacity gate refuses a claim whose run was judged against different thresholds, which is
    what stops a bar being loosened to rescue a red run. Hashing the entire file would make a
    typo fix in a comment invalidate every published qualification, and a rule that fires on
    editorial changes is a rule people start working around.

    So: prose, versions and rationale can be edited freely; changing what a threshold ACTUALLY
    REQUIRES invalidates the claims that rested on it, which is the whole point.
    """
    return sha256_of(
        [
            {"metric": t["metric"], "op": t["op"], "value": t["value"]}
            for t in thresholds.get("thresholds", [])
        ]
    )


def validity(result):
    """Is this run a measurement at all?

    Distinct from PASS/FAIL on purpose. A run whose GENERATOR fell over did not measure the
    product, and reporting that as a product failure is as wrong as reporting it as a pass — the
    honest answer is "this run does not count, run it on a bigger host".

    Kept a pure function of the recorded result so `verify` and the capacity gate can recompute it,
    exactly like the verdict.
    """
    n = len(result.get("cameras") or [])
    respawns = len(result.get("publisher_respawns") or [])
    # More than one generator failure per camera, on average, is a host that cannot sustain the
    # encode load it was asked to produce. The streams then thin out over the run and the recorder
    # is blamed for a coverage hole that had nothing to record.
    if n and respawns > n:
        return {
            "status": "INVALID",
            "reason": f"the synthetic publishers had to be restarted {respawns} times across {n} "
            f"cameras — the host could not sustain the encode load, so this run measures the "
            f"generator rather than the recorder. Generate the streams on a separate machine or "
            f"reduce the camera count.",
        }
    declared = (result.get("scenario") or {}).get("duration_s")
    actual = result.get("duration_s")
    if declared and actual and actual < declared * 0.9:
        return {
            "status": "INVALID",
            "reason": f"the run covered {actual:.0f}s of a declared {declared}s and was cut short",
        }
    if not result.get("measurements"):
        return {"status": "INVALID", "reason": "no measurements were recorded"}
    return {"status": "VALID"}


def evaluate(measurements, thresholds):
    checks, verdict = [], "PASS"
    for t in thresholds["thresholds"]:
        name, op, limit = t["metric"], t["op"], t["value"]
        m = measurements.get(name)
        if m is None:
            status, observed = "MISSING", None
        elif "unmeasured" in m:
            # NOT a pass. A threshold nobody measured is a threshold nobody met, and reporting it
            # green is exactly the fiction this harness exists to prevent.
            status, observed = "UNMEASURED", m["unmeasured"]
        else:
            observed = m["value"]
            status = "PASS" if OPS[op](observed, limit) else "FAIL"
        if status != "PASS":
            verdict = "FAIL"
        checks.append(
            {
                "metric": name,
                "op": op,
                "limit": limit,
                "observed": observed,
                "status": status,
                "why": t.get("why", ""),
            }
        )
    if not checks:
        # An empty threshold set would otherwise report PASS having checked nothing.
        return "FAIL", [
            {
                "metric": "(none)",
                "status": "FAIL",
                "why": "no thresholds were declared; a run that checks nothing cannot qualify",
            }
        ]
    return verdict, checks


# --------------------------------------------------------------------------------------------
# The run.
# --------------------------------------------------------------------------------------------


def sample_process(pid):
    """CPU% and RSS for one pid, via ps — portable across darwin and linux without a dependency."""
    out = run_out(["ps", "-o", "%cpu=,rss=", "-p", str(pid)])
    try:
        cpu, rss = out.split()
        return float(cpu), int(rss) * 1024
    except Exception:
        return None, None


def probe_media(api, cams, results, window_from, limit=8, offset=0):
    """Exercise the operator-facing paths and time them. These are the ones a person notices.

    A BOUNDED, ROTATING sample rather than the whole fleet. One camera's round costs roughly
    live view + snapshot + clip — order ten seconds — so probing 32 cameras takes over five
    minutes, which is longer than that scenario's probe interval: the loop would spend the entire
    run probing and sample no metrics at all. Rotating by `offset` means the fleet is still covered
    over the run, and every probe row records which camera it hit.
    """
    if not cams:
        return
    window = [cams[(offset + i) % len(cams)] for i in range(min(limit, len(cams)))]
    for cam in window:
        t0 = time.monotonic()
        status, _ = api.get(f"/api/v1/cameras/{cam}/liveview", timeout=20)
        results["liveview"].append((status == 200, time.monotonic() - t0, cam, now()))

        t0 = time.monotonic()
        status, _ = api.get(f"/api/v1/cameras/{cam}/snapshot", timeout=30)
        results["snapshot"].append((status == 200, time.monotonic() - t0, cam, now()))

        to = now()
        frm = max(window_from, to - timedelta(seconds=30))
        if (to - frm).total_seconds() >= 5:
            t0 = time.monotonic()
            status, _ = api.post(
                f"/api/v1/cameras/{cam}/clip",
                {"from": rfc3339(frm), "to": rfc3339(to)},
                timeout=60,
            )
            # A clip over a window with no footage is a 404 by design, not a failure of the export
            # path. Counted separately so a young run cannot look like a broken one.
            results["clip"].append((status == 200, time.monotonic() - t0, status, cam, now()))


def unplayable_segments(data_dir, segment_seconds, restart_windows=None, limit=60):
    """ffprobe the segments THE INDEX CLAIMS EXIST.

    The obvious version of this walks the recordings directory, and it is wrong. A file on disk the
    indexer never accepted is litter — nothing points at it and no timeline promises it — so
    counting it reports a corruption the product does not have. The first run of this harness did
    exactly that: a SIGTERM to the core left a 28-byte MP4 stub, ffprobe refused it, and the harness
    called it a data-integrity failure. The indexer had already rejected the file.

    What matters is the other direction: a segment the timeline OFFERS an operator that will not
    decode when they ask for it. So the population is the segment table, and the count is how many
    of its rows do not survive ffprobe. A row whose file is missing entirely counts too — that is
    worse than unplayable, not better.
    """
    if not shutil.which("ffprobe"):
        return None, None, "ffprobe is not installed", 0, 0
    db = os.path.join(data_dir, "heldar.db")
    if not os.path.exists(db):
        return None, None, "no database at the expected path", 0, 0
    import sqlite3

    con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    try:
        every = con.execute("SELECT path, start_time FROM segments ORDER BY start_time").fetchall()
        indexed_total = len(every)
    finally:
        con.close()
    if not every:
        return None, None, "the index holds no segments", 0, 0

    # SPREAD ACROSS THE RUN, not the newest N. `ORDER BY start_time DESC LIMIT 60` covered roughly
    # the last minute of a 30-minute run and systematically excluded the segments around the
    # injected restart — the one event most likely to leave a bad file. A recorder that mis-muxed a
    # segment on every SIGTERM would have scored a clean zero.
    stride = max(1, len(every) // max(1, limit))
    sample = {row[0] for row in every[::stride]}

    # ...and every segment overlapping a restart window, unconditionally. That is where a truncated
    # file comes from, so it is not left to a stride to happen to land there.
    for w_from, w_to in restart_windows or []:
        lo, hi = rfc3339(w_from), rfc3339(w_to or w_from)
        sample |= {path for path, st in every if st and lo <= st <= hi}
    rows = sorted(sample)

    on_disk = sum(
        1
        for _, _, names in os.walk(os.path.join(data_dir, "recordings"))
        for n in names
        if n.endswith((".mp4", ".mkv", ".ts"))
    )
    bad = 0
    for rel in rows:
        f = rel if os.path.isabs(rel) else os.path.join(data_dir, rel)
        if not os.path.exists(f):
            bad += 1
            continue
        r = subprocess.run(
            ["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", f],
            capture_output=True,
            text=True,
        )
        if r.returncode != 0 or not r.stdout.strip():
            bad += 1
    return bad, max(0, on_disk - indexed_total), None, len(rows), indexed_total


def do_run(args):
    scenarios = json.load(open(os.path.join(BENCH, "scenarios.json")))
    if args.scenario not in scenarios:
        print(f"unknown scenario {args.scenario!r}; try `harness.py list`", file=sys.stderr)
        return 2
    sc = dict(scenarios[args.scenario])
    if args.duration_s:
        sc["duration_s"] = args.duration_s
    thresholds = json.load(open(os.path.join(BENCH, "thresholds.json")))

    run_id = f"{args.scenario}-{now().strftime('%Y%m%dT%H%M%SZ')}"
    out_dir = args.out or os.path.join(ROOT, "docs", "benchmarks", "results")
    os.makedirs(out_dir, exist_ok=True)

    mode = sc.get("mode", "synthetic")
    if mode == "field" and sc.get("faults"):
        print(
            "refusing: this scenario injects faults in `field` mode. Killing a process on a real "
            "recorder is an outage, not a benchmark.",
            file=sys.stderr,
        )
        return 2

    data_dir = os.path.join("/tmp", f"heldar-bench-{run_id}")
    log_dir = os.path.join(data_dir, "logs")
    os.makedirs(log_dir, exist_ok=True)

    stack = None
    keep_data_dir = True   # flipped to False only once a result has been written successfully
    started = now()
    faults_done = []
    restart_windows = []
    samples = []
    probe = {"liveview": [], "snapshot": [], "clip": []}
    reconnects = []          # seconds a camera spent not-recording after having recorded
    restart_recoveries = []  # one per core restart; the threshold is judged on the worst
    publisher_respawns = []  # generator failures — a validity signal, not a product measurement
    mediamtx_recoveries = []

    try:
        if mode == "synthetic":
            core_binary()  # fail before booting anything if the release build is absent
            stack = Stack(sc, data_dir, log_dir)
            api = Api(f"http://127.0.0.1:{stack.port}")
            stack.start_mediamtx()
            stack.start_core()
            if not stack.wait_api(api):
                rc = stack.core.poll()
                print(
                    f"the core did not come up (exit {rc})" if rc is not None
                    else "the core is running but never answered /healthz",
                    file=sys.stderr,
                )
                print(open(os.path.join(log_dir, "core.log")).read()[-2000:], file=sys.stderr)
                return 1
            cams = stack.camera_ids()
            # Publishers start AFTER the core: MediaMTX delegates publish authorization to the
            # kernel, so publishing earlier is denied 401 and every ffmpeg exits immediately.
            for cam in cams:
                stack.start_publisher(cam)
            time.sleep(3)
            registered = 0
            for cam in cams:
                st_code, _ = api.post(
                    "/api/v1/cameras",
                    {
                        "id": cam,
                        "name": f"Bench {cam}",
                        "vendor": "generic",
                        "main_stream_url": f"rtsp://127.0.0.1:{stack.rtsp_port}/{cam}",
                        "record_stream": "main",
                        "segment_seconds": sc.get("segment_seconds", 10),
                        "retention_hours": sc.get("retention_hours", 2),
                    },
                )
                registered += st_code in (200, 201)
                if sc.get("ai_profile", "off") != "off":
                    api.post(
                        f"/api/v1/cameras/{cam}/ai-tasks",
                        {
                            "task_type": sc.get("ai_profile"),
                            "fps": sc.get("ai_fps", 2),
                            "width": sc.get("ai_width", 480),
                            "enabled": True,
                            "config": {"threshold": 0.0008, "pixel_delta": 6},
                        },
                    )
            # THE FLEET HAS TO EXIST. A run against a short fleet measures a box with less to do
            # and reports green — no gaps, no failures, every threshold met.
            #
            # This check was written for synthetic mode and first landed, by a bad splice, inside
            # the FIELD branch — where `registered` is never assigned. It was therefore dead for the
            # mode it guards and an UnboundLocalError for the mode it was in, which made field mode
            # unrunnable. Asserting that an edit APPLIED is not the same as asserting it is
            # REACHABLE; `test_harness.py` now checks this one is.
            if registered != len(cams):
                print(
                    f"refusing: {registered} of {len(cams)} cameras were registered. A run against "
                    f"a short fleet measures a box with less to do and reports green — no gaps, no "
                    f"failures, every threshold met — which is the most dangerous result this "
                    f"harness could produce.",
                    file=sys.stderr,
                )
                return 2
        else:
            base = os.environ.get("HELDAR_URL", "http://127.0.0.1:8000")
            token = os.environ.get("HELDAR_TOKEN")
            api = Api(base, token)
            status, body = api.get("/api/v1/cameras")
            if status != 200:
                print(f"field mode: the box returned {status} for /api/v1/cameras", file=sys.stderr)
                return 1
            listing = body.get("cameras", body) if isinstance(body, dict) else body
            cams = [c["id"] for c in listing]
            if not cams:
                print("field mode: the box has no cameras this credential can see", file=sys.stderr)
                return 2

        # WARM-UP. A camera that has not yet connected is not a recorder that lost coverage, and
        # counting the first segment's worth of start-up as a gap would fail every scenario for a
        # reason that has nothing to do with capacity. Time-to-first-segment is reported separately,
        # because it IS interesting — just not as a coverage failure.
        warmup_s = sc.get("warmup_s", 120)
        first_segment_s = None
        if mode == "synthetic":
            t_warm = time.monotonic()
            while time.monotonic() - t_warm < warmup_s:
                _, keyed = parse_prom(api.text("/metrics"))
                # Per camera, not the fleet total: `segments_total >= len(cams)` is satisfied by one
                # camera writing two segments while another has written none, and the measurement
                # window then opens on a camera that has not started — charging the recorder for a
                # leading gap it never caused.
                written = keyed.get("heldar_camera_segments_written_total", {})
                if cams and all(written.get(c, 0) >= 1 for c in cams):
                    first_segment_s = time.monotonic() - t_warm
                    break
                time.sleep(2)

        window_from = now()
        duration = sc["duration_s"]
        interval = sc.get("sample_interval_s", 10)
        probe_every = sc.get("probe_interval_s", 60)
        faults = sorted(sc.get("faults", []), key=lambda f: f["at_s"])
        fi = 0
        last_probe = 0.0
        probe_round = 0
        down_since = {}
        deliberately_down = set()   # cameras the harness is holding off the air right now
        t_start = time.monotonic()

        while True:
            elapsed = time.monotonic() - t_start
            if elapsed >= duration:
                break

            text = api.text("/metrics")
            flat, keyed = parse_prom(text)
            cpu = rss = None
            if stack and stack.core and stack.core.poll() is None:
                cpu, rss = sample_process(stack.core.pid)
            samples.append(
                {
                    "t": round(elapsed, 1),
                    "cameras_recording": flat.get("heldar_cameras_recording"),
                    "segments_total": flat.get("heldar_segments_total"),
                    "recordings_bytes": flat.get("heldar_recordings_bytes"),
                    "disk_used_percent": flat.get("heldar_disk_used_percent"),
                    "detections_stored": flat.get("heldar_detections_stored"),
                    "core_cpu_percent": cpu,
                    "core_rss_bytes": rss,
                    "camera_up": keyed.get("heldar_camera_up", {}),
                }
            )

            if stack:
                # A publisher that died on its own is the generator failing, not the recorder. It is
                # brought back and counted; enough of them invalidate the run.
                for cam in stack.supervise_publishers(deliberately_down):
                    publisher_respawns.append({"camera": cam, "t": round(elapsed, 1)})

            # Reconnect time, at sampling resolution: the span from a camera leaving `recording` to
            # its return. Stated as a resolution-bounded measurement in the report rather than
            # presented as if it were instrumented inside the recorder, which it is not.
            #
            # THE CLOCK IS HELD while the harness is deliberately holding the publisher down. This
            # was the one place the injected-outage subtraction was missing, and it made the metric
            # measure the SCENARIO: a publisher stopped for 120 s produced a 120 s "reconnect", so
            # the 60 s bar could not be met by any product change. What is wanted is the span from
            # the camera being available again to the recorder having noticed.
            for cam, up in keyed.get("heldar_camera_up", {}).items():
                if cam in deliberately_down:
                    down_since[cam] = elapsed
                elif up < 1 and cam not in down_since:
                    down_since[cam] = elapsed
                elif up >= 1 and cam in down_since:
                    reconnects.append(elapsed - down_since.pop(cam))

            if elapsed - last_probe >= probe_every:
                last_probe = elapsed
                probe_media(api, cams, probe, window_from,
                            limit=sc.get("probe_cameras", 8), offset=probe_round)
                probe_round += sc.get("probe_cameras", 8)

            while fi < len(faults) and faults[fi]["at_s"] <= elapsed:
                f = faults[fi]
                fi += 1
                outcome = inject(f, stack, api, cams, restart_windows)
                # Keep the supervisor from "helpfully" restarting a publisher the scenario just
                # stopped, which would silently cancel the fault it was asked to inject.
                deliberately_down |= set(outcome.get("stopped", []))
                deliberately_down -= set(outcome.get("started", []))
                if f["kind"] == "core_restart" and outcome.get("recovery_s") is not None:
                    restart_recoveries.append(outcome["recovery_s"])
                if f["kind"] == "mediamtx_restart" and outcome.get("recovery_s") is not None:
                    mediamtx_recoveries.append(outcome["recovery_s"])
                faults_done.append({**f, "outcome": outcome})

            time.sleep(interval)

        window_to = now()
        probe_media(api, cams, probe, window_from,
                    limit=sc.get("probe_cameras", 8), offset=probe_round)

        # ---- derive measurements -------------------------------------------------------------
        m = {}
        hours = max((window_to - window_from).total_seconds() / 3600.0, 1e-9)

        # Close anything still open, so an outage that outlived the run is still subtracted
        # rather than silently counted as an unexplained gap.
        if stack:
            for row in stack.outages:
                if row[2] is None:
                    row[2] = window_to

        def in_injected_outage(cam, when):
            """True if the harness itself had this camera — or the whole fleet — down at `when`.

            A snapshot that fails while the benchmark is holding the camera's publisher down is not
            a snapshot failure, and a connection refused while the benchmark is restarting the core
            is not an availability incident. Counting them would make the fault-injection scenarios
            fail their own thresholds by construction, and the tempting fix would be to loosen the
            thresholds — the exact move the issue tells us not to make.
            """
            if not stack:
                return False
            grace = timedelta(seconds=sc.get("segment_seconds", 10))
            for who, start, end in stack.outages:
                if who in (cam, "ALL") and start <= when <= (end or window_to) + grace:
                    return True
            return False

        def deliberate_seconds(cam, since=None):
            """Seconds inside the measurement window that this camera was off the air because the
            harness put it there, plus a grace for the recorder to notice and resume.

            The grace is one segment length: a recorder cannot close a gap faster than it can write
            the segment that closes it, so charging it for that interval measures the segment size,
            not the recorder."""
            if not stack:
                return 0.0
            grace = timedelta(seconds=sc.get("segment_seconds", 10))
            total = 0.0
            for who, start, end in stack.outages:
                if who not in (cam, "ALL"):
                    continue
                lo = max(start, since or window_from)
                hi = min(end + grace, window_to)
                if hi > lo:
                    total += (hi - lo).total_seconds()
            return total

        # THE GAP WINDOW IS NOT THE RUN WINDOW. `/gaps` derives coverage from the segment index at
        # query time, and the retention sweeper has already deleted every row older than the
        # camera's retention. Asking for gaps over a 24-hour run with 6-hour retention returns ~18
        # hours of "gap" — the retention policy working exactly as configured, reported as lost
        # coverage. rc-24h-8cam and soak-7d-8cam could never have qualified.
        #
        # So the window is clamped to the retention horizon, less a margin for the sweeper running
        # on its own schedule, and the window actually used is recorded beside the number.
        retention_s = float(sc.get("retention_hours", 2)) * 3600.0
        gap_from = max(window_from, window_to - timedelta(seconds=retention_s * 0.8))
        gap_hours = (window_to - gap_from).total_seconds() / 3600.0
        gap_window = {"from": rfc3339(gap_from), "to": rfc3339(window_to),
                      "clamped_to_retention": gap_from > window_from}

        total_gap = 0.0
        total_unexplained = 0.0
        gap_ok = gap_hours * 3600 >= 120   # under two minutes of window, coverage means nothing
        for cam in cams:
            if not gap_ok:
                break
            status, body = api.get(
                f"/api/v1/cameras/{cam}/gaps"
                f"?from={rfc3339(gap_from)}&to={rfc3339(window_to)}"
            )
            if status != 200 or not isinstance(body, dict):
                gap_ok = False
                continue
            g = body.get("total_gap_seconds", 0.0)
            total_gap += g
            total_unexplained += max(0.0, g - deliberate_seconds(cam, since=gap_from))
        per_hour = len(cams) * max(gap_hours, 1e-9)
        gap_unmeasured = {
            "unmeasured": "the gaps endpoint did not answer for every camera"
            if gap_hours * 3600 >= 120
            else f"the window inside the retention horizon was only "
            f"{gap_hours * 3600:.0f}s — too short for coverage to mean anything"
        }
        m["recording_gap_seconds_per_camera_hour"] = (
            {"value": round(total_gap / per_hour, 3), "unit": "s/camera-hour", "n": len(cams),
             "window": gap_window}
            if gap_ok
            else gap_unmeasured
        )
        # THE ONE THE GATE USES. Raw coverage is what a dashboard shows; this is what the recorder
        # is answerable for, with the harness's own injected outages subtracted.
        m["unexplained_gap_seconds_per_camera_hour"] = (
            {"value": round(total_unexplained / per_hour, 3), "unit": "s/camera-hour",
             "n": len(cams), "window": gap_window}
            if gap_ok
            else gap_unmeasured
        )

        if stack:
            bad, orphans, why, probed, indexed = unplayable_segments(
                data_dir, sc.get("segment_seconds", 10), restart_windows
            )
            m["unplayable_segment_count"] = (
                # `n` is the number of segments actually ffprobed, and `indexed_total` how many
                # existed. A sampled count published with n=1 hid that it was sampled at all.
                {"value": bad, "unit": "segments", "n": probed, "indexed_total": indexed}
                if why is None
                else {"unmeasured": why}
            )
            # Diagnostic, deliberately NOT a threshold: a file the indexer rejected is litter, and
            # a hard restart leaves one behind. Worth seeing on a soak (it accumulates), not worth
            # failing a release for.
            m["unindexed_segment_files"] = (
                {"value": orphans, "unit": "files", "n": 1}
                if why is None
                else {"unmeasured": why}
            )
        else:
            for k in ("unplayable_segment_count", "unindexed_segment_files"):
                m[k] = {"unmeasured": "field mode has no filesystem access to the recordings"}

        m["recorder_reconnect_seconds_p50"] = (
            {"value": round(pct(reconnects, 50), 2), "unit": "s", "n": len(reconnects)}
            if reconnects
            else {"unmeasured": "no camera left the recording state during the run"}
        )
        m["recorder_reconnect_seconds_p95"] = (
            {"value": round(pct(reconnects, 95), 2), "unit": "s", "n": len(reconnects)}
            if reconnects
            else {"unmeasured": "no camera left the recording state during the run"}
        )

        for name, rows in (("liveview", probe["liveview"]), ("snapshot", probe["snapshot"])):
            graded = [(ok, d) for ok, d, cam, when in rows if not in_injected_outage(cam, when)]
            oks = [ok for ok, _ in graded]
            durs = [d for _, d in graded]
            excluded = len(rows) - len(graded)
            m[f"{name}_failure_rate"] = (
                {"value": round(1 - sum(oks) / len(oks), 4), "unit": "ratio", "n": len(oks),
                 "excluded_during_injected_outage": excluded}
                if oks
                else {"unmeasured": f"every {name} probe fell inside an injected outage"}
            )
            m[f"{name}_seconds_p95"] = (
                {"value": round(pct(durs, 95), 3), "unit": "s", "n": len(durs),
                 "excluded_during_injected_outage": excluded}
                if durs
                else {"unmeasured": f"every {name} probe fell inside an injected outage"}
            )

        # A 404 is "no footage in that window", which is a fact about the window, not a fault in
        # the export path. Excluded, like the injected outages, so a short run cannot look like a
        # broken exporter.
        graded = [
            (ok, d)
            for ok, d, st, cam, when in probe["clip"]
            if st != 404 and not in_injected_outage(cam, when)
        ]
        m["clip_success_rate"] = (
            {"value": round(sum(ok for ok, _ in graded) / len(graded), 4), "unit": "ratio",
             "n": len(graded)}
            if graded
            else {"unmeasured": "every clip window had no footage; nothing was graded"}
        )
        m["clip_seconds_p95"] = (
            {"value": round(pct([d for _, d in graded], 95), 3), "unit": "s", "n": len(graded)}
            if graded
            else {"unmeasured": "every clip window had no footage; nothing was graded"}
        )

        cpus = [s["core_cpu_percent"] for s in samples if s["core_cpu_percent"] is not None]
        rsss = [s["core_rss_bytes"] for s in samples if s["core_rss_bytes"] is not None]
        m["core_cpu_percent_mean"] = (
            {"value": round(sum(cpus) / len(cpus), 2), "unit": "percent-of-one-core", "n": len(cpus)}
            if cpus
            else {"unmeasured": "field mode does not have the core's pid"}
        )
        m["core_rss_bytes_max"] = (
            {"value": max(rsss), "unit": "bytes", "n": len(rsss)}
            if rsss
            else {"unmeasured": "field mode does not have the core's pid"}
        )
        disks = [s["disk_used_percent"] for s in samples if s["disk_used_percent"] is not None]
        m["disk_used_percent_max"] = (
            {"value": max(disks), "unit": "percent", "n": len(disks)}
            if disks
            else {"unmeasured": "the exposition carried no disk gauge"}
        )

        # `when >= window_from`: polling /healthz at a core that has not finished starting is the
        # harness waiting for its own stack, not an outage. Counting the setup against the product
        # is how a benchmark reports a fault it caused itself.
        #
        # The outage a call is excused by is the one for ITS camera, falling back to fleet-wide. A
        # request for a camera the harness has unplugged is expected to fail; a request for any
        # other camera is not excused by that camera being down.
        def outage_key(path):
            for c in cams:
                if f"/{c}/" in path or path.endswith(f"/{c}"):
                    return c
            return "ALL"

        # Media paths are excluded from the API aggregate: each already has its own threshold
        # (snapshot 10 s, live view 5 s, clip 30 s) because each does inherently slow work — a
        # snapshot waits for a keyframe. Folding them into one API percentile gated at 2 s makes the
        # threshold set contradict itself, failing a snapshot against 2 s and passing it against 10.
        # See the note in thresholds.json: this definition changed AFTER a run, which is why the
        # thresholds version moved and every result carries its hash.
        MEDIA = ("/snapshot", "/liveview", "/clip")
        kept = [
            (c, d, pth)
            for c, d, when, pth in zip(api.statuses, api.durations, api.times, api.paths)
            if when >= window_from
            and not in_injected_outage(outage_key(pth), when)
            and not any(pth.endswith(x) for x in MEDIA)
        ]
        # Two different exclusions, reported separately. Lumping them made the field read as
        # "calls dropped because of an injected outage" when most of them were setup traffic or
        # media paths that have their own thresholds.
        in_window = [
            (c, d, pth)
            for c, d, when, pth in zip(api.statuses, api.durations, api.times, api.paths)
            if when >= window_from and not any(pth.endswith(x) for x in MEDIA)
        ]
        excluded_outage = len(in_window) - len(kept)
        excluded_setup_or_media = len(api.statuses) - len(in_window)
        m["api_5xx_rate"] = {
            "value": round(sum(1 for c, _, _ in kept if c == 0 or c >= 500) / max(len(kept), 1), 4),
            "unit": "ratio",
            "n": len(kept),
            "excluded_during_injected_outage": excluded_outage,
            "excluded_as_setup_or_media": excluded_setup_or_media,
        }
        m["api_seconds_p95"] = {
            "value": round(pct([d for _, d, _ in kept], 95), 3),
            "unit": "s",
            "n": len(kept),
            "excluded_during_injected_outage": excluded_outage,
            "excluded_as_setup_or_media": excluded_setup_or_media,
        }

        byts = [s["recordings_bytes"] for s in samples if s["recordings_bytes"] is not None]
        reclaimed = sum(max(0.0, a - b) for a, b in zip(byts, byts[1:]))
        m["retention_bytes_reclaimed"] = (
            {"value": int(reclaimed), "unit": "bytes", "n": len(byts)}
            if byts
            else {"unmeasured": "the exposition carried no recordings_bytes gauge"}
        )

        m["time_to_first_segment_seconds"] = (
            {"value": round(first_segment_s, 2), "unit": "s", "n": len(cams)}
            if first_segment_s is not None
            else {
                "unmeasured": "field mode does not start the cameras"
                if mode == "field"
                else f"not every camera had written a segment within the {warmup_s}s warm-up"
            }
        )

        # What a restart COSTS in footage, as opposed to how long it took to come back. These are
        # different numbers and only one of them is usually reported.
        if restart_windows and all(w[1] for w in restart_windows):
            lost = []
            for w_from, w_to in restart_windows:
                for cam in cams:
                    st, body = api.get(
                        f"/api/v1/cameras/{cam}/gaps"
                        f"?from={rfc3339(w_from)}&to={rfc3339(w_to)}"
                    )
                    if st == 200 and isinstance(body, dict):
                        lost.append(body.get("total_gap_seconds", 0.0))
            m["footage_lost_per_restart_seconds"] = (
                {"value": round(sum(lost) / len(lost), 2), "unit": "s/camera/restart",
                 "n": len(lost)}
                if lost
                else {"unmeasured": "the gaps endpoint did not answer for the restart window"}
            )
        else:
            m["footage_lost_per_restart_seconds"] = {
                "unmeasured": "the scenario injected no completed core restart"
            }

        # The WORST restart, not the last. `rc-24h-8cam` restarts twice and a soak more; keeping
        # only the most recent would let a bad first restart disappear behind a good second one.
        # REPORTED, NOT GATED. This metric was added after watching a run, and thresholds.json is
        # explicit that a bar chosen with a result already in view is not a bar. It is a candidate
        # for the next thresholds version, to be set before the run that would be judged by it.
        m["mediamtx_recovery_seconds"] = (
            {"value": round(max(mediamtx_recoveries), 2), "unit": "s",
             "n": len(mediamtx_recoveries)}
            if mediamtx_recoveries
            else {"unmeasured": "the scenario injected no MediaMTX restart"}
        )

        m["restart_recovery_seconds"] = (
            {"value": round(max(restart_recoveries), 2), "unit": "s", "n": len(restart_recoveries)}
            if restart_recoveries
            else {"unmeasured": "the scenario injected no core restart"}
        )

        if sc.get("ai_profile", "off") != "off":
            dets = [s["detections_stored"] for s in samples if s["detections_stored"] is not None]
            if len(dets) >= 2:
                observed = (dets[-1] - dets[0]) / max(
                    (samples[-1]["t"] - samples[0]["t"]), 1e-9
                )
                # Detections are not frames: a sampler that runs at the requested rate and sees
                # nothing stores nothing. This is a floor on effective throughput, and is labelled
                # as such rather than presented as effective-FPS.
                m["ai_detections_per_second"] = {
                    "value": round(observed, 4),
                    "unit": "detections/s",
                    "n": len(dets),
                }
            m["ai_fps_effective_ratio"] = {
                "unmeasured": "the kernel exposes stored detections, not sampler frame rate; "
                "effective-vs-requested FPS needs a sampler-side counter that does not exist yet"
            }
        else:
            m["ai_fps_effective_ratio"] = {"unmeasured": "this scenario runs with AI off"}

        # Named by the issue, and honestly out of reach from outside the process.
        for name, why in (
            ("event_ingest_latency_seconds_p95",
             "no event producer is driven by this harness; the kernel exposes no ingest histogram"),
            ("sqlite_busy_rate",
             "the kernel exposes no busy counter; API 5xx is measured instead and is a superset"),
            ("retention_sweep_seconds",
             "the sweeper emits no duration metric; only bytes reclaimed is externally visible"),
            ("worker_lease_churn",
             "requires AI workers holding leases; not driven by this harness"),
            ("gpu_utilisation_percent", "no GPU is used by this profile"),
            ("disk_iops", "not portably measurable without a platform-specific dependency"),
            ("network_throughput_bytes",
             "not portably measurable without a platform-specific dependency"),
        ):
            m.setdefault(name, {"unmeasured": why})

        # WHICH calls. A FAIL that says "P95 is 3.9 s" and nothing else sends the next person to
        # re-run the benchmark to find out what was slow; these two lists mean they do not have to.
        def summarise(rows):
            by = {}
            for c, d, pth in rows:
                # Collapse ids so a fleet's worth of per-camera paths is one row.
                key = re.sub(r"/(bench_\d+|[0-9a-f-]{8,})(/|$)", r"/{id}\2", pth)
                e = by.setdefault(key, {"path": key, "n": 0, "max_s": 0.0, "bad": 0})
                e["n"] += 1
                e["max_s"] = max(e["max_s"], round(d, 3))
                if c == 0 or c >= 500:
                    e["bad"] += 1
            return sorted(by.values(), key=lambda e: (-e["bad"], -e["max_s"]))

        by_path = summarise(kept)
        verdict, checks = evaluate(m, thresholds)

        result = {
            "schema": SCHEMA,
            "run_id": run_id,
            "scenario_name": args.scenario,
            "started_at": rfc3339(started),
            "ended_at": rfc3339(now()),
            "duration_s": round(time.monotonic() - t_start, 1),
            "warmup_s": warmup_s,
            "scenario": sc,
            "scenario_sha256": sha256_of(sc),
            "thresholds": thresholds,
            # The exact file, for provenance...
            "thresholds_sha256": sha256_of(thresholds),
            # ...and the bars alone, which is what a capacity claim is checked against.
            "thresholds_bars_sha256": bars_hash(thresholds),
            "provenance": provenance(args.hardware_class or sc.get("hardware_class", "unstated")),
            "cameras": cams,
            "ports": ({"api": stack.port, "rtsp": stack.rtsp_port} if stack else None),
            "faults": faults_done,
            "injected_outages": [
                [who, rfc3339(a), rfc3339(b) if b else None]
                for who, a, b in (stack.outages if stack else [])
            ],
            "measurements": m,
            "publisher_respawns": publisher_respawns,
            "api_by_path": by_path,
            "probes": {
                kind: [
                    {
                        "camera": r[2] if kind != "clip" else r[3],
                        "ok": r[0],
                        "seconds": round(r[1], 3),
                        "at": rfc3339(r[-1]),
                        "status": (r[2] if kind == "clip" else None),
                        "excluded_during_injected_outage": in_injected_outage(
                            r[3] if kind == "clip" else r[2], r[-1]
                        ),
                    }
                    for r in rows
                ]
                for kind, rows in probe.items()
            },
            "checks": checks,
            "verdict": verdict,
            "samples": samples,
        }
        result["validity"] = validity(result)
        path = os.path.join(out_dir, f"{run_id}.json")
        with open(path, "w") as f:
            json.dump(result, f, indent=2, sort_keys=True)
        keep_data_dir = False
        v = result["validity"]
        if v["status"] != "VALID":
            # Loud and FIRST: a threshold verdict computed over a run that did not measure the
            # product is noise, and printing it above this line would invite someone to quote it.
            print(f"INVALID  {path}\n  {v['reason']}")
            return 1
        print(f"{verdict}  {path}")
        for c in checks:
            if c["status"] != "PASS":
                print(f"  {c['status']:10} {c['metric']} {c.get('op','')} {c.get('limit','')} "
                      f"— observed {c['observed']!r}")
        return 0 if verdict == "PASS" else 1
    finally:
        if stack:
            stack.stop()
        # Keep the tree on a failed or refused run — the logs are the only way to find out why.
        # Remove it on success, or a soak leaves tens of gigabytes of synthetic footage in /tmp.
        if stack and os.path.isdir(data_dir) and keep_data_dir is False:
            shutil.rmtree(data_dir, ignore_errors=True)


RECOVERY_TIMEOUT_NOTE = (
    "not every camera had written a new segment within 300 s; recovery is reported as unmeasured "
    "rather than as 300 s, because the deadline is the harness's number and not the box's"
)


def wait_for_every_camera_to_write(api, cams, before_written, t0, deadline_s=300):
    """Seconds until EVERY camera has written a new segment, or None if it did not happen.

    Recovery is not "the port answers", and not "the status table says recording" either:
    camera_status is PERSISTED, so straight after a restart it holds the pre-restart state and
    `cameras_recording` reads full while nothing is being recorded. An early version measured 1.0 s
    for a restart that had plainly not finished.

    PER CAMERA, via `heldar_camera_segments_written_total`. The fleet-wide `heldar_segments_total`
    is wrong twice over: it is `COUNT(*) FROM segments`, so one camera writing N segments satisfies
    a target of N while the other cameras stay dead — and it is a GAUGE the retention sweeper drives
    back down, so at retention steady state the target is never crossed however healthy the
    recovery.

    On timeout this returns None rather than the deadline. Reporting 300 s would publish the
    harness's own patience as though it were the box's recovery time, and would fail a 120 s bar for
    a box that had recovered in five seconds.
    """
    deadline = time.monotonic() + deadline_s
    while time.monotonic() < deadline:
        _, keyed = parse_prom(api.text("/metrics"))
        w = keyed.get("heldar_camera_segments_written_total", {})
        if cams and all(w.get(c, 0) > before_written.get(c, 0) for c in cams):
            return time.monotonic() - t0
        time.sleep(2)
    return None


def inject(f, stack, api, cams, restart_windows):
    """Faults the harness can cause honestly. Anything it cannot is refused rather than faked."""
    kind = f["kind"]
    if kind in ("publisher_stop", "publisher_start", "mediamtx_restart", "core_restart") and not stack:
        return {"skipped": "field mode owns no processes"}

    if kind == "publisher_stop":
        for cam in f.get("cameras") or cams[: f.get("count", 1)]:
            stack.stop_publisher(cam)
        return {"stopped": f.get("cameras") or cams[: f.get("count", 1)]}

    if kind == "publisher_start":
        targets = f.get("cameras") or cams[: f.get("count", 1)]
        for cam in targets:
            stack.start_publisher(cam)
        return {"started": targets}

    if kind == "mediamtx_restart":
        # TOPOLOGY. In production the recorder pulls from the CAMERA directly and MediaMTX serves
        # live view, so restarting MediaMTX should cost live view and not recording. In this
        # synthetic stack MediaMTX is also the camera's RTSP server, so restarting it kills every
        # publisher with a broken pipe — and ffmpeg does not come back on its own.
        #
        # Without restarting them, the fault models something production cannot do: every camera
        # ceasing to exist, permanently. The first qualification runs did exactly that and reported
        # 1459 s/camera-hour of "unexplained gap", a 302 s restart recovery and a 63% snapshot
        # failure rate — five threshold failures that were one harness bug wearing a product's
        # clothes. A synthetic camera has to behave like a camera: it has to come back.
        t0 = time.monotonic()
        _, keyed = parse_prom(api.text("/metrics"))
        before_written = keyed.get("heldar_camera_segments_written_total", {})
        stack.open_outage("ALL", backdate_s=stack.sc.get("segment_seconds", 10))
        stack.mtx.terminate()
        try:
            stack.mtx.wait(timeout=10)
        except subprocess.TimeoutExpired:
            stack.mtx.kill()
        stack.start_mediamtx()
        respawned = stack.respawn_all_publishers()
        # The fault is not over when the process is back — it is over when footage is being written
        # again. Closing the outage at `start_mediamtx()` charged the recorder for its own
        # reconnect, which is how a 14-second recovery turned into 62 s/camera-hour of
        # "unexplained" gap. Same signal as the core restart: one new segment per camera cannot
        # happen unless every recorder is writing.
        recovered = wait_for_every_camera_to_write(api, cams, before_written, t0)
        stack.close_outage("ALL")
        return {
            "restarted": "mediamtx",
            "publishers_restarted": respawned,
            "recovery_s": recovered,
            **({} if recovered is not None else {"note": RECOVERY_TIMEOUT_NOTE}),
        }

    if kind == "core_restart":
        t0 = time.monotonic()
        _, keyed = parse_prom(api.text("/metrics"))
        before_written = keyed.get("heldar_camera_segments_written_total", {})
        stack.open_outage("ALL", backdate_s=stack.sc.get("segment_seconds", 10))
        restart_windows.append([now() - timedelta(seconds=stack.sc.get("segment_seconds", 10)), None])
        stack.core.send_signal(signal.SIGTERM)
        try:
            stack.core.wait(timeout=20)
        except subprocess.TimeoutExpired:
            stack.core.kill()
        stack.start_core()
        if not stack.wait_api(api, timeout_s=90):
            # Close what was opened. Leaving the outage and the restart window open would have the
            # rest of the run silently attributed to this fault, hiding every later failure.
            stack.close_outage("ALL")
            restart_windows[-1][1] = now()
            return {"restarted": "core", "recovery_s": None, "note": "the API did not return"}
        recovered = wait_for_every_camera_to_write(api, cams, before_written, t0)
        stack.close_outage("ALL")
        restart_windows[-1][1] = now()
        return {
            "restarted": "core",
            "recovery_s": recovered,
            **({} if recovered is not None else {"note": RECOVERY_TIMEOUT_NOTE}),
        }

    if kind == "disk_pressure":
        # Shrink the recordings cap so the sweeper must evict, rather than filling the host's disk
        # with ballast. Same code path, no way to damage the machine running the benchmark.
        status, _ = api.put(
            "/api/v1/system/retention", {"max_recordings_gb": f.get("max_recordings_gb", 0.05)}
        )
        return {"retention_cap_gb": f.get("max_recordings_gb", 0.05), "status": status}

    return {"skipped": f"unimplemented fault kind {kind!r}"}


def do_verify(args):
    """Recompute the verdict from the recorded measurements.

    The result file's own `verdict` is not trusted: a text file is editable, and a benchmark whose
    conclusion can be edited is a press release.
    """
    r = json.load(open(args.result))
    if r.get("schema") != SCHEMA:
        print(f"unsupported schema {r.get('schema')!r}", file=sys.stderr)
        return 4
    verdict, checks = evaluate(r["measurements"], r["thresholds"])
    if sha256_of(r["thresholds"]) != r.get("thresholds_sha256"):
        print("MALFORMED: the recorded thresholds do not match their recorded hash", file=sys.stderr)
        return 5
    v = validity(r)
    if v["status"] != "VALID":
        print(f"INVALID  {r['run_id']}\n  {v['reason']}", file=sys.stderr)
        return 1
    if verdict != r.get("verdict"):
        print(
            f"MISMATCH: the file claims {r.get('verdict')!r}; the measurements say {verdict!r}",
            file=sys.stderr,
        )
        for c in checks:
            if c["status"] != "PASS":
                print(f"  {c['status']:10} {c['metric']} — observed {c['observed']!r}",
                      file=sys.stderr)
        return 1
    print(f"{verdict}  {r['run_id']}  (recomputed from measurements)")
    return 0 if verdict == "PASS" else 1


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("list")
    r = sub.add_parser("run")
    r.add_argument("scenario")
    r.add_argument("--out")
    r.add_argument("--duration-s", type=int)
    r.add_argument("--hardware-class")
    v = sub.add_parser("verify")
    v.add_argument("result")
    args = ap.parse_args()

    if args.cmd == "list":
        for name, sc in json.load(open(os.path.join(BENCH, "scenarios.json"))).items():
            print(f"{name:24} {sc.get('description', '')}")
        return 0
    if args.cmd == "run":
        return do_run(args)
    return do_verify(args)


if __name__ == "__main__":
    sys.exit(main())
