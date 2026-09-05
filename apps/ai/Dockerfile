# Heldar reference AI worker.
# Light image — CORE deps only (requests + pillow + numpy): runs every motion-type task; model-type
# tasks (YOLO/ANPR) are imported lazily by worker.py and are deliberately NOT baked in — torch/opencv
# are ~2 GB per architecture, which bloats the image and overflows a CI runner's disk on a
# multi-arch build. To run YOLO in a container, derive your own image:
#   FROM ghcr.io/straits-ai/heldar-ai
#   RUN pip install --no-cache-dir "ultralytics>=8.4.115" "lap>=0.5.13"
# Pinned base: concrete minor.patch tag + multi-arch index @sha256 digest for reproducibility.
# To bump: `docker buildx imagetools inspect python:<tag>` and update the digest (see docs/SUPPLY-CHAIN.md).
# Scanned by Trivy (HIGH/CRITICAL, fixed only) BEFORE anything is published — see
# docs/SUPPLY-CHAIN.md. A base bump that drags in a fixable HIGH fails the build rather than
# shipping. Bump the pinned digest to fix; where there is no fix yet, record it in
# security/dependency-exceptions.json with an owner and an expiry.
FROM python:3.14.7-slim@sha256:cad9a2c871761c413caa6fdd6441c783451e740a48aaeba60ae62a8b53525ef6

# Keep Python lean and logs unbuffered (so they stream out of the container).
ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    HELDAR_API=http://core:8000

WORKDIR /app

# Install deps first for better layer caching.
COPY requirements-core.txt .
# The first image scan (#114) found a fixable HIGH in setuptools (CVE-2025-47273) and in the
# msgpack pip vendors (GHSA-6v7p-g79w-8964). `pip install --upgrade` does NOT clear either: the
# setuptools Trivy sees is a bundled WHEEL under ensurepip/_bundled, and msgpack lives inside
# pip's own _vendor tree. Upgrading leaves both files exactly where they were.
#
# The container's entrypoint is `python worker.py`. It installs nothing at runtime, so pip and
# ensurepip are build-time tools that only ship risk — removed once the dependencies are in.
# This also shrinks the image. If a future change needs pip at runtime, that is the thing to
# reconsider, not this deletion.
# Deleted rather than `pip uninstall`ed: `A && B || true && C` parses as `((A && B) || true) && C`,
# so a `|| true` here would swallow a FAILED dependency install and still build the image. rm is
# deterministic and needs no error handling.
RUN pip install --no-cache-dir -r requirements-core.txt \
 && rm -rf /usr/local/lib/python3.14/ensurepip \
           /usr/local/lib/python3.14/site-packages/pip \
           /usr/local/lib/python3.14/site-packages/pip-*.dist-info \
           /usr/local/lib/python3.14/site-packages/setuptools \
           /usr/local/lib/python3.14/site-packages/setuptools-*.dist-info

COPY worker.py .

# Run as an unprivileged user.
RUN useradd --create-home --uid 10001 worker
USER worker

ENTRYPOINT ["python", "worker.py"]
