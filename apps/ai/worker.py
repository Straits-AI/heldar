#!/usr/bin/env python3
"""VisionOps reference AI worker (Stage 2).

This is the canonical, dependency-light implementation of the VisionOps AI
worker contract. It proves and documents how a perception worker talks to
VisionOps Core so that Stage 3 can drop in a real model (e.g. YOLO) by
implementing a single `Analyzer` subclass — nothing else has to change.

The contract (served by apps/core, see src/routes/ai.rs)
-------------------------------------------------------
1. Discover work:
       GET  {API}/api/v1/ai/tasks
   -> [{ id, camera_id, task_type, stream_profile, fps, width,
         config, frame_url }]

2. Pull the latest sampled frame for a task (JPEG bytes):
       GET  {API}{frame_url}     (frame_url is "/api/v1/cameras/{cam}/frame")
   Response headers of interest:
       x-frame-captured-at  RFC3339 timestamp of the frame
       x-frame-age-ms       age in milliseconds
   A 404 means "no frame sampled yet" — not an error, just skip the cycle.

3. Post results back:
       POST {API}/api/v1/ai/events
       {
         "camera_id":  "...",
         "task_type":  "...",
         "timestamp":  "<RFC3339>",
         "detections": [{ "label", "confidence", "bbox":[x,y,w,h],
                          "track_id", "attributes" }],
         "event":      { "event_type", "severity", "payload" }   # optional
       }
   `bbox` is [x, y, w, h] normalized to 0..1.

Design
------
* One supervisor thread polls /ai/tasks every `poll_interval` seconds and
  reconciles a set of per-task worker threads (start new, stop removed,
  restart changed).
* Each task thread runs its own loop at the task's `fps`: pull frame ->
  run its `Analyzer` -> POST results.
* HTTP calls retry with capped exponential backoff + jitter; 4xx (client)
  errors are not retried.
* SIGINT/SIGTERM trigger a graceful, prompt shutdown (sleeps are
  interruptible).
* Structured logging (text or JSON) with per-task camera/task context.

The placeholder analyzer for unimplemented task types NEVER fabricates
detections — it only exercises the frame-pull path and logs that a real
model must be wired in.
"""

from __future__ import annotations

import argparse
import io
import json
import logging
import os
import random
import signal
import sys
import threading
import time
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Any, Dict, List, Optional

import numpy as np
import requests
from PIL import Image

# Global stop flag, set by signal handlers; watched by every loop/sleep.
SHUTDOWN = threading.Event()

log = logging.getLogger("worker")


# --------------------------------------------------------------------------- #
# Configuration
# --------------------------------------------------------------------------- #
@dataclass(frozen=True)
class Settings:
    api: str
    poll_interval: float
    http_timeout: float
    http_max_retries: int
    backoff_base: float
    backoff_cap: float
    log_level: str
    log_format: str


def _env(key: str, default: str) -> str:
    return os.environ.get(key, default)


def parse_settings(argv: Optional[List[str]] = None) -> Settings:
    """Build settings from env defaults overridden by CLI flags."""
    parser = argparse.ArgumentParser(
        prog="worker",
        description="VisionOps reference AI worker (Stage 2).",
    )
    parser.add_argument(
        "--api",
        default=_env("VISIONOPS_API", "http://localhost:8000"),
        help="VisionOps Core base URL (env VISIONOPS_API).",
    )
    parser.add_argument(
        "--poll-interval",
        type=float,
        default=float(_env("VISIONOPS_AI_POLL_INTERVAL", "10")),
        help="Seconds between /ai/tasks re-polls (env VISIONOPS_AI_POLL_INTERVAL).",
    )
    parser.add_argument(
        "--http-timeout",
        type=float,
        default=float(_env("VISIONOPS_HTTP_TIMEOUT", "10")),
        help="Per-request HTTP timeout in seconds (env VISIONOPS_HTTP_TIMEOUT).",
    )
    parser.add_argument(
        "--http-max-retries",
        type=int,
        default=int(_env("VISIONOPS_HTTP_MAX_RETRIES", "5")),
        help="Max retries for transient HTTP failures (env VISIONOPS_HTTP_MAX_RETRIES).",
    )
    parser.add_argument(
        "--backoff-base",
        type=float,
        default=float(_env("VISIONOPS_HTTP_BACKOFF_BASE", "0.5")),
        help="Initial backoff in seconds (env VISIONOPS_HTTP_BACKOFF_BASE).",
    )
    parser.add_argument(
        "--backoff-cap",
        type=float,
        default=float(_env("VISIONOPS_HTTP_BACKOFF_CAP", "15")),
        help="Max backoff in seconds (env VISIONOPS_HTTP_BACKOFF_CAP).",
    )
    parser.add_argument(
        "--log-level",
        default=_env("VISIONOPS_LOG_LEVEL", "INFO"),
        help="Logging level: DEBUG/INFO/WARNING/ERROR (env VISIONOPS_LOG_LEVEL).",
    )
    parser.add_argument(
        "--log-format",
        choices=("text", "json"),
        default=_env("VISIONOPS_LOG_FORMAT", "text"),
        help="Log output format (env VISIONOPS_LOG_FORMAT).",
    )
    ns = parser.parse_args(argv)
    return Settings(
        api=ns.api.rstrip("/"),
        poll_interval=max(1.0, ns.poll_interval),
        http_timeout=ns.http_timeout,
        http_max_retries=max(0, ns.http_max_retries),
        backoff_base=ns.backoff_base,
        backoff_cap=ns.backoff_cap,
        log_level=ns.log_level.upper(),
        log_format=ns.log_format,
    )


# --------------------------------------------------------------------------- #
# Logging
# --------------------------------------------------------------------------- #
_CONTEXT_FIELDS = ("camera_id", "task_id", "task_type")


class _ContextFilter(logging.Filter):
    """Ensure context fields always exist so format strings never KeyError."""

    def filter(self, record: logging.LogRecord) -> bool:
        for f in _CONTEXT_FIELDS:
            if not hasattr(record, f):
                setattr(record, f, "-")
        return True


class _JsonFormatter(logging.Formatter):
    def format(self, record: logging.LogRecord) -> str:
        payload: Dict[str, Any] = {
            "ts": datetime.fromtimestamp(record.created, timezone.utc).isoformat(),
            "level": record.levelname,
            "logger": record.name,
            "msg": record.getMessage(),
        }
        for f in _CONTEXT_FIELDS:
            val = getattr(record, f, "-")
            if val and val != "-":
                payload[f] = val
        if record.exc_info:
            payload["exc"] = self.formatException(record.exc_info)
        return json.dumps(payload, default=str)


def setup_logging(level: str, fmt: str) -> None:
    handler = logging.StreamHandler(sys.stderr)
    handler.addFilter(_ContextFilter())
    if fmt == "json":
        handler.setFormatter(_JsonFormatter())
    else:
        handler.setFormatter(
            logging.Formatter(
                "%(asctime)s %(levelname)-5s [cam=%(camera_id)s task=%(task_id)s] "
                "%(name)s: %(message)s",
                datefmt="%Y-%m-%dT%H:%M:%S%z",
            )
        )
    root = logging.getLogger()
    root.handlers[:] = [handler]
    root.setLevel(getattr(logging, level, logging.INFO))


def task_logger(task: "Task") -> logging.LoggerAdapter:
    """A logger that injects this task's camera/task context into every record."""
    return logging.LoggerAdapter(
        logging.getLogger("worker.task"),
        {"camera_id": task.camera_id, "task_id": task.id, "task_type": task.task_type},
    )


# --------------------------------------------------------------------------- #
# Domain types
# --------------------------------------------------------------------------- #
@dataclass(frozen=True)
class Task:
    """A unit of perception work as advertised by /ai/tasks."""

    id: str
    camera_id: str
    task_type: str
    stream_profile: str
    fps: float
    width: int
    config: Dict[str, Any]
    frame_url: str

    @classmethod
    def from_json(cls, d: Dict[str, Any]) -> "Task":
        cfg = d.get("config")
        return cls(
            id=str(d["id"]),
            camera_id=str(d["camera_id"]),
            task_type=str(d["task_type"]),
            stream_profile=str(d.get("stream_profile", "sub")),
            fps=float(d.get("fps", 5.0)),
            width=int(d.get("width", 1280)),
            config=cfg if isinstance(cfg, dict) else {},
            frame_url=str(d["frame_url"]),
        )

    def signature(self) -> tuple:
        """Identity of behavior — if this changes, the task thread is restarted."""
        return (
            self.task_type,
            self.stream_profile,
            round(self.fps, 4),
            self.width,
            self.frame_url,
            json.dumps(self.config, sort_keys=True),
        )

    @property
    def period(self) -> float:
        return 1.0 / max(self.fps, 0.1)


@dataclass
class FrameContext:
    """One frame pulled for a task, plus lazy decode helpers."""

    task: Task
    raw: bytes
    captured_at: Optional[str]
    age_ms: Optional[int]

    def image(self) -> Image.Image:
        return Image.open(io.BytesIO(self.raw))

    def gray_array(self, width: Optional[int] = None) -> np.ndarray:
        """Decode to a single-channel uint8 array, optionally downscaled to `width`."""
        img = self.image().convert("L")
        ow, oh = img.size
        if width and 0 < width < ow:
            new_h = max(1, round(oh * width / ow))
            img = img.resize((width, new_h), Image.BILINEAR)
        return np.asarray(img, dtype=np.uint8)


@dataclass
class Detection:
    label: str
    confidence: float
    bbox: Optional[List[float]] = None  # [x, y, w, h] normalized 0..1
    track_id: Optional[str] = None
    attributes: Optional[Dict[str, Any]] = None

    def to_json(self) -> Dict[str, Any]:
        out: Dict[str, Any] = {"label": self.label, "confidence": float(self.confidence)}
        if self.bbox is not None:
            out["bbox"] = [float(v) for v in self.bbox]
        if self.track_id is not None:
            out["track_id"] = self.track_id
        if self.attributes:
            out["attributes"] = self.attributes
        return out


@dataclass
class Event:
    event_type: str
    severity: str = "info"
    payload: Optional[Dict[str, Any]] = None

    def to_json(self) -> Dict[str, Any]:
        out: Dict[str, Any] = {"event_type": self.event_type, "severity": self.severity}
        if self.payload:
            out["payload"] = self.payload
        return out


@dataclass
class AnalysisResult:
    detections: List[Detection] = field(default_factory=list)
    event: Optional[Event] = None

    @property
    def is_empty(self) -> bool:
        return not self.detections and self.event is None


# --------------------------------------------------------------------------- #
# Analyzer interface  ——  THIS is the Stage 3 extension point
# --------------------------------------------------------------------------- #
class Analyzer(ABC):
    """Turns a frame into an :class:`AnalysisResult`.

    Stage 3 plugs in a real model by subclassing this and registering it:

        class YoloAnalyzer(Analyzer):
            def __init__(self, config, log):
                super().__init__(config, log)
                self.model = ultralytics.YOLO(config.get("weights", "yolov8n.pt"))
                self.conf = float(config.get("threshold", 0.25))

            def analyze(self, frame: FrameContext) -> AnalysisResult:
                results = self.model(frame.image(), conf=self.conf, verbose=False)
                dets = [Detection(label=..., confidence=..., bbox=[x,y,w,h])
                        for r in results for ...]
                return AnalysisResult(detections=dets)

        register("detection", YoloAnalyzer)

    Contract:
      * One Analyzer instance is created per task thread (so per-camera state
        such as a previous frame can live on the instance).
      * `analyze` is called on the task's cadence and must be reasonably fast;
        it must NEVER fabricate detections.
    """

    #: Human-readable name, mainly for logs.
    name: str = "analyzer"

    def __init__(self, config: Dict[str, Any], log: logging.LoggerAdapter):
        self.config = config or {}
        self.log = log

    @abstractmethod
    def analyze(self, frame: FrameContext) -> AnalysisResult:
        raise NotImplementedError


class MotionAnalyzer(Analyzer):
    """Frame-differencing motion detector — no model, no GPU.

    Downscales to grayscale, takes the absolute difference against the
    previous frame, and flags a detection when the fraction of changed
    pixels exceeds `config.threshold` (default 0.02). The detection's bbox
    is the tight box around the changed region, normalized to 0..1.
    """

    name = "motion"

    def __init__(self, config: Dict[str, Any], log: logging.LoggerAdapter):
        super().__init__(config, log)
        self.threshold = float(self.config.get("threshold", 0.02))
        self.pixel_delta = int(self.config.get("pixel_delta", 25))
        self.scale_width = int(self.config.get("scale_width", 320))
        self._prev: Dict[str, np.ndarray] = {}

    def analyze(self, frame: FrameContext) -> AnalysisResult:
        cam = frame.task.camera_id
        cur = frame.gray_array(self.scale_width)
        prev = self._prev.get(cam)
        self._prev[cam] = cur

        # Need a baseline (or matching dimensions) before we can compare.
        if prev is None or prev.shape != cur.shape:
            return AnalysisResult()

        diff = np.abs(cur.astype(np.int16) - prev.astype(np.int16))
        mask = diff > self.pixel_delta
        changed = float(mask.mean())  # already in 0..1

        if changed < self.threshold:
            return AnalysisResult()

        rows = np.any(mask, axis=1)
        cols = np.any(mask, axis=0)
        h, w = mask.shape
        ys = np.where(rows)[0]
        xs = np.where(cols)[0]
        ymin, ymax = int(ys[0]), int(ys[-1])
        xmin, xmax = int(xs[0]), int(xs[-1])
        bbox = [
            round(xmin / w, 4),
            round(ymin / h, 4),
            round((xmax - xmin + 1) / w, 4),
            round((ymax - ymin + 1) / h, 4),
        ]
        confidence = round(min(changed, 1.0), 4)

        detection = Detection(
            label="motion",
            confidence=confidence,
            bbox=bbox,
            attributes={"changed_fraction": confidence, "pixel_delta": self.pixel_delta},
        )
        event = Event(
            event_type="motion",
            severity="info",
            payload={"changed_fraction": confidence, "bbox": bbox},
        )
        self.log.debug("motion changed_fraction=%.4f bbox=%s", changed, bbox)
        return AnalysisResult(detections=[detection], event=event)


class PlaceholderAnalyzer(Analyzer):
    """Safe stand-in for task types without a real model wired in yet.

    It pulls and decodes the frame (so the full frame-pull/heartbeat path is
    exercised) but emits NO detections — we never fabricate results. It logs,
    rate-limited, that a real model must be registered for this task type.
    """

    name = "placeholder"

    def __init__(self, task_type: str, config: Dict[str, Any], log: logging.LoggerAdapter):
        super().__init__(config, log)
        self.task_type = task_type
        self._log_interval = float(self.config.get("log_interval_s", 60))
        self._last_log = 0.0

    def analyze(self, frame: FrameContext) -> AnalysisResult:
        size = None
        try:
            size = frame.image().size  # validate the pipeline end-to-end
        except Exception as exc:  # noqa: BLE001 - report any decode issue, keep running
            self.log.warning("frame decode failed: %s", exc)

        now = time.monotonic()
        if now - self._last_log >= self._log_interval:
            self._last_log = now
            self.log.warning(
                "no real analyzer for task_type=%r — Stage 3 must register a model. "
                "Frame pulled (size=%s); emitting NO detections (never fabricate).",
                self.task_type,
                size,
            )
        return AnalysisResult()


# COCO class groups used for optional alert events. These are the default class
# names emitted by the bundled yolov8n weights.
_PERSON_CLASSES = frozenset({"person"})
_VEHICLE_CLASSES = frozenset({"bicycle", "car", "motorcycle", "bus", "truck", "train"})


class YoloAnalyzer(Analyzer):
    """Real object detector + tracker: Ultralytics YOLOv8 (nano) + ByteTrack.

    This is the Stage 3 model that replaces the placeholder for ``detection``
    (and ``yolo``) tasks. It loads ``yolov8n.pt`` once per task thread and, on
    every frame, calls ``model.track(..., persist=True, tracker="bytetrack.yaml")``
    so each box carries a stable ByteTrack ``track_id`` across frames.

    A model instance is intentionally created *per task thread* (not shared
    process-wide): ByteTrack keeps its tracker state on the model/predictor, so
    one model per camera keeps each camera's track ids independent.

    Per-task ``config`` keys (all optional):
      * ``weights``      — weights file/name (default ``yolov8n.pt``; keep nano
                           for speed).
      * ``threshold``    — minimum confidence to keep a box (default ``0.25``).
      * ``classes``      — allowlist of class names and/or COCO indices; when
                           set, only these classes are detected (filtered at
                           inference for speed).
      * ``imgsz``        — inference image size (default model native).
      * ``device``       — force a device (e.g. ``"cpu"``, ``0``); default auto:
                           GPU if ``torch.cuda.is_available()`` else CPU.
      * ``emit_events``  — emit an alert event when person/vehicle classes
                           appear (default ``True``).
      * ``alert_classes``— class names that trigger the alert event
                           (default: person + common vehicle classes).
    """

    name = "yolo"

    def __init__(self, config: Dict[str, Any], log: logging.LoggerAdapter):
        super().__init__(config, log)
        # Lazy imports keep the worker (and motion-only deployments) free of the
        # heavy torch/ultralytics dependency unless a YOLO task is actually run.
        import torch
        from ultralytics import YOLO

        self.weights = str(self.config.get("weights", "yolov8n.pt"))
        self.conf = float(self.config.get("threshold", 0.25))
        self.imgsz = self.config.get("imgsz")  # None -> model default

        # Device: explicit override, else auto-detect CUDA, else CPU.
        device = self.config.get("device")
        if device is None or device == "auto":
            device = 0 if torch.cuda.is_available() else "cpu"
        self.device = device

        # Load the model once (weights auto-download on first use if absent).
        self.model = YOLO(self.weights)
        self.names: Dict[int, str] = dict(self.model.names)

        # Optional class allowlist: accept names and/or integer indices.
        self.classes: Optional[List[int]] = self._resolve_classes(self.config.get("classes"))

        # Optional alert event configuration.
        self.emit_events = bool(self.config.get("emit_events", True))
        alert = self.config.get("alert_classes")
        if alert:
            self.alert_classes = frozenset(str(c).lower() for c in alert)
        else:
            self.alert_classes = _PERSON_CLASSES | _VEHICLE_CLASSES

        self.log.info(
            "YOLO loaded weights=%s device=%s conf=%.2f classes=%s",
            self.weights,
            self.device,
            self.conf,
            self.classes if self.classes is not None else "all",
        )

    def _resolve_classes(self, raw: Any) -> Optional[List[int]]:
        """Map a mixed list of class names/indices to COCO class indices."""
        if not raw:
            return None
        name_to_idx = {name.lower(): idx for idx, name in self.names.items()}
        out: List[int] = []
        for item in raw:
            if isinstance(item, bool):  # guard: bool is an int subclass
                continue
            if isinstance(item, int):
                if item in self.names:
                    out.append(item)
                continue
            key = str(item).strip().lower()
            if key.isdigit() and int(key) in self.names:
                out.append(int(key))
            elif key in name_to_idx:
                out.append(name_to_idx[key])
            else:
                self.log.warning("ignoring unknown class filter %r", item)
        return sorted(set(out)) or None

    def analyze(self, frame: FrameContext) -> AnalysisResult:
        img = frame.image().convert("RGB")
        width, height = img.size

        track_kwargs: Dict[str, Any] = {
            "persist": True,                 # keep ByteTrack state across frames
            "tracker": "bytetrack.yaml",
            "conf": self.conf,
            "device": self.device,
            "verbose": False,
        }
        if self.classes is not None:
            track_kwargs["classes"] = self.classes
        if self.imgsz:
            track_kwargs["imgsz"] = self.imgsz

        results = self.model.track(img, **track_kwargs)
        if not results:
            return AnalysisResult()
        result = results[0]

        # Normalize by the model's view of the frame; fall back to PIL size.
        oh, ow = getattr(result, "orig_shape", (height, width))
        ow = ow or width
        oh = oh or height

        detections: List[Detection] = []
        label_counts: Dict[str, int] = {}
        boxes = result.boxes
        if boxes is not None:
            for box in boxes:
                cls_idx = int(box.cls.item())
                label = self.names.get(cls_idx, str(cls_idx))
                confidence = float(box.conf.item())

                x1, y1, x2, y2 = (float(v) for v in box.xyxy[0].tolist())
                bbox = [
                    round(max(0.0, x1) / ow, 5),
                    round(max(0.0, y1) / oh, 5),
                    round((x2 - x1) / ow, 5),
                    round((y2 - y1) / oh, 5),
                ]

                track_id = None
                if box.id is not None:
                    track_id = str(int(box.id.item()))

                detections.append(
                    Detection(
                        label=label,
                        confidence=round(confidence, 4),
                        bbox=bbox,
                        track_id=track_id,
                        attributes={"class_id": cls_idx},
                    )
                )
                label_counts[label] = label_counts.get(label, 0) + 1

        # `result.speed` is a dict of ms timings {preprocess, inference, postprocess}.
        speed = getattr(result, "speed", {}) or {}
        infer_ms = float(speed.get("inference", 0.0))
        self.log.debug(
            "yolo dets=%d inference=%.1fms device=%s labels=%s",
            len(detections),
            infer_ms,
            self.device,
            label_counts or "{}",
        )

        event = self._maybe_event(label_counts) if self.emit_events else None
        return AnalysisResult(detections=detections, event=event)

    def _maybe_event(self, label_counts: Dict[str, int]) -> Optional[Event]:
        """Raise an alert event when configured person/vehicle classes appear."""
        triggered = {
            lbl: n for lbl, n in label_counts.items() if lbl.lower() in self.alert_classes
        }
        if not triggered:
            return None
        has_person = any(lbl.lower() in _PERSON_CLASSES for lbl in triggered)
        return Event(
            event_type="object_detected",
            severity="warning" if has_person else "info",
            payload={"counts": triggered, "total": sum(triggered.values())},
        )


# Registry: task_type -> Analyzer subclass. Stage 3 wires the real YOLO model
# in for "detection" (and the explicit "yolo" alias); motion stays available.
ANALYZERS: Dict[str, type] = {
    "motion": MotionAnalyzer,
    "detection": YoloAnalyzer,
    "yolo": YoloAnalyzer,
}


def register(task_type: str, analyzer_cls: type) -> None:
    """Register an Analyzer subclass for a task type (used by Stage 3)."""
    ANALYZERS[task_type] = analyzer_cls


def build_analyzer(task: Task, log: logging.LoggerAdapter) -> Analyzer:
    cls = ANALYZERS.get(task.task_type)
    if cls is not None:
        try:
            return cls(task.config, log)
        except Exception as exc:  # noqa: BLE001
            # A registered analyzer that can't be constructed (e.g. ultralytics
            # not installed, or weights can't be fetched) must not crash the
            # supervisor — fall back to the safe placeholder, which never
            # fabricates detections, and keep the rest of the worker running.
            log.error(
                "failed to construct %s analyzer for task_type=%r (%s); "
                "falling back to placeholder",
                getattr(cls, "name", cls.__name__),
                task.task_type,
                exc,
            )
    return PlaceholderAnalyzer(task.task_type, task.config, log)


# --------------------------------------------------------------------------- #
# HTTP client
# --------------------------------------------------------------------------- #
class WorkerShutdown(Exception):
    """Raised when a graceful shutdown interrupts an in-flight retry loop."""


class WorkerHTTPError(Exception):
    def __init__(self, status: Optional[int], detail: str, url: str):
        self.status = status
        super().__init__(f"HTTP {status} for {url}: {detail}")


class CoreClient:
    """Thin VisionOps Core client with capped exponential backoff + jitter."""

    def __init__(self, settings: Settings):
        self.s = settings
        self.session = requests.Session()
        self.session.headers["User-Agent"] = "visionops-ai-worker/1.0"

    def close(self) -> None:
        self.session.close()

    def _sleep(self, seconds: float) -> None:
        # Interruptible sleep so shutdown is prompt mid-backoff.
        if SHUTDOWN.wait(seconds):
            raise WorkerShutdown()

    def _request(
        self,
        method: str,
        url: str,
        *,
        allow_404: bool = False,
        **kwargs: Any,
    ) -> Optional[requests.Response]:
        attempt = 0
        last_err = "unknown error"
        while True:
            if SHUTDOWN.is_set():
                raise WorkerShutdown()
            try:
                resp = self.session.request(
                    method, url, timeout=self.s.http_timeout, **kwargs
                )
            except (requests.ConnectionError, requests.Timeout) as exc:
                last_err = f"{type(exc).__name__}: {exc}"
            else:
                if resp.status_code == 404 and allow_404:
                    return None
                if 500 <= resp.status_code < 600:
                    last_err = f"server error {resp.status_code}: {resp.text[:200]}"
                elif resp.status_code >= 400:
                    # Client error: retrying won't help — surface immediately.
                    raise WorkerHTTPError(resp.status_code, resp.text[:200], url)
                else:
                    return resp

            attempt += 1
            if attempt > self.s.http_max_retries:
                raise WorkerHTTPError(None, last_err, url)
            delay = min(self.s.backoff_cap, self.s.backoff_base * (2 ** (attempt - 1)))
            delay += random.uniform(0, delay * 0.25)  # decorrelated jitter
            log.warning(
                "%s %s failed (%s); retry %d/%d in %.1fs",
                method,
                url,
                last_err,
                attempt,
                self.s.http_max_retries,
                delay,
            )
            self._sleep(delay)

    def fetch_tasks(self) -> List[Task]:
        resp = self._request("GET", f"{self.s.api}/api/v1/ai/tasks")
        assert resp is not None  # only None when allow_404
        return [Task.from_json(d) for d in resp.json()]

    def fetch_frame(self, task: Task) -> Optional[FrameContext]:
        """Pull the latest sampled frame; returns None if none exists yet (404)."""
        resp = self._request("GET", f"{self.s.api}{task.frame_url}", allow_404=True)
        if resp is None:
            return None
        age = resp.headers.get("x-frame-age-ms")
        return FrameContext(
            task=task,
            raw=resp.content,
            captured_at=resp.headers.get("x-frame-captured-at") or None,
            age_ms=int(age) if age and age.isdigit() else None,
        )

    def post_results(self, task: Task, result: AnalysisResult) -> int:
        body: Dict[str, Any] = {
            "camera_id": task.camera_id,
            "task_type": task.task_type,
            "timestamp": datetime.now(timezone.utc).isoformat(),
            "detections": [d.to_json() for d in result.detections],
        }
        if result.event is not None:
            body["event"] = result.event.to_json()
        resp = self._request("POST", f"{self.s.api}/api/v1/ai/events", json=body)
        assert resp is not None
        try:
            return int(resp.json().get("detections_ingested", 0))
        except (ValueError, AttributeError):
            return 0


# --------------------------------------------------------------------------- #
# Per-task worker thread
# --------------------------------------------------------------------------- #
class TaskRunner(threading.Thread):
    def __init__(self, task: Task, client: CoreClient):
        super().__init__(name=f"task-{task.id}", daemon=True)
        self.task = task
        self.client = client
        self.log = task_logger(task)
        self.analyzer = build_analyzer(task, self.log)
        self._stop = threading.Event()
        self._last_captured: Optional[str] = None

    def stop(self) -> None:
        self._stop.set()

    def _should_run(self) -> bool:
        return not self._stop.is_set() and not SHUTDOWN.is_set()

    def _cycle(self) -> None:
        frame = self.client.fetch_frame(self.task)
        if frame is None:
            self.log.debug("no sampled frame yet; skipping cycle")
            return
        # Skip re-analyzing a frame we've already seen (worker fps may exceed
        # the sampler's), which also avoids spurious "no motion" baselines.
        if frame.captured_at and frame.captured_at == self._last_captured:
            self.log.debug("frame unchanged (captured_at=%s); skipping", frame.captured_at)
            return
        self._last_captured = frame.captured_at

        result = self.analyzer.analyze(frame)
        if result.is_empty:
            return
        ingested = self.client.post_results(self.task, result)
        self.log.info(
            "posted %d detection(s)%s",
            len(result.detections),
            f" + event '{result.event.event_type}'" if result.event else "",
        )
        self.log.debug("server ingested=%d", ingested)

    def run(self) -> None:
        self.log.info(
            "started analyzer=%s fps=%.2f profile=%s",
            self.analyzer.name,
            self.task.fps,
            self.task.stream_profile,
        )
        period = self.task.period
        while self._should_run():
            start = time.monotonic()
            try:
                self._cycle()
            except WorkerShutdown:
                break
            except WorkerHTTPError as exc:
                self.log.error("ingest/frame error: %s", exc)
            except Exception:  # noqa: BLE001 - one bad frame must not kill the loop
                self.log.exception("unexpected error in cycle")
            elapsed = time.monotonic() - start
            # Interruptible pacing sleep — wakes immediately on stop().
            if self._stop.wait(max(0.0, period - elapsed)):
                break
        self.log.info("stopped")


# --------------------------------------------------------------------------- #
# Supervisor
# --------------------------------------------------------------------------- #
class Supervisor:
    """Polls /ai/tasks and reconciles the set of running TaskRunner threads."""

    def __init__(self, client: CoreClient, settings: Settings):
        self.client = client
        self.s = settings
        self.runners: Dict[str, TaskRunner] = {}

    def _reconcile(self, tasks: List[Task]) -> None:
        by_id = {t.id: t for t in tasks}

        # Stop runners whose task disappeared or whose behavior changed.
        for tid in list(self.runners):
            runner = self.runners[tid]
            new = by_id.get(tid)
            if new is None:
                log.info("task removed; stopping", extra={"task_id": tid})
                runner.stop()
                del self.runners[tid]
            elif new.signature() != runner.task.signature():
                log.info(
                    "task changed; restarting",
                    extra={"task_id": tid, "camera_id": new.camera_id},
                )
                runner.stop()
                runner.join(timeout=self.s.http_timeout + 2)
                del self.runners[tid]

        # Start runners for new (or just-restarted) tasks.
        for tid, task in by_id.items():
            if tid not in self.runners:
                runner = TaskRunner(task, self.client)
                self.runners[tid] = runner
                runner.start()

        # Drop dead threads (e.g. exited on an unexpected fatal error).
        for tid in list(self.runners):
            if not self.runners[tid].is_alive():
                del self.runners[tid]

    def run(self) -> None:
        log.info("supervisor polling %s every %.0fs", self.s.api, self.s.poll_interval)
        while not SHUTDOWN.is_set():
            try:
                tasks = self.client.fetch_tasks()
                self._reconcile(tasks)
                log.debug("active tasks: %d", len(self.runners))
            except WorkerShutdown:
                break
            except WorkerHTTPError as exc:
                log.error("failed to fetch tasks: %s", exc)
            except Exception:  # noqa: BLE001
                log.exception("unexpected error while polling tasks")
            if SHUTDOWN.wait(self.s.poll_interval):
                break
        self._shutdown_all()

    def _shutdown_all(self) -> None:
        if not self.runners:
            return
        log.info("stopping %d task runner(s)", len(self.runners))
        for runner in self.runners.values():
            runner.stop()
        deadline = time.monotonic() + self.s.http_timeout + 5
        for runner in self.runners.values():
            runner.join(timeout=max(0.1, deadline - time.monotonic()))
        self.runners.clear()


# --------------------------------------------------------------------------- #
# Entrypoint
# --------------------------------------------------------------------------- #
def _install_signal_handlers() -> None:
    def handler(signum: int, _frame: Any) -> None:
        log.info("received %s; shutting down gracefully", signal.Signals(signum).name)
        SHUTDOWN.set()

    signal.signal(signal.SIGINT, handler)
    signal.signal(signal.SIGTERM, handler)


def main(argv: Optional[List[str]] = None) -> int:
    settings = parse_settings(argv)
    setup_logging(settings.log_level, settings.log_format)
    _install_signal_handlers()
    log.info("VisionOps AI worker starting (api=%s)", settings.api)

    client = CoreClient(settings)
    try:
        Supervisor(client, settings).run()
    finally:
        client.close()
    log.info("VisionOps AI worker stopped")
    return 0


if __name__ == "__main__":
    sys.exit(main())
