# Heldar reference AI worker.
# Light image — the worker only needs requests + pillow + numpy (no GPU/model).
FROM python:3.13-slim

# Keep Python lean and logs unbuffered (so they stream out of the container).
ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    HELDAR_API=http://core:8000

WORKDIR /app

# Install deps first for better layer caching.
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY worker.py .

# Run as an unprivileged user.
RUN useradd --create-home --uid 10001 worker
USER worker

ENTRYPOINT ["python", "worker.py"]
