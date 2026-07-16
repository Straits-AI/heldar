#!/usr/bin/env python3
"""ANPR accuracy benchmark harness (issue #37).

Measures OCR plate-reading accuracy (and, when configured, the DIY make/model classifier) against
YOUR footage — accuracy claims only mean anything on locally representative plates/lighting, so the
loop is: collect → label → score.

1) COLLECT — run the same vehicle-detect + OCR pipeline the worker uses over a directory of images,
   a video file, or live frames pulled from the kernel, and write per-vehicle crops plus a
   `manifest.csv` with every backend's read side by side and an empty `truth` column:

     python3 anpr_bench.py collect --images  /path/to/frames  --out bench/
     python3 anpr_bench.py collect --video   gate.mp4 --every 10 --out bench/
     python3 anpr_bench.py collect --kernel  http://127.0.0.1:8000 --camera cam7 \
                                   --api-key vok_... --duration 300 --out bench/

2) LABEL — open `bench/manifest.csv`, look at each crop in `bench/crops/`, and fill the `truth`
   column with the real plate (leave blank when unreadable by a human too; those score separately).
   Optionally fill `truth_make_model` ("Make Model") to score the classifier.

3) SCORE — compute per-backend exact accuracy, character accuracy (Levenshtein), and read rate:

     python3 anpr_bench.py score --out bench/

Dependencies: whatever the worker already uses (ultralytics + PIL + numpy; paddleocr/easyocr
optional — backends that aren't installed simply score as absent). No kernel required for
images/video modes, so this runs anywhere the footage is.
"""

from __future__ import annotations

import argparse
import csv
import io
import json
import logging
import sys
import time
from pathlib import Path
from typing import Any, Dict, List, Optional

log = logging.getLogger("anpr_bench")

VEHICLE_CLASSES = {"car", "truck", "bus", "motorcycle"}


# ---------------------------------------------------------------- collection


def load_detector(weights: str):
    from ultralytics import YOLO  # noqa: PLC0415

    model = YOLO(weights)
    names = dict(model.names)
    classes = sorted(i for i, n in names.items() if n.lower() in VEHICLE_CLASSES)
    return model, names, classes


def iter_frames(args) -> "list":
    """Yield (source_id, PIL.Image) pairs from whichever input was chosen."""
    from PIL import Image  # noqa: PLC0415

    if args.images:
        root = Path(args.images)
        exts = {".jpg", ".jpeg", ".png", ".bmp"}
        for p in sorted(root.rglob("*")):
            if p.suffix.lower() in exts:
                try:
                    yield (p.name, Image.open(p).convert("RGB"))
                except Exception as exc:  # noqa: BLE001
                    log.warning("skip %s: %s", p, exc)
        return

    if args.video:
        try:
            import cv2  # noqa: PLC0415
        except ImportError:
            sys.exit("video mode needs opencv-python (pip install opencv-python)")
        cap = cv2.VideoCapture(args.video)
        idx = 0
        while True:
            ok, frame = cap.read()
            if not ok:
                break
            if idx % max(1, args.every) == 0:
                rgb = cv2.cvtColor(frame, cv2.COLOR_BGR2RGB)
                yield (f"frame{idx:07d}", Image.fromarray(rgb))
            idx += 1
        cap.release()
        return

    if args.kernel:
        import urllib.request  # noqa: PLC0415

        deadline = time.time() + args.duration
        seq = 0
        url = f"{args.kernel.rstrip('/')}/api/v1/cameras/{args.camera}/frame?profile=sub"
        while time.time() < deadline:
            req = urllib.request.Request(url)
            if args.api_key:
                req.add_header("X-API-Key", args.api_key)
            try:
                with urllib.request.urlopen(req, timeout=10) as resp:
                    data = resp.read()
                yield (f"live{seq:07d}", Image.open(io.BytesIO(data)).convert("RGB"))
                seq += 1
            except Exception as exc:  # noqa: BLE001
                log.warning("frame fetch failed: %s", exc)
            time.sleep(max(0.2, args.interval))
        return

    sys.exit("choose an input: --images DIR | --video FILE | --kernel URL --camera ID")


def make_ocr_backends(log_adapter) -> Dict[str, Any]:
    """Every OCR backend that is installed, so reads can be compared side by side."""
    sys.path.insert(0, str(Path(__file__).parent))
    from worker import _OcrBackend  # noqa: PLC0415

    backends: Dict[str, Any] = {}
    for kind in ("paddleocr", "easyocr"):
        b = _OcrBackend(kind, log_adapter)
        if b.enabled and b.kind == kind:
            backends[kind] = b
    return backends


def cmd_collect(args) -> None:
    out = Path(args.out)
    crops_dir = out / "crops"
    crops_dir.mkdir(parents=True, exist_ok=True)
    manifest = out / "manifest.csv"

    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(message)s")
    adapter = logging.LoggerAdapter(log, {})
    model, names, classes = load_detector(args.weights)
    backends = make_ocr_backends(adapter)
    if not backends:
        log.warning("no OCR backend installed — crops will still be collected for labeling")

    mm = None
    if args.make_model_onnx and args.make_model_labels:
        from worker import _MakeModelClassifier  # noqa: PLC0415

        mm = _MakeModelClassifier(
            {
                "make_model_onnx": args.make_model_onnx,
                "make_model_labels": args.make_model_labels,
                "make_model_min_conf": 0.0,  # record everything; the floor is a scoring decision
            },
            adapter,
        )
        if not mm.enabled:
            mm = None

    fields = ["crop", "source", "vehicle_type", "det_conf"]
    for kind in ("paddleocr", "easyocr"):
        fields += [f"{kind}_text", f"{kind}_conf"]
    fields += ["mm_pred", "mm_conf", "truth", "truth_make_model"]

    n_crops = 0
    with open(manifest, "w", newline="", encoding="utf-8") as fh:
        writer = csv.DictWriter(fh, fieldnames=fields)
        writer.writeheader()
        for source, img in iter_frames(args):
            results = model(img, conf=args.threshold, classes=classes or None, verbose=False)
            if not results:
                continue
            boxes = results[0].boxes
            if boxes is None:
                continue
            for box in boxes:
                x1, y1, x2, y2 = (int(v) for v in box.xyxy[0].tolist())
                if (x2 - x1) < args.min_px or (y2 - y1) < args.min_px:
                    continue
                crop = img.crop((x1, y1, x2, y2))
                name = f"{n_crops:06d}_{source}.jpg"
                crop.save(crops_dir / name, quality=92)
                row: Dict[str, Any] = {
                    "crop": name,
                    "source": source,
                    "vehicle_type": names.get(int(box.cls.item()), "?"),
                    "det_conf": round(float(box.conf.item()), 3),
                    "truth": "",
                    "truth_make_model": "",
                }
                for kind, backend in backends.items():
                    read = backend.read_plate(crop)
                    row[f"{kind}_text"] = read[0] if read else ""
                    row[f"{kind}_conf"] = round(read[1], 3) if read else ""
                if mm:
                    pred = mm.classify(crop)
                    row["mm_pred"] = f"{pred[0]} {pred[1]}".strip() if pred else ""
                    row["mm_conf"] = round(pred[2], 3) if pred else ""
                writer.writerow(row)
                n_crops += 1
    log.info(
        "collected %d vehicle crops -> %s (label the `truth` column, then run `score`)",
        n_crops,
        manifest,
    )


# ---------------------------------------------------------------- scoring


def levenshtein(a: str, b: str) -> int:
    if not a:
        return len(b)
    if not b:
        return len(a)
    prev = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        cur = [i]
        for j, cb in enumerate(b, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (ca != cb)))
        prev = cur
    return prev[-1]


def norm_plate(s: str) -> str:
    return "".join(c for c in (s or "").upper() if c.isalnum())


def cmd_score(args) -> None:
    manifest = Path(args.out) / "manifest.csv"
    rows = list(csv.DictReader(open(manifest, encoding="utf-8")))
    labeled = [r for r in rows if norm_plate(r.get("truth", ""))]
    if not labeled:
        sys.exit(f"no labeled rows in {manifest} — fill the `truth` column first")

    report: Dict[str, Any] = {
        "total_crops": len(rows),
        "labeled": len(labeled),
        "backends": {},
    }
    for kind in ("paddleocr", "easyocr"):
        col = f"{kind}_text"
        if not any(r.get(col) for r in rows):
            continue
        exact = 0
        char_acc_sum = 0.0
        read = 0
        for r in labeled:
            truth = norm_plate(r["truth"])
            pred = norm_plate(r.get(col, ""))
            if pred:
                read += 1
            if pred == truth:
                exact += 1
            denom = max(len(truth), len(pred), 1)
            char_acc_sum += 1.0 - (levenshtein(truth, pred) / denom)
        n = len(labeled)
        report["backends"][kind] = {
            "exact_accuracy": round(exact / n, 4),
            "char_accuracy": round(char_acc_sum / n, 4),
            "read_rate": round(read / n, 4),
            "labeled": n,
        }

    mm_labeled = [r for r in labeled if (r.get("truth_make_model") or "").strip()]
    if mm_labeled and any(r.get("mm_pred") for r in rows):
        hit = sum(
            1
            for r in mm_labeled
            if (r.get("mm_pred") or "").strip().lower()
            == r["truth_make_model"].strip().lower()
        )
        report["make_model"] = {
            "exact_accuracy": round(hit / len(mm_labeled), 4),
            "labeled": len(mm_labeled),
        }

    out_path = Path(args.out) / "scores.json"
    out_path.write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(json.dumps(report, indent=2))
    print(f"\nwritten to {out_path}")


# ---------------------------------------------------------------- cli


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)

    c = sub.add_parser("collect", help="detect vehicles + run every installed OCR backend; write crops + manifest")
    c.add_argument("--out", required=True, help="output directory (crops/ + manifest.csv)")
    c.add_argument("--images", help="directory of still frames")
    c.add_argument("--video", help="video file (needs opencv-python)")
    c.add_argument("--every", type=int, default=10, help="video: sample every Nth frame")
    c.add_argument("--kernel", help="kernel base URL for live frame pulls")
    c.add_argument("--camera", help="camera id (kernel mode)")
    c.add_argument("--api-key", help="integration API key (kernel mode)")
    c.add_argument("--duration", type=int, default=300, help="kernel mode: seconds to collect")
    c.add_argument("--interval", type=float, default=1.0, help="kernel mode: seconds between frames")
    c.add_argument("--weights", default="yolov8n.pt")
    c.add_argument("--threshold", type=float, default=0.3)
    c.add_argument("--min-px", type=int, default=48, help="skip vehicle boxes smaller than this")
    c.add_argument("--make-model-onnx", help="optional DIY make/model ONNX classifier")
    c.add_argument("--make-model-labels", help="labels file for the classifier")
    c.set_defaults(func=cmd_collect)

    s = sub.add_parser("score", help="score a labeled manifest")
    s.add_argument("--out", required=True, help="directory holding manifest.csv")
    s.set_defaults(func=cmd_score)

    args = ap.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
