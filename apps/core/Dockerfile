# VisionOps Core — multi-stage container build.
# Build context is apps/core/. Runtime image bundles FFmpeg (recorder/clip/snapshot need it).

FROM rust:1-bookworm AS builder
WORKDIR /app
# Cache dependencies: copy manifests first, build a stub, then the real sources.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release && rm -rf src
COPY migrations ./migrations
COPY src ./src
# Touch main so cargo rebuilds the real binary.
RUN touch src/main.rs && cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ffmpeg ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/visionops-core /usr/local/bin/visionops-core
ENV VISIONOPS_DATA_DIR=/data \
    VISIONOPS_DATABASE_URL=sqlite:///data/visionops.db \
    VISIONOPS_API_HOST=0.0.0.0 \
    VISIONOPS_API_PORT=8000
VOLUME ["/data"]
EXPOSE 8000
ENTRYPOINT ["visionops-core"]
