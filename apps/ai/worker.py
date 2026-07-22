#!/usr/bin/env python3
"""Heldar reference AI worker (Stage 2).

This is the canonical, dependency-light implementation of the Heldar AI
worker contract. It proves and documents how a perception worker talks to
Heldar Core so that Stage 3 can drop in a real model (e.g. YOLO) by
implementing a single `Analyzer` subclass — nothing else has to change.

The contract (served by the kernel; see crates/heldar-kernel/src/routes/ai.rs)
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

4. Semantic retrieval (issue #38) — the "embedding" task type indexes CLIP
   vectors of tracked detection crops, and a fast side-channel answers query
   embeddings for the kernel's /search/semantic route:
       POST {API}/api/v1/ai/embeddings
       {
         "camera_id": "...",
         "model":     "open_clip/ViT-B-32/openai",
         "dim":       512,
         "frame_id":  "<task>:<captured_at>",       # optional idempotency key
         "items":     [{ "track_id", "detection_id", "label", "timestamp",
                         "bbox":[x,y,w,h], "vec":[f32...], "thumb_b64" }]
       }                                            # 1..=128 items per POST
       GET  {API}/api/v1/ai/embed-queries?worker_id=...
   -> { "queries": [{ "id", "kind": "text"|"image", "payload" }] }
       POST {API}/api/v1/ai/embed-queries/{id}/result
       { "vec": [f32...], "model": "...", "dim": N }    # or {"error": "..."}
   A 404 on either endpoint means an old kernel: the worker logs once and
   disables that path — it never crash-loops against a core that predates it.

Design
------
* One supervisor thread polls /ai/tasks every `poll_interval` seconds and
  reconciles a set of per-task worker threads (start new, stop removed,
  restart changed).
* Each task thread runs its own loop at the task's `fps`: pull frame ->
  run its `Analyzer` -> POST results.
* A separate daemon thread (EmbedQueryWorker) fast-polls /ai/embed-queries
  (default 1s) and answers semantic-search query embeddings through the same
  shared CLIP backend the "embedding" analyzers use.
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
import base64
import io
import json
import logging
import math
import os
import random
import signal
import socket
import sys
import threading
import time
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Any, Dict, List, Optional, Tuple
from urllib.parse import quote

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
    api_key: Optional[str]
    # Stable identity of this worker process. Sent to /ai/tasks so the kernel can SHARD the task set
    # across multiple workers on one node (each analyzes only its slice) instead of every worker redoing
    # every task. Defaults to "<hostname>:<pid>", so two workers on one host get distinct ids and split
    # the load automatically; a single worker gets the whole set.
    worker_id: str
    # Semantic retrieval (issue #38): the /ai/embed-queries poll cadence and the CLIP checkpoint the
    # query worker answers with — it must match what the "embedding" tasks index with, or the
    # kernel's cosine scores compare vectors from different spaces.
    embed_poll_interval: float
    clip_model: str
    clip_pretrained: str


def _default_worker_id() -> str:
    return f"{socket.gethostname()}:{os.getpid()}"


def _env(key: str, default: str) -> str:
    return os.environ.get(key, default)


def parse_settings(argv: Optional[List[str]] = None) -> Settings:
    """Build settings from env defaults overridden by CLI flags."""
    parser = argparse.ArgumentParser(
        prog="worker",
        description="Heldar reference AI worker (Stage 2).",
    )
    parser.add_argument(
        "--api",
        default=_env("HELDAR_API", "http://localhost:8000"),
        help="Heldar Core base URL (env HELDAR_API).",
    )
    parser.add_argument(
        "--poll-interval",
        type=float,
        default=float(_env("HELDAR_AI_POLL_INTERVAL", "10")),
        help="Seconds between /ai/tasks re-polls (env HELDAR_AI_POLL_INTERVAL).",
    )
    parser.add_argument(
        "--http-timeout",
        type=float,
        default=float(_env("HELDAR_HTTP_TIMEOUT", "10")),
        help="Per-request HTTP timeout in seconds (env HELDAR_HTTP_TIMEOUT).",
    )
    parser.add_argument(
        "--http-max-retries",
        type=int,
        default=int(_env("HELDAR_HTTP_MAX_RETRIES", "5")),
        help="Max retries for transient HTTP failures (env HELDAR_HTTP_MAX_RETRIES).",
    )
    parser.add_argument(
        "--backoff-base",
        type=float,
        default=float(_env("HELDAR_HTTP_BACKOFF_BASE", "0.5")),
        help="Initial backoff in seconds (env HELDAR_HTTP_BACKOFF_BASE).",
    )
    parser.add_argument(
        "--backoff-cap",
        type=float,
        default=float(_env("HELDAR_HTTP_BACKOFF_CAP", "15")),
        help="Max backoff in seconds (env HELDAR_HTTP_BACKOFF_CAP).",
    )
    parser.add_argument(
        "--log-level",
        default=_env("HELDAR_LOG_LEVEL", "INFO"),
        help="Logging level: DEBUG/INFO/WARNING/ERROR (env HELDAR_LOG_LEVEL).",
    )
    parser.add_argument(
        "--log-format",
        choices=("text", "json"),
        default=_env("HELDAR_LOG_FORMAT", "text"),
        help="Log output format (env HELDAR_LOG_FORMAT).",
    )
    parser.add_argument(
        "--api-key",
        default=_env("HELDAR_API_KEY", ""),
        help="API key (integration role) sent as X-API-Key when Core auth is enabled "
        "(env HELDAR_API_KEY). Optional when Core runs with auth disabled.",
    )
    parser.add_argument(
        "--worker-id",
        default=_env("HELDAR_AI_WORKER_ID", _default_worker_id()),
        help="Stable worker identity for task sharding across multiple workers on one node "
        "(env HELDAR_AI_WORKER_ID; default <hostname>:<pid>).",
    )
    parser.add_argument(
        "--embed-poll-interval",
        type=float,
        default=float(_env("HELDAR_AI_EMBED_POLL_INTERVAL", "1.0")),
        help="Seconds between /ai/embed-queries polls — kept fast because semantic-search "
        "requests block on the answer (env HELDAR_AI_EMBED_POLL_INTERVAL).",
    )
    parser.add_argument(
        "--clip-model",
        default=_env("HELDAR_AI_CLIP_MODEL", "ViT-B-32"),
        help="open_clip architecture used to answer embed queries (env HELDAR_AI_CLIP_MODEL).",
    )
    parser.add_argument(
        "--clip-pretrained",
        default=_env("HELDAR_AI_CLIP_PRETRAINED", "openai"),
        help="open_clip pretrained tag used to answer embed queries "
        "(env HELDAR_AI_CLIP_PRETRAINED).",
    )
    ns = parser.parse_args(argv)
    return Settings(
        api=ns.api.rstrip("/"),
        poll_interval=max(1.0, ns.poll_interval),
        http_timeout=ns.http_timeout,
        http_max_retries=max(0, ns.http_max_retries),
        # Clamp to positive values: a 0/negative base or cap (env typo) would make the retry delay 0,
        # turning transient failures into a tight, log-spamming retry loop against an unavailable Core.
        backoff_base=max(0.001, ns.backoff_base),
        backoff_cap=max(0.1, ns.backoff_cap),
        log_level=ns.log_level.upper(),
        log_format=ns.log_format,
        api_key=ns.api_key.strip() or None,
        worker_id=ns.worker_id.strip() or _default_worker_id(),
        # Floor at 0.2s: this endpoint is polled forever by design, so a 0/negative interval
        # (env typo) would hammer Core in a tight loop.
        embed_poll_interval=max(0.2, ns.embed_poll_interval),
        clip_model=ns.clip_model.strip() or "ViT-B-32",
        clip_pretrained=ns.clip_pretrained.strip() or "openai",
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
class EmbeddingItem:
    """One CLIP vector of a tracked detection crop, destined for POST /api/v1/ai/embeddings.

    ``model`` is carried per item but sent once at the batch level (every item from one analyzer
    instance shares it); the wire-level ``dim`` is derived as ``len(vec)``. ``vec`` holds plain
    python floats straight from the CLIP output tensor — deliberately NOT normalized, the kernel
    computes full cosine similarity itself.
    """

    vec: List[float]
    model: str  # e.g. "open_clip/ViT-B-32/openai"
    label: Optional[str] = None
    bbox: Optional[List[float]] = None  # [x, y, w, h] normalized 0..1
    track_id: Optional[str] = None
    detection_id: Optional[str] = None
    timestamp: Optional[str] = None  # RFC3339 observation time; kernel defaults to now
    thumb_b64: Optional[str] = None  # small JPEG crop, base64 (no data: prefix)

    def to_json(self) -> Dict[str, Any]:
        out: Dict[str, Any] = {"vec": [float(v) for v in self.vec]}
        if self.track_id is not None:
            out["track_id"] = self.track_id
        if self.detection_id is not None:
            out["detection_id"] = self.detection_id
        if self.label is not None:
            out["label"] = self.label
        if self.timestamp is not None:
            out["timestamp"] = self.timestamp
        if self.bbox is not None:
            out["bbox"] = [float(v) for v in self.bbox]
        if self.thumb_b64 is not None:
            out["thumb_b64"] = self.thumb_b64
        return out


@dataclass
class AnalysisResult:
    detections: List[Detection] = field(default_factory=list)
    event: Optional[Event] = None
    # Semantic-index vectors (issue #38). Deliberately separate from `detections`: an embedding
    # task returns vectors WITHOUT detections, so indexing never double-fires the zone/entry
    # consumers that watch /ai/events.
    embeddings: List[EmbeddingItem] = field(default_factory=list)

    @property
    def is_empty(self) -> bool:
        return not self.detections and not self.embeddings and self.event is None


def safe_weights(config: Dict[str, Any]) -> str:
    """Resolve the YOLO weights name from task config, rejecting path traversal / absolute paths so a
    task can only name a local model file (or a known ultralytics name it downloads) — never an
    arbitrary filesystem path or `..` escape from an untrusted task definition."""
    w = str(config.get("weights", "yolov8n.pt"))
    if "/" in w or "\\" in w or ".." in w or w.startswith("~"):
        raise ValueError(f"unsafe weights path in task config: {w!r}")
    return w


def norm_bbox(
    x1: float, y1: float, x2: float, y2: float, ow: float, oh: float
) -> Optional[List[float]]:
    """Normalize a pixel xyxy box to ``[x, y, w, h]`` in 0..1, clamped to the frame.

    YOLO boxes routinely extend past the frame edge for objects at the border; clamping each
    coordinate to [0,1] before differencing keeps w/h within the kernel contract (0..1) and stops the
    dashboard from drawing boxes outside the frame. Returns ``None`` for a degenerate frame/box.
    """
    if ow <= 0 or oh <= 0:
        return None
    nx1 = min(1.0, max(0.0, x1 / ow))
    ny1 = min(1.0, max(0.0, y1 / oh))
    nx2 = min(1.0, max(0.0, x2 / ow))
    ny2 = min(1.0, max(0.0, y2 / oh))
    return [
        round(nx1, 5),
        round(ny1, 5),
        round(max(0.0, nx2 - nx1), 5),
        round(max(0.0, ny2 - ny1), 5),
    ]


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


def _resolve_class_filter(
    raw: Any, names: Dict[int, str], log: logging.LoggerAdapter
) -> Optional[List[int]]:
    """Map a mixed list of class names/indices to model class indices (the `classes` config key
    shared by the YOLO-backed analyzers)."""
    if not raw:
        return None
    name_to_idx = {name.lower(): idx for idx, name in names.items()}
    out: List[int] = []
    for item in raw:
        if isinstance(item, bool):  # guard: bool is an int subclass
            continue
        if isinstance(item, int):
            if item in names:
                out.append(item)
            continue
        key = str(item).strip().lower()
        if key.isdigit() and int(key) in names:
            out.append(int(key))
        elif key in name_to_idx:
            out.append(name_to_idx[key])
        else:
            log.warning("ignoring unknown class filter %r", item)
    return sorted(set(out)) or None


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

        self.weights = safe_weights(self.config)
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
        self.classes: Optional[List[int]] = _resolve_class_filter(
            self.config.get("classes"), self.names, self.log
        )

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
                bbox = norm_bbox(x1, y1, x2, y2, ow, oh)
                if bbox is None:
                    continue

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


# Basic color buckets for the (assistive) vehicle-color heuristic. Names match what an operator
# would type when registering a vehicle, so the core's case-insensitive mismatch check lines up.
_COLOR_NAMES = ("black", "white", "gray", "red", "orange", "yellow", "green", "blue", "purple")


def _estimate_color(img_rgb: "Image.Image", bbox_px: tuple) -> Optional[str]:
    """Crude dominant-color estimate over the centre of a vehicle box (assistive metadata only).

    Returns a coarse color name or None. This is deliberately simple — color is
    secondary verification, not an access decision, and real accuracy needs local benchmarking.
    """
    x1, y1, x2, y2 = bbox_px
    if x2 - x1 < 4 or y2 - y1 < 4:
        return None
    # Sample the central 50% of the box to avoid background/edges.
    cx1 = x1 + (x2 - x1) // 4
    cx2 = x2 - (x2 - x1) // 4
    cy1 = y1 + (y2 - y1) // 4
    cy2 = y2 - (y2 - y1) // 4
    crop = img_rgb.crop((cx1, cy1, cx2, cy2)).resize((16, 16), Image.BILINEAR)
    arr = np.asarray(crop, dtype=np.float32) / 255.0
    r, g, b = (float(arr[..., i].mean()) for i in range(3))
    mx, mn = max(r, g, b), min(r, g, b)
    v = mx
    s = 0.0 if mx <= 0 else (mx - mn) / mx
    if s < 0.18:  # achromatic
        if v < 0.25:
            return "black"
        if v > 0.72:
            return "white"
        return "gray"
    # Hue in degrees.
    if mx == mn:
        h = 0.0
    elif mx == r:
        h = (60 * ((g - b) / (mx - mn)) + 360) % 360
    elif mx == g:
        h = 60 * ((b - r) / (mx - mn)) + 120
    else:
        h = 60 * ((r - g) / (mx - mn)) + 240
    if h < 15 or h >= 345:
        return "red"
    if h < 45:
        return "orange"
    if h < 70:
        return "yellow"
    if h < 170:
        return "green"
    if h < 260:
        return "blue"
    if h < 345:
        return "purple"
    return None


class _OcrBackend:
    """Lazy, optional OCR backend for plate reading. Tries PaddleOCR then EasyOCR; if neither is
    installed it stays disabled and the analyzer simply emits vehicles WITHOUT a plate (never a
    fabricated one). Returns (text, confidence) for the most plate-like token found."""

    def __init__(self, preferred: Optional[str], log: logging.LoggerAdapter):
        self.log = log
        self.kind: Optional[str] = None
        self._engine = None
        self._init(preferred)

    def _init(self, preferred: Optional[str]) -> None:
        order = [preferred] if preferred else ["paddleocr", "easyocr"]
        for kind in order:
            if not kind:
                continue
            try:
                if kind == "paddleocr":
                    from paddleocr import PaddleOCR  # type: ignore

                    self._engine = PaddleOCR(use_angle_cls=False, lang="en", show_log=False)
                    self.kind = "paddleocr"
                    self.log.info("ANPR OCR backend: PaddleOCR")
                    return
                if kind == "easyocr":
                    import easyocr  # type: ignore

                    self._engine = easyocr.Reader(["en"], gpu=False)
                    self.kind = "easyocr"
                    self.log.info("ANPR OCR backend: EasyOCR")
                    return
            except Exception as exc:  # noqa: BLE001
                self.log.warning("OCR backend %s unavailable (%s)", kind, exc)
        self.log.warning(
            "ANPR: no OCR backend available (install paddleocr or easyocr); emitting vehicle "
            "attributes WITHOUT plate reads. The engine will still log unreadable-plate events."
        )

    @property
    def enabled(self) -> bool:
        return self._engine is not None

    def read_plate(self, crop: "Image.Image") -> Optional[tuple]:
        if self._engine is None:
            return None
        try:
            arr = np.asarray(crop.convert("RGB"))
            candidates: List[tuple] = []  # (text, conf)
            if self.kind == "paddleocr":
                result = self._engine.ocr(arr, cls=False)
                for block in result or []:
                    for line in block or []:
                        try:
                            text, conf = line[1][0], float(line[1][1])
                            candidates.append((text, conf))
                        except (IndexError, TypeError, ValueError):
                            continue
            elif self.kind == "easyocr":
                for item in self._engine.readtext(arr):
                    try:
                        text, conf = item[1], float(item[2])
                        candidates.append((text, conf))
                    except (IndexError, TypeError, ValueError):
                        continue
            best = None
            for text, conf in candidates:
                norm = "".join(c for c in text.upper() if c.isalnum())
                if not (3 <= len(norm) <= 10):
                    continue
                if not (any(c.isalpha() for c in norm) and any(c.isdigit() for c in norm)):
                    continue
                if best is None or conf > best[1]:
                    best = (norm, conf)
            return best
        except Exception as exc:  # noqa: BLE001
            self.log.debug("OCR read failed: %s", exc)
            return None


class _MakeModelClassifier:
    """Optional DIY vehicle make/model classifier (issue #37): an ONNX image classifier applied to
    vehicle crops. Bring your own weights — any classifier over vehicle-crop images works (e.g. a
    ResNet fine-tuned on Stanford Cars / VMMRdb / local footage). Emits `make` + `model` attributes
    (labels formatted "Make Model" or "Make|Model", one per line); the core's engine treats them as
    SECONDARY assist only (a mismatch raises a review exception, never an auto-reject), so imperfect
    weights degrade to extra guard reviews, not wrong gate decisions.

    Task config keys: `make_model_onnx` (path), `make_model_labels` (path), and
    `make_model_min_conf` (default 0.5). Disabled unless both files load; import of onnxruntime is
    lazy and failures are non-fatal."""

    def __init__(self, config: Dict[str, Any], log: logging.LoggerAdapter):
        self.log = log
        self._session = None
        self.labels: List[str] = []
        self.min_conf = float(config.get("make_model_min_conf", 0.5))
        self.input_size = int(config.get("make_model_input", 224))
        onnx_path = config.get("make_model_onnx")
        labels_path = config.get("make_model_labels")
        if not onnx_path or not labels_path:
            return
        try:
            import onnxruntime  # type: ignore

            with open(labels_path, "r", encoding="utf-8") as fh:
                self.labels = [ln.strip() for ln in fh if ln.strip()]
            self._session = onnxruntime.InferenceSession(
                onnx_path, providers=onnxruntime.get_available_providers()
            )
            self._input_name = self._session.get_inputs()[0].name
            self.log.info(
                "make/model classifier loaded: %s (%d labels)", onnx_path, len(self.labels)
            )
        except Exception as exc:  # noqa: BLE001
            self._session = None
            self.log.warning("make/model classifier unavailable (%s); attribute disabled", exc)

    @property
    def enabled(self) -> bool:
        return self._session is not None and bool(self.labels)

    def classify(self, crop: "Image.Image") -> Optional[tuple]:
        """Returns (make, model, confidence) or None below the confidence floor / on error."""
        if not self.enabled:
            return None
        try:
            img = crop.convert("RGB").resize((self.input_size, self.input_size))
            arr = np.asarray(img, dtype=np.float32) / 255.0
            # Standard ImageNet normalization; DIY exporters should match it (documented).
            mean = np.array([0.485, 0.456, 0.406], dtype=np.float32)
            std = np.array([0.229, 0.224, 0.225], dtype=np.float32)
            arr = (arr - mean) / std
            arr = arr.transpose(2, 0, 1)[None, ...]  # NCHW
            out = self._session.run(None, {self._input_name: arr})[0][0]
            out = np.asarray(out, dtype=np.float64)
            # Softmax if the exporter left logits.
            if out.min() < 0.0 or out.max() > 1.0:
                e = np.exp(out - out.max())
                out = e / e.sum()
            idx = int(out.argmax())
            conf = float(out[idx])
            if conf < self.min_conf or idx >= len(self.labels):
                return None
            raw = self.labels[idx].replace("|", " ")
            parts = raw.split(None, 1)
            make = parts[0]
            model = parts[1] if len(parts) > 1 else ""
            return (make, model, conf)
        except Exception as exc:  # noqa: BLE001
            self.log.debug("make/model classify failed: %s", exc)
            return None


class AnprAnalyzer(Analyzer):
    """ANPR / vehicle-attribute analyzer (Stage 4).

    Detects + tracks vehicles with YOLOv8 + ByteTrack (same backbone as :class:`YoloAnalyzer`),
    estimates a coarse color, and — when an OCR backend is installed — reads the plate from the
    vehicle crop. It emits ONE detection per vehicle box per frame, carrying the per-frame plate
    read and attributes in ``attributes``; the core's ANPR engine performs temporal voting across
    frames, validates the plate, and resolves authorization. This analyzer never fabricates a plate:
    if OCR is unavailable or unconfident, it simply omits the plate field.

    Per-task ``config`` keys (all optional):
      * ``weights``    — YOLO weights (default ``yolov8n.pt``).
      * ``threshold``  — min vehicle confidence (default ``0.3``).
      * ``ocr``        — force OCR backend: ``"paddleocr"`` | ``"easyocr"`` (default: auto-detect).
      * ``direction``  — fixed lane direction for this camera: ``"inbound"`` | ``"outbound"``
                         (gate cameras are usually single-direction; the core has no calibrated
                         line-crossing yet, so this is how direction is supplied).
      * ``device``     — force device (default auto).
      * ``min_box_area`` — ignore vehicle boxes smaller than this fraction of the frame (default 0).
    """

    name = "anpr"

    def __init__(self, config: Dict[str, Any], log: logging.LoggerAdapter):
        super().__init__(config, log)
        import torch
        from ultralytics import YOLO

        self.weights = safe_weights(self.config)
        self.conf = float(self.config.get("threshold", 0.3))
        self.imgsz = self.config.get("imgsz")
        device = self.config.get("device")
        if device is None or device == "auto":
            device = 0 if torch.cuda.is_available() else "cpu"
        self.device = device
        self.model = YOLO(self.weights)
        self.names: Dict[int, str] = dict(self.model.names)
        # Restrict to vehicle classes for speed.
        self.vehicle_classes = sorted(
            idx for idx, name in self.names.items() if name.lower() in _VEHICLE_CLASSES
        )
        direction = self.config.get("direction")
        self.direction = direction if direction in ("inbound", "outbound") else None
        self.min_box_area = float(self.config.get("min_box_area", 0.0))
        self.ocr = _OcrBackend(self.config.get("ocr"), log)
        self.make_model = _MakeModelClassifier(self.config, log)
        self.model_versions = {
            "anpr": f"anpr_v0.1_{self.ocr.kind or 'noocr'}",
            "vehicle_attr": "heuristic_v0.1",
            "make_model": "onnx_diy" if self.make_model.enabled else "none",
            "detector": self.weights,
        }
        self.log.info(
            "ANPR loaded weights=%s device=%s ocr=%s direction=%s",
            self.weights,
            self.device,
            self.ocr.kind or "none",
            self.direction or "unknown",
        )

    def analyze(self, frame: FrameContext) -> AnalysisResult:
        img = frame.image().convert("RGB")
        width, height = img.size
        track_kwargs: Dict[str, Any] = {
            "persist": True,
            "tracker": "bytetrack.yaml",
            "conf": self.conf,
            "device": self.device,
            "verbose": False,
        }
        if self.vehicle_classes:
            track_kwargs["classes"] = self.vehicle_classes
        if self.imgsz:
            track_kwargs["imgsz"] = self.imgsz

        results = self.model.track(img, **track_kwargs)
        if not results:
            return AnalysisResult()
        result = results[0]
        oh, ow = getattr(result, "orig_shape", (height, width))
        ow = ow or width
        oh = oh or height

        detections: List[Detection] = []
        boxes = result.boxes
        plates_read = 0
        if boxes is not None:
            for box in boxes:
                cls_idx = int(box.cls.item())
                vtype = self.names.get(cls_idx, str(cls_idx))
                confidence = float(box.conf.item())
                x1, y1, x2, y2 = (float(v) for v in box.xyxy[0].tolist())
                bbox = norm_bbox(x1, y1, x2, y2, ow, oh)
                if bbox is None:
                    continue
                # Normalized area = w*h; drop boxes below the minimum-area gate.
                if bbox[2] * bbox[3] < self.min_box_area:
                    continue
                track_id = str(int(box.id.item())) if box.id is not None else None

                attrs: Dict[str, Any] = {
                    "vehicle_type": vtype,
                    "model_versions": self.model_versions,
                }
                color = _estimate_color(img, (int(x1), int(y1), int(x2), int(y2)))
                if color:
                    attrs["color"] = color
                if self.direction:
                    attrs["direction"] = self.direction
                if self.ocr.enabled or self.make_model.enabled:
                    crop = img.crop((int(x1), int(y1), int(x2), int(y2)))
                    if self.ocr.enabled:
                        plate = self.ocr.read_plate(crop)
                        if plate:
                            attrs["plate"] = plate[0]
                            attrs["plate_confidence"] = round(plate[1], 4)
                            plates_read += 1
                    if self.make_model.enabled:
                        mm = self.make_model.classify(crop)
                        if mm:
                            attrs["make"] = mm[0]
                            if mm[1]:
                                attrs["model"] = mm[1]
                            attrs["make_model_confidence"] = round(mm[2], 4)

                detections.append(
                    Detection(
                        label=vtype,
                        confidence=round(confidence, 4),
                        bbox=bbox,
                        track_id=track_id,
                        attributes=attrs,
                    )
                )

        self.log.debug(
            "anpr vehicles=%d plates=%d ocr=%s",
            len(detections),
            plates_read,
            self.ocr.kind or "none",
        )
        return AnalysisResult(detections=detections)


# Process-wide CLIP cache: N camera tasks + the embed-query worker share ONE loaded model per
# (architecture, pretrained tag, device) instead of each loading a ~350 MB copy.
_CLIP_LOCK = threading.Lock()
_CLIP_BACKENDS: Dict[tuple, "_ClipBackend"] = {}


class _ClipBackend:
    """One loaded open_clip model + its preprocess/tokenizer. Obtain via
    :func:`_get_clip_backend` — instances are shared process-wide, and ``encode_*`` serializes
    access (one forward pass at a time keeps CPU inference from oversubscribing threads when
    several tasks share the model)."""

    def __init__(self, model_name: str, pretrained: str, device: Any):
        # Lazy heavy imports: callers construct this from their own __init__ / first use, so a
        # missing open_clip/torch degrades (placeholder analyzer, disabled query worker) instead
        # of crashing the worker.
        import open_clip
        import torch

        self.torch = torch
        self.model_name = model_name
        self.pretrained = pretrained
        self.device = device
        #: Wire-level model id (embeddings.model in the kernel), e.g. "open_clip/ViT-B-32/openai".
        self.model_id = f"open_clip/{model_name}/{pretrained}"
        model, _, preprocess = open_clip.create_model_and_transforms(
            model_name, pretrained=pretrained
        )
        self.model = model.eval().to(device)
        self.preprocess = preprocess
        self.tokenizer = open_clip.get_tokenizer(model_name)
        self._lock = threading.Lock()

    def encode_images(self, images: List["Image.Image"]) -> List[List[float]]:
        """Embed RGB PIL images in ONE batched forward pass. Returns rows of plain python
        floats, deliberately NOT normalized — the kernel computes full cosine itself."""
        with self._lock, self.torch.no_grad():
            batch = self.torch.stack([self.preprocess(im) for im in images]).to(self.device)
            feats = self.model.encode_image(batch)
            return feats.cpu().float().tolist()

    def encode_text(self, text: str) -> List[float]:
        """Embed one text query through the CLIP text tower (plain floats, not normalized)."""
        with self._lock, self.torch.no_grad():
            tokens = self.tokenizer([text]).to(self.device)
            feats = self.model.encode_text(tokens)
            return feats.cpu().float().tolist()[0]


def _get_clip_backend(model_name: str, pretrained: str, device: Any) -> _ClipBackend:
    """The process-wide CLIP singleton, keyed by (model, pretrained, device). Loading happens
    inside the lock so concurrent first users wait for one load instead of racing a second."""
    key = (model_name, pretrained, str(device))
    with _CLIP_LOCK:
        backend = _CLIP_BACKENDS.get(key)
        if backend is None:
            backend = _ClipBackend(model_name, pretrained, device)
            _CLIP_BACKENDS[key] = backend
        return backend


class EmbeddingAnalyzer(Analyzer):
    """CLIP semantic-retrieval indexer (issue #38).

    Detects + tracks objects with YOLOv8 + ByteTrack (same backbone as :class:`YoloAnalyzer`,
    restricted to vehicle classes by default — person embedding is deliberately OFF as a privacy
    posture; opt in explicitly via ``classes``), crops each tracked box, and embeds the crops with
    open_clip in one batched forward pass. Each track is embedded on FIRST SIGHT and then every
    ``stride_seconds`` (untracked boxes are skipped — without a track identity there is nothing to
    stride-gate on). The vectors ride :attr:`AnalysisResult.embeddings` and are POSTed to
    ``/api/v1/ai/embeddings``; the analyzer returns NO detections — this is an indexing task, and
    detections here would double-fire the zone/entry consumers already fed by a ``detection``
    task on the same camera.

    Per-task ``config`` keys (all optional):
      * ``weights``         — YOLO weights (default ``yolov8n.pt``; safe_weights-guarded).
      * ``conf``            — min box confidence to consider a crop (default ``0.35``).
      * ``classes``         — class names/indices to embed (default ``[1, 2, 3, 5, 7]`` =
                              bicycle/car/motorcycle/bus/truck; person deliberately excluded).
      * ``stride_seconds``  — re-embed cadence per track (default ``10``).
      * ``static_suppression`` — skip the stride refresh while a track hasn't moved (default
                              ``True``; mirrors the zone engine's knob). A parked car otherwise
                              burns thousands of near-identical vectors + thumbs per day.
      * ``static_epsilon``  — normalized bbox movement (max |Δ| over x/y/w/h) below which a
                              track counts as static (default ``0.02``, same as zones).
      * ``static_refresh_seconds`` — even a static track is re-embedded this often (default
                              ``3600``) so a permanently parked object re-enters the index
                              after its old rows age out on the retention TTL.
      * ``min_box_px``      — skip boxes narrower/shorter than this many pixels (default ``24``).
      * ``clip_model``      — open_clip architecture (default ``ViT-B-32``).
      * ``clip_pretrained`` — open_clip pretrained tag (default ``openai``).
      * ``device``          — force a device (default auto: CUDA if available else CPU).
      * ``thumb_max_px``    — max dimension of the JPEG crop thumb sent for the UI
                              (default ``320``; ``0`` disables thumbs).
      * ``imgsz``           — YOLO inference image size (default model native).
    """

    name = "embedding"

    #: Drop a track's stride-gate entry after this much idle time (the track is long gone).
    _TRACK_IDLE_S = 600.0
    #: Kernel cap on one thumb's base64 length (an over-cap thumb 400s the whole batch) —
    #: past it we keep the vector and drop the thumb.
    _THUMB_B64_MAX = 131072

    def __init__(self, config: Dict[str, Any], log: logging.LoggerAdapter):
        super().__init__(config, log)
        import torch
        from ultralytics import YOLO

        self.weights = safe_weights(self.config)
        self.conf = float(self.config.get("conf", 0.35))
        self.imgsz = self.config.get("imgsz")
        device = self.config.get("device")
        if device is None or device == "auto":
            device = 0 if torch.cuda.is_available() else "cpu"
        self.device = device
        self.model = YOLO(self.weights)
        self.names: Dict[int, str] = dict(self.model.names)
        raw_classes = self.config.get("classes", [1, 2, 3, 5, 7])
        self.classes: Optional[List[int]] = _resolve_class_filter(
            raw_classes, self.names, self.log
        )
        if self.classes is None:
            # Privacy posture: this analyzer must never silently widen to "all classes" (which
            # includes person). An unresolvable `classes` config drops back to the vehicle
            # default; if even that is absent from the model's classes, refuse to construct
            # (build_analyzer then installs the safe placeholder).
            self.log.warning(
                "embedding `classes` %r resolved to nothing; using the vehicle default",
                raw_classes,
            )
            self.classes = _resolve_class_filter([1, 2, 3, 5, 7], self.names, self.log)
            if self.classes is None:
                raise ValueError(
                    "no resolvable `classes` for the embedding task; refusing to embed every class"
                )
        self.stride_seconds = max(0.0, float(self.config.get("stride_seconds", 10)))
        self.static_suppression = bool(self.config.get("static_suppression", True))
        eps = self.config.get("static_epsilon", 0.02)
        try:
            eps = float(eps)
        except (TypeError, ValueError):
            eps = 0.02
        self.static_epsilon = eps if math.isfinite(eps) and eps > 0 else 0.02
        self.static_epsilon = min(max(self.static_epsilon, 0.001), 0.5)
        # A static track still refreshes on this slow floor: without it, a permanently parked
        # object's only embeddings would age out on the retention TTL and it would silently
        # disappear from semantic search.
        refresh = self.config.get("static_refresh_seconds", 3600)
        try:
            refresh = float(refresh)
        except (TypeError, ValueError):
            refresh = 3600.0
        if not math.isfinite(refresh) or refresh <= 0:
            refresh = 3600.0
        self.static_refresh_seconds = max(self.stride_seconds, refresh)
        # The stride gate's idle eviction must outlast the stride itself, or a continuously
        # visible track with a long stride would be re-embedded every eviction instead.
        self._track_idle_s = max(
            self._TRACK_IDLE_S, 2.0 * self.stride_seconds, 2.0 * self.static_refresh_seconds
        )
        self.min_box_px = float(self.config.get("min_box_px", 24))
        self.thumb_max_px = int(self.config.get("thumb_max_px", 320))
        self.clip = _get_clip_backend(
            str(self.config.get("clip_model", "ViT-B-32")),
            str(self.config.get("clip_pretrained", "openai")),
            self.device,
        )
        # track_id -> (monotonic time of last embed, bbox at last embed) — the stride gate plus
        # the static-suppression reference position.
        self._last_embedded: Dict[str, Tuple[float, Optional[List[float]]]] = {}
        self.log.info(
            "embedding loaded weights=%s device=%s clip=%s stride=%.0fs classes=%s",
            self.weights,
            self.device,
            self.clip.model_id,
            self.stride_seconds,
            self.classes if self.classes is not None else "all",
        )

    def analyze(self, frame: FrameContext) -> AnalysisResult:
        img = frame.image().convert("RGB")
        width, height = img.size
        track_kwargs: Dict[str, Any] = {
            "persist": True,
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
        oh, ow = getattr(result, "orig_shape", (height, width))
        ow = ow or width
        oh = oh or height

        now = time.monotonic()
        self._prune_stride_gate(now)

        # Collect the crops due for embedding: first sight of a track, then every stride_seconds.
        due: List[tuple] = []  # (crop, label, bbox, track_id)
        boxes = result.boxes
        if boxes is not None:
            for box in boxes:
                if box.id is None:
                    continue  # untracked box: no identity to stride-gate on
                track_id = str(int(box.id.item()))
                state = self._last_embedded.get(track_id)
                if state is not None and now - state[0] < self.stride_seconds:
                    continue
                x1, y1, x2, y2 = (float(v) for v in box.xyxy[0].tolist())
                if x2 - x1 < self.min_box_px or y2 - y1 < self.min_box_px:
                    continue  # too small for a useful embedding
                bbox = norm_bbox(x1, y1, x2, y2, ow, oh)
                if bbox is None:
                    continue
                # Static suppression (mirrors the zone engine): the stride has elapsed, but if
                # the box hasn't moved since the last embed there is nothing new to index — a
                # parked car must not accrete near-identical vectors every stride. The slow
                # refresh floor keeps even a permanently static object present in the index
                # after its older rows age out on the retention TTL.
                if (
                    self.static_suppression
                    and state is not None
                    and state[1] is not None
                    and now - state[0] < self.static_refresh_seconds
                    and max(abs(a - b) for a, b in zip(bbox, state[1])) < self.static_epsilon
                ):
                    continue
                cls_idx = int(box.cls.item())
                label = self.names.get(cls_idx, str(cls_idx))
                crop = img.crop((int(x1), int(y1), int(x2), int(y2)))
                due.append((crop, label, bbox, track_id))

        if not due:
            return AnalysisResult()

        # ONE batched CLIP forward pass for every due crop on this frame. The stride gate is only
        # advanced afterwards, so a failed encode retries on the next frame.
        vecs = self.clip.encode_images([crop for crop, _, _, _ in due])
        items: List[EmbeddingItem] = []
        for (crop, label, bbox, track_id), vec in zip(due, vecs):
            self._last_embedded[track_id] = (now, bbox)
            items.append(
                EmbeddingItem(
                    vec=vec,
                    model=self.clip.model_id,
                    label=label,
                    bbox=bbox,
                    track_id=track_id,
                    timestamp=frame.captured_at,
                    thumb_b64=self._thumb_b64(crop),
                )
            )
        self.log.debug("embedded %d crop(s) dim=%d", len(items), len(vecs[0]))
        # Indexing task: NO detections — a paired "detection" task feeds the zone/entry consumers.
        return AnalysisResult(embeddings=items)

    def _thumb_b64(self, crop: "Image.Image") -> Optional[str]:
        """Small JPEG of the crop for the UI's ranked results (base64, no data: prefix)."""
        if self.thumb_max_px <= 0:
            return None
        thumb = crop.copy()
        thumb.thumbnail((self.thumb_max_px, self.thumb_max_px))
        buf = io.BytesIO()
        thumb.save(buf, format="JPEG", quality=70)
        b64 = base64.b64encode(buf.getvalue()).decode("ascii")
        return b64 if len(b64) <= self._THUMB_B64_MAX else None

    def _prune_stride_gate(self, now: float) -> None:
        stale = [
            tid
            for tid, (ts, _bbox) in self._last_embedded.items()
            if now - ts > self._track_idle_s
        ]
        for tid in stale:
            del self._last_embedded[tid]


# Registry: task_type -> Analyzer subclass. Stage 3 wires the real YOLO model
# in for "detection" (and the explicit "yolo" alias); Stage 4 adds "anpr"; issue #38 adds
# "embedding"; motion stays available.
ANALYZERS: Dict[str, type] = {
    "motion": MotionAnalyzer,
    "detection": YoloAnalyzer,
    "yolo": YoloAnalyzer,
    "anpr": AnprAnalyzer,
    "embedding": EmbeddingAnalyzer,
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
    """Thin Heldar Core client with capped exponential backoff + jitter."""

    def __init__(self, settings: Settings):
        self.s = settings
        self.session = requests.Session()
        self.session.headers["User-Agent"] = "heldar-ai-worker/1.0"
        # When Core auth is enabled, the worker authenticates with an integration API key. Harmless
        # when auth is disabled (Core ignores it). Sent on every request via the shared session.
        if settings.api_key:
            self.session.headers["X-API-Key"] = settings.api_key

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
                    # Log only the status (no body): a 5xx body can echo internal detail/secrets, and
                    # this line is logged at WARNING on every retry.
                    last_err = f"server error {resp.status_code}"
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
        # Send worker_id so the kernel shards the task set across co-located workers (and treats this
        # poll as our liveness heartbeat). A single worker gets the whole set; N workers split it.
        url = f"{self.s.api}/api/v1/ai/tasks?worker_id={quote(self.s.worker_id, safe='')}"
        resp = self._request("GET", url)
        if resp is None:  # only None when allow_404, which we didn't set
            raise WorkerHTTPError(None, "unexpected empty response from _request", url)
        return [Task.from_json(d) for d in resp.json()]

    def fetch_frame(self, task: Task) -> Optional[FrameContext]:
        """Pull the latest sampled frame; returns None if none exists yet (404)."""
        # The frame URL comes from the server's task list; defend against SSRF / path escape by
        # requiring a relative API path (no scheme/host, no `..`).
        fu = task.frame_url
        if not fu.startswith("/") or "://" in fu or ".." in fu:
            log.error("rejecting unsafe frame_url %r for task %s", fu, task.id)
            return None
        resp = self._request("GET", f"{self.s.api}{fu}", allow_404=True)
        if resp is None:
            return None
        age = resp.headers.get("x-frame-age-ms")
        return FrameContext(
            task=task,
            raw=resp.content,
            captured_at=resp.headers.get("x-frame-captured-at") or None,
            age_ms=int(age) if age and age.isdigit() else None,
        )

    def post_results(
        self, task: Task, result: AnalysisResult, frame_id: Optional[str] = None
    ) -> int:
        # Bound the batch to the kernel's per-request cap (MAX_INGEST_DETECTIONS = 1000): an
        # over-cap POST is rejected wholesale (400), losing everything. Truncating keeps the bulk and
        # is logged. Order is detector-confidence-descending, so the kept slice is the most salient.
        dets = result.detections
        if len(dets) > 1000:
            log.warning(
                "capping %d detections to 1000 (kernel per-request limit)", len(dets)
            )
            dets = dets[:1000]
        body: Dict[str, Any] = {
            "camera_id": task.camera_id,
            "task_type": task.task_type,
            "timestamp": datetime.now(timezone.utc).isoformat(),
            "detections": [d.to_json() for d in dets],
        }
        # Idempotency key: lets Core dedup an at-least-once redelivery of this exact frame's batch
        # (e.g. a retry after a committed-but-unacked POST) so consumer side effects don't double-fire.
        if frame_id is not None:
            body["frame_id"] = frame_id
        if result.event is not None:
            body["event"] = result.event.to_json()
        url = f"{self.s.api}/api/v1/ai/events"
        resp = self._request("POST", url, json=body)
        if resp is None:  # only None when allow_404, which we didn't set
            raise WorkerHTTPError(None, "unexpected empty response from _request", url)
        try:
            val = resp.json().get("detections_ingested", 0)
            return int(val) if val is not None else 0
        except (ValueError, AttributeError, TypeError):
            return 0

    # Kernel per-request cap on POST /ai/embeddings items (contract: 1..=128 per batch).
    EMBED_BATCH_MAX = 128

    def post_embeddings(
        self, task: Task, items: List[EmbeddingItem], frame_id: Optional[str] = None
    ) -> Optional[int]:
        """POST embedding vectors for one frame, split into <=128-item batches (kernel cap).

        Returns the total ingested, or None when the kernel 404s the endpoint (a core that
        predates issue #38) so the caller can log once and stop posting for the task. Redelivered
        rows are deduped server-side on (camera_id, frame_id, track_id) and simply not counted.
        """
        total = 0
        url = f"{self.s.api}/api/v1/ai/embeddings"
        for start in range(0, len(items), self.EMBED_BATCH_MAX):
            batch = items[start : start + self.EMBED_BATCH_MAX]
            body: Dict[str, Any] = {
                "camera_id": task.camera_id,
                "model": batch[0].model,
                "dim": len(batch[0].vec),
                "items": [item.to_json() for item in batch],
            }
            if frame_id is not None:
                body["frame_id"] = frame_id
            resp = self._request("POST", url, json=body, allow_404=True)
            if resp is None:
                return None
            try:
                val = resp.json().get("embeddings_ingested", 0)
                total += int(val) if val is not None else 0
            except (ValueError, AttributeError, TypeError):
                pass
        return total

    def fetch_embed_queries(self) -> Optional[List[Dict[str, Any]]]:
        """Claim pending query-embedding jobs (the kernel hands out a small batch atomically).

        Returns None when the kernel 404s the endpoint (old core) so the poller can stop for
        good; otherwise the (possibly empty) claimed list ``[{id, kind, payload}]``."""
        url = (
            f"{self.s.api}/api/v1/ai/embed-queries"
            f"?worker_id={quote(self.s.worker_id, safe='')}"
        )
        resp = self._request("GET", url, allow_404=True)
        if resp is None:
            return None
        try:
            queries = resp.json().get("queries", [])
        except (ValueError, AttributeError):
            return []
        return [q for q in queries if isinstance(q, dict)]

    def post_embed_query_result(
        self,
        query_id: str,
        *,
        vec: Optional[List[float]] = None,
        model: Optional[str] = None,
        error: Optional[str] = None,
    ) -> None:
        """Report one embed-query outcome: a vector on success, ``{"error": ...}`` on failure (so
        the /search/semantic request blocked on it fails fast instead of timing out). First result
        wins server-side; late duplicates are a 200 no-op, and a 404 (query pruned) is ignored."""
        if error is not None:
            body: Dict[str, Any] = {"error": error}
        else:
            body = {"vec": [float(v) for v in vec or []], "model": model, "dim": len(vec or [])}
        url = f"{self.s.api}/api/v1/ai/embed-queries/{quote(str(query_id), safe='')}/result"
        self._request("POST", url, json=body, allow_404=True)


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
        # Set on the first 404 from POST /ai/embeddings (a kernel that predates issue #38):
        # log once, then drop embeddings for this task instead of warning on every frame.
        self._embeddings_unsupported = False

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
        # Skip a stale frame (sampler stalled / network lag): analyzing seconds-old footage produces
        # misleading events. Threshold = max(2s, 3× the task period) to tolerate normal jitter.
        if frame.age_ms is not None:
            stale_ms = max(2000.0, self.task.period * 1000.0 * 3.0)
            if frame.age_ms > stale_ms:
                self.log.debug("frame too old (age=%dms > %.0fms); skipping", frame.age_ms, stale_ms)
                return
        self._last_captured = frame.captured_at

        result = self.analyzer.analyze(frame)
        if result.is_empty:
            return
        # Per-frame idempotency key: the sampler's capture timestamp (monotonic, restart-safe),
        # namespaced by task so multiple tasks on one camera don't collide. Omitted if the frame
        # carried no capture time (then Core accepts every batch).
        frame_id = (
            f"{self.task.id}:{frame.captured_at}" if frame.captured_at else None
        )
        # Detections/events and embeddings travel on separate endpoints; an embedding task
        # returns ONLY embeddings, so each POST happens iff there is something to say.
        if result.detections or result.event is not None:
            ingested = self.client.post_results(self.task, result, frame_id=frame_id)
            self.log.info(
                "posted %d detection(s)%s",
                len(result.detections),
                f" + event '{result.event.event_type}'" if result.event else "",
            )
            self.log.debug("server ingested=%d", ingested)
        if result.embeddings:
            self._post_embeddings(result, frame_id)

    def _post_embeddings(self, result: AnalysisResult, frame_id: Optional[str]) -> None:
        if self._embeddings_unsupported:
            return
        ingested = self.client.post_embeddings(
            self.task, result.embeddings, frame_id=frame_id
        )
        if ingested is None:
            self._embeddings_unsupported = True
            self.log.warning(
                "kernel has no /ai/embeddings endpoint (predates semantic search); "
                "dropping embeddings for this task — upgrade Heldar Core to index them"
            )
            return
        self.log.info("posted %d embedding(s)", len(result.embeddings))
        self.log.debug("server embeddings_ingested=%d", ingested)

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
# Embed-query worker (semantic search, issue #38)
# --------------------------------------------------------------------------- #
class EmbedQueryWorker(threading.Thread):
    """Daemon thread that answers query-embedding jobs for /search/semantic.

    The kernel's semantic-search route enqueues one job per query and blocks (~3s) on the
    answer, so this poller runs FAST (default 1s, ``--embed-poll-interval``) and independently
    of the task supervisor. Each claimed job goes through the shared CLIP backend — kind "text"
    via the text tower, kind "image" via base64 -> PIL -> the image tower — and its result is
    POSTed back; any failure is reported as ``{"error": ...}`` so the search fails fast instead
    of timing out.

    Degrades cleanly on every axis:
      * old kernel (404 on the endpoint) — log once, stop polling;
      * open_clip/torch not installed (ImportError) — answer the claimed jobs with "clip
        backend unavailable", warn once, stop polling;
      * transient backend-load failure (e.g. a checkpoint download blip) — answer the claimed
        jobs with an error so their searches fail fast, keep polling, and retry the load on
        the next claimed query.
    """

    def __init__(self, client: CoreClient, settings: Settings):
        super().__init__(name="embed-queries", daemon=True)
        self.client = client
        self.s = settings
        self.log = logging.getLogger("worker.embed")
        self._clip: Optional[_ClipBackend] = None
        self._backend_warned = False

    def _backend(self) -> _ClipBackend:
        """Load the shared CLIP backend lazily on the first claimed query, so deployments that
        never issue a semantic search never pay the model load."""
        if self._clip is None:
            import torch

            device = 0 if torch.cuda.is_available() else "cpu"
            self._clip = _get_clip_backend(self.s.clip_model, self.s.clip_pretrained, device)
        return self._clip

    def _answer(self, query: Dict[str, Any]) -> bool:
        """Embed one claimed query and POST its result. Returns False only when the CLIP backend
        cannot exist on this host (missing deps) — the caller then stops the thread."""
        query_id = str(query.get("id") or "")
        if not query_id:
            return True
        try:
            clip = self._backend()
        except ImportError as exc:
            # Deps genuinely absent — they will not appear this run; stop for good.
            if not self._backend_warned:
                self._backend_warned = True
                self.log.warning(
                    "CLIP backend unavailable (%s); semantic search needs "
                    "`pip install -r requirements-embed.txt` — disabling query embedding",
                    exc,
                )
            self._post_error(query_id, "clip backend unavailable")
            return False
        except Exception as exc:  # noqa: BLE001 - e.g. a transient checkpoint-download failure
            # Fail THIS query fast, but keep the thread alive — the load is retried on the next
            # claimed query, so a network blip doesn't disable semantic search until restart.
            self.log.warning("CLIP backend load failed (will retry): %s", exc)
            self._post_error(query_id, f"clip backend load failed: {exc}")
            return True
        kind = str(query.get("kind") or "")
        payload = query.get("payload") or ""
        try:
            if kind == "text":
                vec = clip.encode_text(str(payload))
            elif kind == "image":
                img = Image.open(io.BytesIO(base64.b64decode(payload))).convert("RGB")
                vec = clip.encode_images([img])[0]
            else:
                raise ValueError(f"unknown embed-query kind {kind!r}")
        except Exception as exc:  # noqa: BLE001 - one bad query must not kill the poller
            self.log.warning("embed query %s failed: %s", query_id, exc)
            self._post_error(query_id, str(exc) or type(exc).__name__)
            return True
        try:
            self.client.post_embed_query_result(query_id, vec=vec, model=clip.model_id)
            self.log.info("answered embed query %s kind=%s dim=%d", query_id, kind, len(vec))
        except WorkerHTTPError as exc:
            self.log.error("failed to post embed-query result %s: %s", query_id, exc)
        return True

    def _post_error(self, query_id: str, message: str) -> None:
        try:
            self.client.post_embed_query_result(query_id, error=message)
        except WorkerHTTPError as exc:
            self.log.debug("could not report embed-query error for %s: %s", query_id, exc)

    def run(self) -> None:
        self.log.info(
            "embed-query worker polling every %.1fs (clip=%s/%s, loaded on first query)",
            self.s.embed_poll_interval,
            self.s.clip_model,
            self.s.clip_pretrained,
        )
        while not SHUTDOWN.is_set():
            try:
                queries = self.client.fetch_embed_queries()
                if queries is None:
                    # Old kernel: the endpoint doesn't exist and won't appear this run. Not an
                    # error — semantic search just isn't available until Core is upgraded.
                    self.log.info(
                        "kernel has no embed-queries endpoint; disabling query embedding"
                    )
                    return
                backend_missing = False
                for query in queries:
                    if SHUTDOWN.is_set():
                        return
                    # No break on failure: every claimed job gets its error POSTed (the search
                    # requests blocked on them fail fast), then the thread stops for good.
                    if not self._answer(query):
                        backend_missing = True
                if backend_missing:
                    return
            except WorkerShutdown:
                break
            except WorkerHTTPError as exc:
                self.log.error("embed-query poll failed: %s", exc)
            except Exception:  # noqa: BLE001 - the poller must survive anything
                self.log.exception("unexpected error in embed-query worker")
            if SHUTDOWN.wait(self.s.embed_poll_interval):
                break
        self.log.info("embed-query worker stopped")


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
    log.info("Heldar AI worker starting (api=%s)", settings.api)

    client = CoreClient(settings)
    # Fast side-channel for semantic-search query embeddings (issue #38). Independent of the
    # task supervisor: it must answer within the search route's ~3s budget even while frames
    # are being analyzed. Stops itself against an old kernel or without the CLIP extra.
    embed_queries = EmbedQueryWorker(client, settings)
    embed_queries.start()
    try:
        Supervisor(client, settings).run()
    finally:
        embed_queries.join(timeout=settings.http_timeout + 2)
        client.close()
    log.info("Heldar AI worker stopped")
    return 0


if __name__ == "__main__":
    sys.exit(main())
