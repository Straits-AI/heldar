# Heldar reference AI worker.
# Light image — CORE deps only (requests + pillow + numpy): runs every motion-type task; model-type
# tasks (YOLO/ANPR) are imported lazily by worker.py and are deliberately NOT baked in — torch/opencv
# are ~2 GB per architecture, which bloats the image and overflows a CI runner's disk on a
# multi-arch build. To run YOLO in a container, derive your own image:
#   FROM ghcr.io/straits-ai/heldar-ai
#   RUN pip install --no-cache-dir "ultralytics>=8.3" "lap>=0.5"
# Pinned base: concrete minor.patch tag + multi-arch index @sha256 digest for reproducibility.
# To bump: `docker buildx imagetools inspect python:<tag>` and update the digest (see docs/SUPPLY-CHAIN.md).
FROM python:3.14.7-slim@sha256:656d12e70054d5fda18a045e2494c96701e9792dd1445f95b3d038df954f57e9

# Keep Python lean and logs unbuffered (so they stream out of the container).
ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    HELDAR_API=http://core:8000

WORKDIR /app

# Install deps first for better layer caching.
COPY requirements-core.txt .
RUN pip install --no-cache-dir -r requirements-core.txt

COPY worker.py .

# Run as an unprivileged user.
RUN useradd --create-home --uid 10001 worker
USER worker

ENTRYPOINT ["python", "worker.py"]
