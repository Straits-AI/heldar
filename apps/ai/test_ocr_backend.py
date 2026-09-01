#!/usr/bin/env python3
"""Controls for `_OcrBackend`, the ANPR plate reader (#188).

Run: python3 apps/ai/test_ocr_backend.py    (needs requirements-core.txt: numpy, pillow, requests)

`paddleocr>=2.7` has no upper bound, so a fresh install resolves 3.x — and the code was written
against 2.x. Three things differ between the majors (the constructor arguments, the inference call,
and the result shape), and EVERY failure was swallowed: the constructor sat inside `except
Exception` logging a warning, and `read_plate` logged at debug. So the box emitted vehicles with no
plate, indefinitely, while looking healthy.

That is the failure mode these tests exist for. It is not a crash — it renders perfectly and reports
nothing — so nothing but an assertion catches it.

No paddleocr is installed to run these against, and none should be: the point is to pin the SHAPE of
both APIs so a port cannot quietly satisfy one and break the other. The fakes below are written from
the two libraries' documented result structures.
"""

import logging
import sys
import types
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import worker  # noqa: E402

LOG = logging.LoggerAdapter(logging.getLogger("test"), {})
FAILED = []


def check(cond, msg):
    if not cond:
        FAILED.append(msg)
        print(f"  FAIL  {msg}")
    else:
        print(f"  ok    {msg}")


# --- fakes -------------------------------------------------------------------------------------

class Paddle3:
    """PaddleOCR 3.x: rejects `show_log`, takes `use_textline_orientation`, `predict()` only."""

    def __init__(self, **kw):
        for name in kw:
            if name not in ("lang", "use_textline_orientation", "use_angle_cls", "device"):
                # 3.x's real behaviour: an unmapped argument reaches the common-args validator.
                raise ValueError(f"Unknown argument: {name}")
        self.kw = kw

    def predict(self, arr):
        return [{"rec_texts": ["ABC123", "xx"], "rec_scores": [0.91, 0.4]}]

    def ocr(self, *a, **kw):  # deprecated in 3.x and keyword-only underneath
        raise TypeError("predict() got an unexpected keyword argument 'cls'")


class Paddle3Attrs(Paddle3):
    """Same major, result objects exposing attributes rather than mapping access."""

    def predict(self, arr):
        r = types.SimpleNamespace(rec_texts=["ABC123"], rec_scores=[0.91])
        return [r]


class Paddle2:
    """PaddleOCR 2.x: takes `show_log`, rejects `use_textline_orientation`, nested `ocr()` result."""

    def __init__(self, **kw):
        if "use_textline_orientation" in kw:
            raise TypeError("__init__() got an unexpected keyword argument 'use_textline_orientation'")
        self.kw = kw

    def ocr(self, arr, cls=False):
        return [[[[[0, 0], [1, 1]], ("ABC123", 0.91)], [[[0, 0], [1, 1]], ("xx", 0.4)]]]


def backend_with(engine, api):
    b = worker._OcrBackend.__new__(worker._OcrBackend)
    b.log, b.kind, b._engine, b._paddle_api = LOG, "paddleocr", engine, api
    return b


# --- construction ------------------------------------------------------------------------------

eng, api = worker._OcrBackend._build_paddle(Paddle3)
check(api == 3, "a 3.x PaddleOCR is detected as the 3.x API")
check("show_log" not in eng.kw, "3.x is NOT passed show_log — the argument that used to raise")
check(eng.kw.get("use_textline_orientation") is False, "3.x gets use_textline_orientation")

eng2, api2 = worker._OcrBackend._build_paddle(Paddle2)
check(api2 == 2, "a 2.x PaddleOCR falls back to the 2.x API rather than failing")
check(eng2.kw.get("use_angle_cls") is False, "2.x still gets use_angle_cls")

# THE REGRESSION. Before the port this raised and was swallowed, disabling OCR forever.
try:
    worker._OcrBackend._build_paddle(Paddle3)
    check(True, "constructing against 3.x does not raise (the #188 regression)")
except Exception as exc:  # noqa: BLE001
    check(False, f"constructing against 3.x raised {type(exc).__name__}: {exc}")


# --- reading -----------------------------------------------------------------------------------

got3 = backend_with(Paddle3(lang="en"), 3)._read_paddle(None)
check(("ABC123", 0.91) in got3, "3.x results parse from rec_texts/rec_scores")
check(len(got3) == 2, f"3.x returns every candidate, not just the first (got {len(got3)})")

got3a = backend_with(Paddle3Attrs(lang="en"), 3)._read_paddle(None)
check(("ABC123", 0.91) in got3a, "3.x results parse when the object is attribute-like, not mapping-like")

got2 = backend_with(Paddle2(lang="en"), 2)._read_paddle(None)
check(("ABC123", 0.91) in got2, "2.x nested results still parse — the port did not break the old major")

# A 3.x engine driven the 2.x way raises TypeError; the port must not do that.
b3 = backend_with(Paddle3(lang="en"), 3)
try:
    b3._read_paddle(None)
    check(True, "the 3.x path never calls ocr(cls=...), which 3.x rejects")
except TypeError as exc:
    check(False, f"the 3.x path still called the 2.x API: {exc}")


# --- the operator-facing message ---------------------------------------------------------------

def init_with(module_or_none):
    saved = sys.modules.get("paddleocr")
    if module_or_none is None:
        sys.modules["paddleocr"] = None  # import raises ImportError
    else:
        sys.modules["paddleocr"] = module_or_none
    records = []
    logger = logging.getLogger("ocr-msg")
    logger.handlers = [type("H", (logging.Handler,), {"emit": lambda s, r: records.append(r.getMessage())})()]
    logger.setLevel(logging.DEBUG)
    logger.propagate = False
    b = worker._OcrBackend.__new__(worker._OcrBackend)
    b.log, b.kind, b._engine, b._paddle_api = logging.LoggerAdapter(logger, {}), None, None, None
    b._init("paddleocr")
    if saved is None:
        sys.modules.pop("paddleocr", None)
    else:
        sys.modules["paddleocr"] = saved
    return b, " | ".join(records)


class Exploding:
    def __init__(self, **kw):
        raise RuntimeError("libpaddle.so: cannot open shared object file")


mod = types.ModuleType("paddleocr")
mod.PaddleOCR = Exploding
_, msg_broken = init_with(mod)
check(
    "installed but failed to start" in msg_broken,
    "an installed-but-broken backend says so, rather than 'install paddleocr'",
)
check(
    "cannot open shared object" in msg_broken,
    "the underlying error reaches the operator instead of being dropped",
)

_, msg_missing = init_with(None)
check("not installed" in msg_missing, "a genuinely absent backend is reported as not installed")
check(
    "installed but failed to start" not in msg_missing,
    "an absent backend is NOT reported as broken — the two cases must stay distinguishable",
)

mod_ok = types.ModuleType("paddleocr")
mod_ok.PaddleOCR = Paddle3
b_ok, _ = init_with(mod_ok)
check(b_ok.enabled, "a working 3.x backend ends up enabled")
check(b_ok._paddle_api == 3, "and records which API it is driving")


print()
if FAILED:
    print(f"{len(FAILED)} failure(s)")
    sys.exit(1)
print("all OCR backend checks passed")
